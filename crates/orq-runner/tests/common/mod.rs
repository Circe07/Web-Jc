//! Helpers shared by the integration tests of target selection, failing-first
//! and prebuild.

#![allow(dead_code, clippy::expect_used, clippy::unwrap_used)]

use std::path::{Path, PathBuf};

use orq_core::{CrateId, ImpactSet};

/// Path of a fixture in the repository.
pub fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name)
}

/// Copies the sources of a standalone fixture (no `target/`) into `dest`.
pub fn copy_fixture(name: &str, dest: &Path) {
    copy_dir(&fixture(name), dest);
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("mkdir");
    for entry in std::fs::read_dir(from).expect("read_dir") {
        let entry = entry.expect("entry");
        let name = entry.file_name();
        if name == "target" {
            continue;
        }
        let dest = to.join(&name);
        if entry.file_type().expect("type").is_dir() {
            copy_dir(&entry.path(), &dest);
        } else {
            std::fs::copy(entry.path(), dest).expect("copy");
        }
    }
}

/// A crate located at `root/dir`.
pub fn krate(root: &Path, dir: &str, name: &str) -> CrateId {
    CrateId {
        name: name.to_string(),
        manifest_path: root.join(dir).join("Cargo.toml"),
    }
}

/// Impact set made of the single package of a standalone project.
pub fn single(root: &Path, name: &str) -> ImpactSet {
    ImpactSet {
        changed: vec![krate(root, "", name)],
        ..ImpactSet::default()
    }
}

/// Writes an executable `cargo` wrapper in `dir` that appends its arguments
/// (one invocation per line) to the returned log file, then runs the real
/// cargo.
#[cfg(unix)]
pub fn cargo_wrapper(dir: &Path) -> (PathBuf, PathBuf) {
    use std::os::unix::fs::PermissionsExt;

    let log = dir.join("cargo-calls.log");
    let script = dir.join("cargo-wrapper.sh");
    let real = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\necho \"$@\" >> '{}'\nexec '{real}' \"$@\"\n",
            log.display()
        ),
    )
    .expect("write wrapper");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    // Another test thread may have forked while the script was open for
    // writing; until that child execs, running the script fails with
    // ETXTBSY ("Text file busy"). Once one exec succeeds the window is over.
    for attempt in 0.. {
        match std::process::Command::new(&script)
            .arg("--version")
            .output()
        {
            Ok(_) => break,
            Err(e) if e.raw_os_error() == Some(26) && attempt < 100 => {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(e) => panic!("cargo wrapper not runnable: {e}"),
        }
    }
    let _ = std::fs::remove_file(&log);
    (script, log)
}

/// The recorded cargo invocations whose first argument is `sub`.
pub fn calls(log: &Path, sub: &str) -> Vec<String> {
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .filter(|l| l.split_whitespace().next() == Some(sub))
        .map(str::to_string)
        .collect()
}
