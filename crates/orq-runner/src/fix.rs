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

/// Most file names listed by [`FixOutcome::summary`].
const SUMMARY_FILES: usize = 3;

impl FixOutcome {
    /// One short line telling the agent which files orq edited, such as
    /// `fixed: 2 files (src/a.rs, src/b.rs)`; `None` when nothing changed.
    ///
    /// Lint names are not part of it: `cargo clippy --fix` does not say which
    /// lint produced each edit.
    pub fn summary(&self) -> Option<String> {
        if self.files_changed.is_empty() {
            return None;
        }
        let n = self.files_changed.len();
        let mut names: Vec<String> = self
            .files_changed
            .iter()
            .take(SUMMARY_FILES)
            .map(|p| p.display().to_string())
            .collect();
        if n > SUMMARY_FILES {
            names.push(format!("+{}", n - SUMMARY_FILES));
        }
        let noun = if n == 1 { "file" } else { "files" };
        Some(format!("fixed: {n} {noun} ({})", names.join(", ")))
    }
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
    args.extend(crate::compile_package_args(impact));
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

#[cfg(test)]
mod tests {
    use super::*;

    fn outcome(files: &[&str]) -> FixOutcome {
        FixOutcome {
            files_changed: files.iter().map(PathBuf::from).collect(),
            cargo_ok: true,
        }
    }

    #[test]
    fn summary_is_none_without_changes() {
        assert_eq!(outcome(&[]).summary(), None);
    }

    #[test]
    fn summary_names_the_files() {
        assert_eq!(
            outcome(&["src/a.rs"]).summary().as_deref(),
            Some("fixed: 1 file (src/a.rs)")
        );
        assert_eq!(
            outcome(&["src/a.rs", "src/b.rs"]).summary().as_deref(),
            Some("fixed: 2 files (src/a.rs, src/b.rs)")
        );
    }

    #[test]
    fn summary_caps_the_list() {
        assert_eq!(
            outcome(&["a.rs", "b.rs", "c.rs", "d.rs", "e.rs"])
                .summary()
                .as_deref(),
            Some("fixed: 5 files (a.rs, b.rs, c.rs, +2)")
        );
    }
}
