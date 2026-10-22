//! Workspace root discovery and path helpers.

use std::path::{Component, Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};

/// Resolves the workspace root: `explicit` if given, else the workspace
/// containing the current directory.
///
/// Walking up the directory tree avoids spawning `cargo` (~70 ms) on the
/// common layouts; `cargo locate-project` is only used when the nearest
/// workspace manifest `exclude`s the current package, or when no manifest
/// is found at all.
pub fn resolve(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(dir) = explicit {
        return dir
            .canonicalize()
            .with_context(|| format!("invalid --root {}", dir.display()));
    }
    let cwd = std::env::current_dir()
        .and_then(|d| d.canonicalize())
        .context("cannot read the current directory")?;
    match find_workspace(&cwd) {
        Some(root) => Ok(root),
        None => locate_with_cargo(),
    }
}

/// The nearest ancestor of `start` whose `Cargo.toml` declares `[workspace]`
/// (or the nearest package if none does), unless the answer is ambiguous.
fn find_workspace(start: &Path) -> Option<PathBuf> {
    let mut nearest_package = None;
    for dir in start.ancestors() {
        let Ok(manifest) = std::fs::read_to_string(dir.join("Cargo.toml")) else {
            continue;
        };
        let has_table = |name: &str| manifest.lines().any(|l| l.trim() == name);
        if has_table("[workspace]") {
            // A package under an `exclude`d path is not a member: let cargo
            // find its real workspace.
            let excluded = nearest_package.as_deref().is_some_and(|p: &Path| {
                p.strip_prefix(dir).is_ok_and(|rel| {
                    manifest
                        .lines()
                        .filter(|l| l.trim_start().starts_with("exclude"))
                        .flat_map(|l| l.split('"').skip(1).step_by(2))
                        .any(|ex| rel.starts_with(ex))
                })
            });
            return (!excluded).then(|| dir.to_path_buf());
        }
        if nearest_package.is_none() && has_table("[package]") {
            nearest_package = Some(dir.to_path_buf());
        }
    }
    nearest_package
}

fn locate_with_cargo() -> Result<PathBuf> {
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

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn finds_nearest_workspace_or_package() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        write(&root.join("Cargo.toml"), "[workspace]\nmembers = [\"a\"]\n");
        write(&root.join("a/Cargo.toml"), "[package]\nname = \"a\"\n");
        write(
            &root.join("solo/Cargo.toml"),
            "[package]\nname = \"s\"\n[workspace]\n",
        );
        std::fs::create_dir_all(root.join("a/src")).unwrap();
        assert_eq!(find_workspace(&root.join("a/src")), Some(root.clone()));
        assert_eq!(find_workspace(&root), Some(root.clone()));
        assert_eq!(find_workspace(&root.join("solo")), Some(root.join("solo")));
    }

    #[test]
    fn exclude_lists_defer_to_cargo() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        write(&root.join("Cargo.toml"), "[workspace]\nexclude = [\"x\"]\n");
        write(&root.join("x/Cargo.toml"), "[package]\nname = \"x\"\n");
        write(&root.join("m/Cargo.toml"), "[package]\nname = \"m\"\n");
        assert_eq!(find_workspace(&root.join("x")), None);
        assert_eq!(find_workspace(&root.join("m")), Some(root.clone()));
        assert_eq!(find_workspace(&root), Some(root.clone()));
    }

    #[test]
    fn normalize_removes_dots() {
        assert_eq!(normalize(Path::new("/a/./b/../c")), PathBuf::from("/a/c"));
    }
}
