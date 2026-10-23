//! Skipping the rustup proxies.
//!
//! With rustup, `cargo`, `cargo-clippy`, `cargo-fmt` and `rustc` on `PATH`
//! are proxies that resolve the toolchain (env, `rust-toolchain.toml`,
//! overrides) on every call: about 35 ms each, and one verify makes around
//! ten such calls. Resolving the toolchain once (`rustup which cargo`, run in
//! the workspace so overrides apply) and putting its `bin/` first on `PATH`
//! lets every child call the real binaries directly.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use tokio::process::Command;

/// Toolchain `bin/` dir per workspace root; `None` when rustup is not in use
/// or could not answer. Resolved once per process (the daemon keeps it).
fn cache() -> &'static Mutex<HashMap<PathBuf, Option<PathBuf>>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, Option<PathBuf>>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// The `bin/` dir of the toolchain rustup picks for `root`, or `None` to keep
/// using the proxies (no rustup, an error, or a `RUSTUP_TOOLCHAIN`-less
/// setup rustup cannot resolve).
pub(crate) async fn bin_dir(root: &Path) -> Option<PathBuf> {
    if let Ok(cache) = cache().lock() {
        if let Some(hit) = cache.get(root) {
            return hit.clone();
        }
    }
    let resolved = resolve(root).await;
    if let Ok(mut cache) = cache().lock() {
        cache.insert(root.to_path_buf(), resolved.clone());
    }
    resolved
}

async fn resolve(root: &Path) -> Option<PathBuf> {
    let out = Command::new("rustup")
        .args(["which", "cargo"])
        .current_dir(root)
        .kill_on_drop(true)
        .output()
        .await
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let cargo = PathBuf::from(String::from_utf8(out.stdout).ok()?.trim());
    let dir = cargo.parent()?.to_path_buf();
    (cargo.is_file() && dir.is_absolute()).then_some(dir)
}

/// `PATH` with `dir` in front.
pub(crate) fn path_with(dir: &Path) -> Option<OsString> {
    let current = std::env::var_os("PATH").unwrap_or_default();
    let mut parts = vec![dir.to_path_buf()];
    parts.extend(std::env::split_paths(&current));
    std::env::join_paths(parts).ok()
}

/// `true` if an executable named `name` (`name.exe` on Windows) is on `PATH`.
pub(crate) fn on_path(name: &str) -> bool {
    let file = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    };
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join(&file).is_file()))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_with_puts_the_dir_first() {
        let dir = PathBuf::from("/opt/toolchain/bin");
        let path = path_with(&dir).unwrap();
        assert_eq!(std::env::split_paths(&path).next(), Some(dir));
    }

    #[test]
    fn missing_programs_are_not_on_path() {
        assert!(!on_path("orq-no-such-program-xyz"));
    }

    #[tokio::test]
    async fn resolution_is_cached_per_root() {
        let dir = tempfile::tempdir().unwrap();
        let first = bin_dir(dir.path()).await;
        assert_eq!(cache().lock().unwrap().get(dir.path()), Some(&first));
        assert_eq!(bin_dir(dir.path()).await, first);
    }
}
