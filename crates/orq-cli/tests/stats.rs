//! Integration tests for `orq stats`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::process::Command as Std;

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::Value;
use tempfile::TempDir;

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

fn orq(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("orq").unwrap();
    cmd.current_dir(dir)
        .env("ORQ_NO_DAEMON", "1")
        .env_remove("CARGO_TARGET_DIR");
    cmd
}

#[test]
fn empty_log_prints_message() {
    let dir = TempDir::new().unwrap();
    std::fs::write(
        dir.path().join("Cargo.toml"),
        "[workspace]\nmembers = []\nresolver = \"2\"\n",
    )
    .unwrap();
    orq(dir.path())
        .args(["stats", "--root", "."])
        .assert()
        .success()
        .stdout(predicate::str::contains("no verifications recorded yet"));
}

#[test]
fn two_verifies_then_stats() {
    let dir = TempDir::new().unwrap();
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/multi");
    copy_dir(&fixture, dir.path());
    std::fs::write(dir.path().join(".gitignore"), "target/\n").unwrap();
    git(dir.path(), &["init", "-q"]);
    git(dir.path(), &["add", "-A"]);
    git(dir.path(), &["commit", "-q", "-m", "init"]);

    for _ in 0..2 {
        orq(dir.path())
            .args(["verify", "--no-daemon", "--files", "crates/leaf/src/lib.rs"])
            .assert()
            .success();
    }

    let out = orq(dir.path())
        .args(["stats", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(v["count"], 2);
    assert_eq!(v["pass"], 2);
    assert!((v["cache_hit_rate"].as_f64().unwrap() - 0.5).abs() < 1e-9);
    assert_eq!(v["by_cache"]["hit"]["count"], 1);
    assert_eq!(v["savings"]["estimate"], true);
    assert_eq!(v["savings"]["hits_counted"], 1);
    let raw = v["savings"]["raw_tokens"].as_u64().unwrap();
    let sent = v["savings"]["report_tokens"].as_u64().unwrap();
    assert!(raw > sent, "raw {raw} should exceed sent {sent}");
    assert_eq!(v["savings"]["tokens_saved"].as_u64().unwrap(), raw - sent);

    orq(dir.path())
        .args(["stats"])
        .assert()
        .success()
        .stdout(predicate::str::contains("savings (estimate)"));

    orq(dir.path())
        .args(["stats", "--last", "1"])
        .assert()
        .success()
        .stdout(predicate::str::contains("verifications  1 "));
}
