//! Toolchain fingerprint: decides when a memoized `rustc -Vv` may be reused.
//!
//! `rustc -Vv` costs about 70 ms per process, so its output is memoized in
//! the database under a *toolchain fingerprint*: a BLAKE3 hash of everything
//! that can change which `rustc` runs (or what it reports). Whenever any part
//! of the fingerprint differs, `rustc -Vv` is run again. The fingerprint
//! covers, in order:
//!
//! * the program name (`RUSTC`, default `rustc`);
//! * the values of `RUSTC`, `RUSTUP_TOOLCHAIN`, `RUSTUP_HOME`, `HOME` and
//!   `PATH`, both as seen by the configured environment source and by the
//!   process (the process environment is what the spawned `rustc` sees);
//! * every `rust-toolchain` / `rust-toolchain.toml` in `root` and in each of
//!   its ancestors up to the filesystem root (contents, or absence), as
//!   rustup resolves overrides;
//! * the path, size and mtime of the `rustc` the program resolves to (PATH is
//!   searched here, symlinks are followed), plus its canonical path;
//! * `$RUSTUP_HOME/settings.toml` (contents and mtime), the mtime of
//!   `$RUSTUP_HOME/toolchains` and `$RUSTUP_HOME/update-hashes`, and for each
//!   entry of those directories its name and mtime; for each installed
//!   toolchain also size and mtime of `bin/rustc` and the mtime of `lib`. So
//!   `rustup update`, `rustup default` and `rustup override` all invalidate it.
//!
//! `RUSTUP_HOME` defaults to `~/.rustup`.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// Looks up a variable: configured source first, then the process.
fn var(env: &[(String, String)], name: &str) -> Option<String> {
    env.iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.clone())
        .or_else(|| std::env::var(name).ok())
}

fn field(h: &mut blake3::Hasher, bytes: &[u8]) {
    h.update(&(bytes.len() as u64).to_le_bytes());
    h.update(bytes);
}

/// Mixes size and mtime (ns) of `path` (following symlinks) into `h`.
fn stat_into(h: &mut blake3::Hasher, path: &Path) {
    match std::fs::metadata(path) {
        Ok(m) => {
            let mtime = m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_nanos());
            h.update(&[1]);
            h.update(&m.len().to_le_bytes());
            h.update(&mtime.to_le_bytes());
        }
        Err(_) => {
            h.update(&[0]);
        }
    }
}

/// Mixes the mtime of directory `dir` and of each of its entries.
fn dir_into(h: &mut blake3::Hasher, dir: &Path) {
    field(h, dir.to_string_lossy().as_bytes());
    stat_into(h, dir);
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut names: Vec<OsString> = rd.filter_map(|e| e.ok().map(|e| e.file_name())).collect();
    names.sort();
    for n in names {
        field(h, n.to_string_lossy().as_bytes());
        stat_into(h, &dir.join(n));
    }
}

/// Resolves `program` like the OS would: used as is if it has a separator,
/// otherwise searched in `path`.
fn resolve(program: &str, path: Option<&str>) -> Option<PathBuf> {
    if program.contains('/') || program.contains(std::path::MAIN_SEPARATOR) {
        return Some(PathBuf::from(program));
    }
    std::env::split_paths(path?)
        .map(|d| d.join(program))
        .find(|c| c.is_file())
}

/// Computes the toolchain fingerprint (see the module docs).
pub(crate) fn fingerprint(root: &Path, program: &str, env: &[(String, String)]) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    field(&mut h, b"orq-toolchain-fp-v1");
    field(&mut h, program.as_bytes());
    for name in ["RUSTC", "RUSTUP_TOOLCHAIN", "RUSTUP_HOME", "HOME", "PATH"] {
        for v in [
            env.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone()),
            std::env::var(name).ok(),
        ] {
            field(&mut h, name.as_bytes());
            match v {
                Some(v) => {
                    h.update(&[1]);
                    field(&mut h, v.as_bytes());
                }
                None => {
                    h.update(&[0]);
                }
            }
        }
    }

    // rustup override files, from `root` up to the filesystem root.
    let start = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    for dir in start.ancestors() {
        for name in ["rust-toolchain", "rust-toolchain.toml"] {
            let p = dir.join(name);
            field(&mut h, p.to_string_lossy().as_bytes());
            match std::fs::read(&p) {
                Ok(bytes) => {
                    h.update(&[1]);
                    field(&mut h, &bytes);
                }
                Err(_) => {
                    h.update(&[0]);
                }
            }
        }
    }

    // The rustc binary itself.
    let path_var = var(env, "PATH");
    match resolve(program, path_var.as_deref()) {
        Some(p) => {
            h.update(&[1]);
            field(&mut h, p.to_string_lossy().as_bytes());
            if let Ok(c) = p.canonicalize() {
                field(&mut h, c.to_string_lossy().as_bytes());
            }
            stat_into(&mut h, &p);
        }
        None => {
            h.update(&[0]);
        }
    }

    // rustup state.
    let rustup_home = var(env, "RUSTUP_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| var(env, "HOME").map(|home| PathBuf::from(home).join(".rustup")));
    if let Some(home) = rustup_home {
        let settings = home.join("settings.toml");
        stat_into(&mut h, &settings);
        if let Ok(bytes) = std::fs::read(&settings) {
            field(&mut h, &bytes);
        }
        let toolchains = home.join("toolchains");
        dir_into(&mut h, &toolchains);
        dir_into(&mut h, &home.join("update-hashes"));
        if let Ok(rd) = std::fs::read_dir(&toolchains) {
            let mut dirs: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
            dirs.sort();
            for d in dirs {
                stat_into(&mut h, &d.join("bin").join("rustc"));
                stat_into(&mut h, &d.join("lib"));
            }
        }
    }
    *h.finalize().as_bytes()
}
