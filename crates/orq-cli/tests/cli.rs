//! Integration tests for the `orq` binary over the repo fixtures.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::process::Command as Std;
use std::time::{Duration, Instant};

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::Value;
use tempfile::TempDir;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name)
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name();
        if name == "target" {
            continue;
        }
        let dest = to.join(&name);
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &dest);
        } else {
            std::fs::copy(entry.path(), dest).unwrap();
        }
    }
}

fn git(dir: &Path, args: &[&str]) {
    let status = Std::new("git")
        .current_dir(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c"])
        .arg("commit.gpgsign=false")
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?} failed");
}

/// Copies a fixture into a temp dir and commits it in a fresh git repo.
fn project(name: &str) -> TempDir {
    let dir = TempDir::new().unwrap();
    copy_dir(&fixture(name), dir.path());
    std::fs::write(dir.path().join(".gitignore"), "target/\n").unwrap();
    git(dir.path(), &["init", "-q"]);
    git(dir.path(), &["add", "-A"]);
    git(dir.path(), &["commit", "-q", "-m", "init"]);
    dir
}

fn orq(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("orq").unwrap();
    cmd.current_dir(dir)
        .env("ORQ_NO_DAEMON", "1")
        .env_remove("CARGO_TARGET_DIR");
    cmd
}

fn names(value: &Value) -> Vec<String> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect()
}

#[test]
fn multi_only_leaf_changed_then_cache_hit() {
    let dir = project("multi");
    let leaf = dir.path().join("crates/leaf/src/lib.rs");
    let mut src = std::fs::read_to_string(&leaf).unwrap();
    src.push_str("\n/// Added by the test.\npub fn extra() -> u32 {\n    1\n}\n");
    std::fs::write(&leaf, src).unwrap();

    let out = orq(dir.path())
        .args(["verify", "--json", "--no-daemon"])
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(names(&json["impact"]["changed"]), ["leaf"]);
    assert!(names(&json["impact"]["dependents"]).is_empty());
    assert_eq!(json["cache_hit"], Value::Bool(false));
    let text = String::from_utf8_lossy(&out);
    assert!(
        !text.contains("\"base\"") && !text.contains("\"app\""),
        "{text}"
    );

    // Second run, nothing changed: served from the cache, and fast.
    // The 200 ms budget includes process spawn plus `cargo locate-project`.
    let start = Instant::now();
    orq(dir.path())
        .args(["verify", "--no-daemon"])
        .assert()
        .code(0)
        .stdout(predicate::str::contains("(cache: yes)"));
    let elapsed = start.elapsed();
    assert!(
        elapsed < Duration::from_millis(200),
        "cached run took {elapsed:?}"
    );
}

#[test]
fn broken_type_fails_with_e0308() {
    let dir = project("broken-type");
    orq(dir.path())
        .args(["verify", "--no-daemon", "--files", "src/lib.rs"])
        .assert()
        .code(1)
        .stdout(predicate::str::contains("E0308"));
}

#[test]
fn up_to_check_skips_failing_test() {
    let dir = project("failing-test");
    orq(dir.path())
        .args(["verify", "--no-daemon", "--up-to", "check"])
        .args(["--files", "src/lib.rs"])
        .assert()
        .code(0);
}

#[test]
fn explain_known_code_within_budget() {
    let out = Command::cargo_bin("orq")
        .unwrap()
        .args(["explain", "E0308", "--budget", "100"])
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();
    let text = String::from_utf8_lossy(&out);
    assert!(!text.is_empty());
    assert!(orq_core::estimate_tokens(&text) <= 101, "{text}");
}

#[test]
fn explain_unknown_code_is_internal_error() {
    Command::cargo_bin("orq")
        .unwrap()
        .args(["explain", "E9999"])
        .assert()
        .code(2);
}

#[test]
fn daemon_status_without_daemon() {
    let dir = project("single");
    orq(dir.path())
        .args(["daemon", "status"])
        .assert()
        .code(1)
        .stdout(predicate::str::contains("not running"));
}

#[test]
fn verify_rejects_unknown_step() {
    Command::cargo_bin("orq")
        .unwrap()
        .args(["verify", "--up-to", "bogus"])
        .assert()
        .failure();
}

#[test]
fn build_flags_reach_the_request() {
    let dir = project("multi");
    let run = |extra: &[&str]| -> Value {
        let out = orq(dir.path())
            .args([
                "verify",
                "--json",
                "--no-daemon",
                "--no-cache",
                "--up-to",
                "check",
                "--files",
                "crates/leaf/src/lib.rs",
            ])
            .args(extra)
            .assert()
            .get_output()
            .stdout
            .clone();
        serde_json::from_slice(&out).unwrap()
    };
    let default = run(&[]);
    assert_eq!(default["build"]["all_features"], Value::Bool(false));
    assert_eq!(default["build"]["target"], Value::Null);
    let flagged = run(&["--all-features", "--no-default-features"]);
    assert_eq!(flagged["build"]["all_features"], Value::Bool(true));
    assert_eq!(flagged["build"]["no_default_features"], Value::Bool(true));
}

#[test]
fn verify_accepts_build_flags() {
    Command::cargo_bin("orq")
        .unwrap()
        .args(["verify", "--help"])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("--features")
                .and(predicate::str::contains("--all-features"))
                .and(predicate::str::contains("--no-default-features"))
                .and(predicate::str::contains("--target")),
        );
}

/// Stops the daemon (and the MCP server) even when the test fails.
#[cfg(unix)]
struct Cleanup {
    root: PathBuf,
    mcp: std::process::Child,
}

#[cfg(unix)]
impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = self.mcp.kill();
        let _ = self.mcp.wait();
        let _ = Std::new(assert_cmd::cargo::cargo_bin("orq"))
            .args(["daemon", "stop", "--root"])
            .arg(&self.root)
            .status();
    }
}

#[cfg(unix)]
#[test]
fn mcp_autostarts_the_daemon() {
    let dir = project("multi");
    let root = dir.path().canonicalize().unwrap();
    let bin = assert_cmd::cargo::cargo_bin("orq");
    let mcp = Std::new(&bin)
        .args(["mcp", "--root"])
        .arg(&root)
        .current_dir(&root)
        .env_remove("ORQ_NO_DAEMON")
        .env_remove("CARGO_TARGET_DIR")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let _cleanup = Cleanup {
        root: root.clone(),
        mcp,
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let out = Std::new(&bin)
            .args(["daemon", "status", "--root"])
            .arg(&root)
            .output()
            .unwrap();
        if out.status.success() && String::from_utf8_lossy(&out.stdout).contains("running") {
            break;
        }
        assert!(Instant::now() < deadline, "daemon never came up");
        std::thread::sleep(Duration::from_millis(100));
    }
}
