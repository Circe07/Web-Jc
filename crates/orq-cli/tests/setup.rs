//! Integration tests for `orq setup`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::process::Command;

use assert_cmd::Command as Cmd;
use predicates::prelude::*;

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let dest = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            if entry.file_name() != "target" {
                copy_dir(&entry.path(), &dest);
            }
        } else {
            std::fs::copy(entry.path(), dest).unwrap();
        }
    }
}

fn fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/single");
    copy_dir(&src, dir.path());
    dir
}

fn orq(dir: &Path) -> Cmd {
    let mut c = Cmd::cargo_bin("orq").unwrap();
    c.arg("setup").arg("--root").arg(dir);
    c
}

#[test]
fn dry_run_prints_diff_and_writes_nothing() {
    let dir = fixture();
    orq(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("+[profile.dev]"))
        .stdout(predicate::str::contains("Dry run"));
    assert!(!dir.path().join(".cargo/config.toml").exists());
}

#[test]
fn apply_writes_config_and_cargo_check_works() {
    let dir = fixture();
    orq(dir.path()).arg("--apply").assert().success();
    let cfg = std::fs::read_to_string(dir.path().join(".cargo/config.toml")).unwrap();
    assert!(cfg.contains("# >>> orq setup >>>"));
    assert!(cfg.contains("line-tables-only"));

    let status = Command::new("cargo")
        .args(["check", "--offline"])
        .current_dir(dir.path())
        .status()
        .unwrap();
    assert!(status.success());

    // Second apply is a no-op: file unchanged, no backup created.
    orq(dir.path())
        .arg("--apply")
        .assert()
        .success()
        .stdout(predicate::str::contains("up to date"));
    assert_eq!(
        std::fs::read_to_string(dir.path().join(".cargo/config.toml")).unwrap(),
        cfg
    );
    assert!(!dir.path().join(".cargo/config.toml.orq-backup").exists());
}

#[test]
fn apply_refuses_when_user_owns_profile_dev() {
    let dir = fixture();
    std::fs::create_dir_all(dir.path().join(".cargo")).unwrap();
    let cfg = dir.path().join(".cargo/config.toml");
    std::fs::write(&cfg, "[profile.dev]\nopt-level = 0\n").unwrap();
    orq(dir.path())
        .arg("--apply")
        .assert()
        .code(2)
        .stdout(predicate::str::contains("Add these by hand"));
    assert_eq!(
        std::fs::read_to_string(&cfg).unwrap(),
        "[profile.dev]\nopt-level = 0\n"
    );
}
