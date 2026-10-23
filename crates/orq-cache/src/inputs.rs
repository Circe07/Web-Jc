//! Collection of the inputs that feed the cache key.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use orq_core::{CoreError, ImpactSet};

/// Files at the workspace root that influence every build.
pub(crate) const GLOBAL_FILES: &[&str] = &[
    "Cargo.lock",
    "Cargo.toml",
    "rust-toolchain",
    "rust-toolchain.toml",
    ".cargo/config",
    ".cargo/config.toml",
    // Options such as `deny_warnings` change the outcome.
    "orq.toml",
];

/// Environment variables hashed verbatim when set.
const ENV_EXACT: &[&str] = &[
    "RUSTFLAGS",
    "RUSTDOCFLAGS",
    "RUSTC",
    "RUSTC_WRAPPER",
    "RUSTC_WORKSPACE_WRAPPER",
    "RUSTUP_TOOLCHAIN",
    "CARGO_ENCODED_RUSTFLAGS",
    "CARGO_INCREMENTAL",
];

/// Environment variable prefixes hashed when set.
const ENV_PREFIXES: &[&str] = &[
    "CARGO_BUILD_",
    "CARGO_PROFILE_",
    "CARGO_TARGET_",
    "CARGO_CFG_",
];

/// Variables matching a relevant prefix that do not affect results.
const ENV_EXCLUDED: &[&str] = &[
    "CARGO_TARGET_DIR",
    "CARGO_BUILD_TARGET_DIR",
    "CARGO_BUILD_JOBS",
    "CARGO_BUILD_BUILD_DIR",
];

/// `true` if `name` is part of the key.
pub(crate) fn env_is_relevant(name: &str) -> bool {
    if ENV_EXCLUDED.contains(&name) {
        return false;
    }
    ENV_EXACT.contains(&name) || ENV_PREFIXES.iter().any(|p| name.starts_with(p))
}

/// Relevant environment entries, sorted by name.
pub(crate) fn relevant_env(all: Vec<(String, String)>) -> Vec<(String, String)> {
    let mut v: Vec<_> = all
        .into_iter()
        .filter(|(k, _)| env_is_relevant(k))
        .collect();
    v.sort();
    v
}

/// Relative paths (to `root`) of the files that belong to the crates of
/// `impact` (or the whole workspace on `full_rebuild`), sorted and unique.
pub(crate) fn collect_files(
    root: &Path,
    impact: &ImpactSet,
) -> Result<BTreeSet<PathBuf>, CoreError> {
    let canon_root = root.canonicalize().ok();
    let mut dirs: Vec<PathBuf> = Vec::new();
    if impact.full_rebuild {
        dirs.push(PathBuf::new());
    } else {
        for krate in impact.affected() {
            let dir = krate.manifest_path.parent().unwrap_or(Path::new(""));
            let rel = dir
                .strip_prefix(root)
                .ok()
                .or_else(|| canon_root.as_deref().and_then(|c| dir.strip_prefix(c).ok()));
            // Outside the root: keep the absolute path and walk it.
            dirs.push(rel.map_or_else(|| dir.to_path_buf(), Path::to_path_buf));
        }
    }

    let mut files = BTreeSet::new();
    let tracked = git_files(root);
    for dir in &dirs {
        match (&tracked, dir.is_absolute()) {
            (Some(all), false) => {
                files.extend(all.iter().filter(|f| f.starts_with(dir)).cloned());
            }
            _ => walk(root, dir, &mut files)?,
        }
    }
    Ok(files)
}

/// `git ls-files` (tracked plus untracked-not-ignored), relative to `root`.
/// `None` if `root` is not usable as a git work tree.
fn git_files(root: &Path) -> Option<Vec<PathBuf>> {
    let out = Command::new("git")
        .args([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ])
        .current_dir(root)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let mut v: Vec<PathBuf> = out
        .stdout
        .split(|b| *b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| PathBuf::from(String::from_utf8_lossy(s).into_owned()))
        .collect();
    v.sort();
    v.dedup();
    Some(v)
}

/// Fallback: recursive walk skipping `target/` and hidden directories.
fn walk(root: &Path, dir: &Path, out: &mut BTreeSet<PathBuf>) -> Result<(), CoreError> {
    let abs = if dir.is_absolute() {
        dir.to_path_buf()
    } else {
        root.join(dir)
    };
    let mut stack = vec![abs];
    while let Some(d) = stack.pop() {
        let entries = match std::fs::read_dir(&d) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(CoreError::io(d, e)),
        };
        for entry in entries {
            let entry = entry.map_err(|e| CoreError::io(&d, e))?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let path = entry.path();
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_dir() {
                if name == "target" || name.starts_with('.') {
                    continue;
                }
                stack.push(path);
            } else if ft.is_file() {
                let rel = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
                out.insert(rel);
            }
        }
    }
    Ok(())
}
