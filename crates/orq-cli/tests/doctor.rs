//! Integration tests for `orq doctor`.

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

fn fixture_copy(git_init: bool) -> TempDir {
    let dir = TempDir::new().unwrap();
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/multi");
    copy_dir(&fixture, dir.path());
    std::fs::write(dir.path().join(".gitignore"), "target/\n").unwrap();
    if git_init {
        git(dir.path(), &["init", "-q"]);
        git(dir.path(), &["add", "-A"]);
        git(dir.path(), &["commit", "-q", "-m", "init"]);
    }
    dir
}

#[test]
fn doctor_reports_checks_on_a_fixture() {
    let dir = fixture_copy(true);
    let out = orq(dir.path())
        .args(["doctor", "--json"])
        .assert()
        .get_output()
        .stdout
        .clone();
    let v: Value = serde_json::from_slice(&out).unwrap();
    let checks = v["checks"].as_array().unwrap();
    let level = |name: &str| {
        checks
            .iter()
            .find(|c| c["name"] == name)
            .unwrap_or_else(|| panic!("no check {name}"))["level"]
            .as_str()
            .unwrap()
            .to_string()
    };
    assert_eq!(level("workspace"), "ok");
    assert_eq!(level("git repo"), "ok");
    assert_eq!(level("config"), "ok");
    assert_eq!(level("target dir"), "ok");
    assert_eq!(level("cache"), "ok");
    assert!(["ok", "info"].contains(&level("daemon").as_str()));
    assert!(v["config"]["budget"].is_u64());
    // The toolchain used to run the tests is installed; the exit code
    // mirrors the JSON verdict.
    if v["ok"] == true {
        orq(dir.path()).arg("doctor").assert().success();
    }
}

#[test]
fn doctor_text_output_and_nonzero_exit_without_git_repo() {
    let dir = fixture_copy(false);
    // A directory outside any repository: `git rev-parse` must fail there.
    orq(dir.path())
        .env("GIT_CEILING_DIRECTORIES", dir.path().parent().unwrap())
        .args(["doctor", "--root", "."])
        .assert()
        .code(1)
        .stdout(predicate::str::contains("[fail] git repo"))
        .stdout(predicate::str::contains("workspace"));
}

#[test]
fn doctor_flags_an_unparsable_orq_toml() {
    let dir = fixture_copy(true);
    std::fs::write(dir.path().join("orq.toml"), "budget = \"lots\"\n").unwrap();
    orq(dir.path())
        .arg("doctor")
        .assert()
        .code(1)
        .stdout(predicate::str::contains("[fail] config"));
}
