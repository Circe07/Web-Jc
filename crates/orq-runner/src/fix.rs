//! Applies the machine-applicable suggestions of rustc and clippy
//! (`orq fix`, MCP `apply_fix`; PLAN.md T5.3).
//!
//! The work is done by `cargo clippy --fix`, which applies the
//! `MachineApplicable` suggestions of both rustc and clippy. `--broken-code`
//! is deliberately not passed: it would let cargo write fixes even when the
//! result no longer compiles, and an agent should never be handed a worse
//! tree than the one it had. When the code does not compile, cargo refuses to
//! fix and [`FixOutcome::cargo_ok`] is `false`; the verify that follows
//! reports the errors.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use orq_core::{BuildOptions, ImpactSet};
use tokio::process::Command;

use crate::RunnerError;

/// What [`apply_fixes`] changed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FixOutcome {
    /// Files modified, relative to the workspace root, sorted.
    pub files_changed: Vec<PathBuf>,
    /// `false` if `cargo clippy --fix` exited with an error (for instance the
    /// code does not compile, so nothing could be fixed).
    pub cargo_ok: bool,
}

/// Applies machine-applicable fixes to the crates of `impact` (the whole
/// workspace on `full_rebuild`).
///
/// Changed files are found by hashing the `.rs` files of those crates before
/// and after the run, so it works without git. Nothing applicable gives an
/// empty `files_changed`. Only a failure to run cargo is an error.
pub async fn apply_fixes(
    root: &Path,
    impact: &ImpactSet,
    build: &BuildOptions,
) -> Result<FixOutcome, RunnerError> {
    if !root.is_dir() {
        return Err(RunnerError::InvalidRoot(root.to_path_buf()));
    }
    if impact.is_empty() {
        return Ok(FixOutcome {
            files_changed: Vec::new(),
            cargo_ok: true,
        });
    }
    let dirs: Vec<PathBuf> = if impact.full_rebuild {
        vec![root.to_path_buf()]
    } else {
        impact
            .affected()
            .iter()
            .filter_map(|c| c.manifest_path.parent().map(Path::to_path_buf))
            .collect()
    };
    let before = snapshot(root, &dirs).await;

    let mut args: Vec<String> = vec![
        "clippy".into(),
        "--fix".into(),
        "--allow-dirty".into(),
        "--allow-staged".into(),
        "--allow-no-vcs".into(),
    ];
    if impact.full_rebuild {
        args.push("--workspace".into());
    } else {
        for c in impact.affected() {
            args.extend(["-p".into(), c.name]);
        }
    }
    let cargo: OsString = std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));
    // Same target selection as the check/clippy steps (`Targets::Auto` skips
    // unchanged benches/examples, which may need nightly), so `fix` touches
    // exactly what `verify` compiles.
    args.extend(
        crate::targets::compile_target_args(cargo.clone(), root, impact, build.targets).await,
    );
    args.extend(build.cargo_args());

    let command = format!("cargo {}", args.join(" "));
    tracing::debug!(%command, "running");
    let output = Command::new(cargo)
        .args(&args)
        .current_dir(root)
        .env("CARGO_TERM_COLOR", "never")
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|source| RunnerError::Spawn { command, source })?;
    if !output.status.success() {
        tracing::debug!(
            stderr = %String::from_utf8_lossy(&output.stderr),
            "cargo clippy --fix failed"
        );
    }

    let after = snapshot(root, &dirs).await;
    let mut files_changed: Vec<PathBuf> = after
        .iter()
        .filter(|(path, hash)| before.get(*path) != Some(*hash))
        .map(|(path, _)| path.clone())
        .collect();
    files_changed.sort();
    Ok(FixOutcome {
        files_changed,
        cargo_ok: output.status.success(),
    })
}

/// Hashes every `.rs` file below `dirs`, keyed by path relative to `root`.
async fn snapshot(root: &Path, dirs: &[PathBuf]) -> BTreeMap<PathBuf, [u8; 32]> {
    let root = root.to_path_buf();
    let dirs = dirs.to_vec();
    tokio::task::spawn_blocking(move || {
        let mut map = BTreeMap::new();
        for dir in &dirs {
            walk(&root, dir, &mut map);
        }
        map
    })
    .await
    .unwrap_or_default()
}

fn walk(root: &Path, dir: &Path, map: &mut BTreeMap<PathBuf, [u8; 32]>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            if name != "target" && !name.starts_with('.') {
                walk(root, &path, map);
            }
        } else if kind.is_file() && name.ends_with(".rs") {
            if let Ok(bytes) = std::fs::read(&path) {
                let rel = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
                map.insert(rel, *blake3::hash(&bytes).as_bytes());
            }
        }
    }
}
