//! Integration tests for `orq fix`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;

use assert_cmd::Command;
use predicates::prelude::*;
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

fn fixture(name: &str) -> TempDir {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
    let dir = tempfile::tempdir().unwrap();
    copy_dir(&src.join(name), dir.path());
    dir
}

fn orq(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("orq").unwrap();
    cmd.current_dir(dir)
        .env("ORQ_NO_DAEMON", "1")
        .env_remove("CARGO_TARGET_DIR");
    cmd
}

#[test]
fn fix_applies_suggestions_then_verifies() {
    let dir = fixture("clippy-warn");
    orq(dir.path())
        .args(["fix", "--files", "src/lib.rs", "--up-to", "clippy"])
        .assert()
        .stdout(predicate::str::starts_with("fixed 1 file(s): src/lib.rs"));
    let source = std::fs::read_to_string(dir.path().join("src/lib.rs")).unwrap();
    assert!(!source.contains("return v.len() == 0"), "{source}");
}

#[test]
fn fix_on_clean_workspace_fixes_nothing_and_passes() {
    let dir = fixture("multi");
    orq(dir.path())
        .args([
            "fix",
            "--files",
            "crates/base/src/lib.rs",
            "--up-to",
            "check",
        ])
        .assert()
        .success()
        .stdout(predicate::str::starts_with("fixed 0 file(s)"));
}

#[test]
fn fix_json_has_fixed_and_report() {
    let dir = fixture("multi");
    let out = orq(dir.path())
        .args([
            "fix",
            "--json",
            "--files",
            "crates/base/src/lib.rs",
            "--up-to",
            "check",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(json["fixed"], serde_json::json!([]));
    assert!(json["report"].is_object());
}
