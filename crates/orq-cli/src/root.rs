//! Workspace root discovery and path helpers.

use std::path::{Component, Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};

/// Resolves the workspace root: `explicit` if given, else the workspace
/// containing the current directory according to cargo.
pub fn resolve(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(dir) = explicit {
        return dir
            .canonicalize()
            .with_context(|| format!("invalid --root {}", dir.display()));
    }
    let output = Command::new("cargo")
        .args(["locate-project", "--workspace", "--message-format", "plain"])
        .output()
        .context("failed to run `cargo locate-project`")?;
    if !output.status.success() {
        bail!(
            "not inside a cargo workspace: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let manifest = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
    let dir = manifest
        .parent()
        .context("cargo returned a manifest path without a parent")?;
    dir.canonicalize()
        .with_context(|| format!("invalid workspace root {}", dir.display()))
}

/// Converts a path given relative to the current directory into one relative
/// to `root`. Paths outside `root` are kept (absolute) as resolved.
pub fn relative_to_root(root: &Path, file: &Path) -> PathBuf {
    let cwd = std::env::current_dir().unwrap_or_default();
    let absolute = if file.is_absolute() {
        file.to_path_buf()
    } else {
        cwd.join(file)
    };
    let absolute = normalize(&absolute);
    // Canonicalize the parent so symlinked prefixes (e.g. /tmp) match `root`.
    let resolved = match (absolute.parent(), absolute.file_name()) {
        (Some(parent), Some(name)) => parent
            .canonicalize()
            .map(|p| p.join(name))
            .unwrap_or_else(|_| absolute.clone()),
        _ => absolute.clone(),
    };
    resolved
        .strip_prefix(root)
        .or_else(|_| absolute.strip_prefix(root))
        .map(Path::to_path_buf)
        .unwrap_or(resolved)
}

fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Directory where cargo writes build output (honours `CARGO_TARGET_DIR`).
pub fn target_dir(root: &Path) -> PathBuf {
    match std::env::var_os("CARGO_TARGET_DIR").filter(|d| !d.is_empty()) {
        Some(dir) => root.join(dir),
        None => root.join("target"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_removes_dots() {
        assert_eq!(normalize(Path::new("/a/./b/../c")), PathBuf::from("/a/c"));
    }
}
