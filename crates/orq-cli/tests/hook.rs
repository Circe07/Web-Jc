//! Integration tests for `orq hook` (Claude Code PostToolUse hook).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::time::{Duration, Instant};

use assert_cmd::Command;
use serde_json::{json, Value};
use tempfile::TempDir;

fn fixture(name: &str) -> std::path::PathBuf {
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

fn copy_fixture(name: &str) -> (TempDir, std::path::PathBuf) {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join(name);
    copy_dir(&fixture(name), &root);
    let root = root.canonicalize().unwrap();
    (tmp, root)
}

fn payload(cwd: &Path, tool: &str, file: &Path) -> String {
    json!({
        "session_id": "t",
        "cwd": cwd,
        "hook_event_name": "PostToolUse",
        "tool_name": tool,
        "tool_input": { "file_path": file },
        "tool_response": { "filePath": file },
    })
    .to_string()
}

fn hook(cwd: &Path) -> Command {
    let mut cmd = Command::cargo_bin("orq").unwrap();
    cmd.arg("hook").current_dir(cwd).env("ORQ_NO_DAEMON", "1");
    cmd
}

#[test]
fn non_rust_edit_is_silent_and_fast() {
    let tmp = TempDir::new().unwrap();
    let file = tmp.path().join("README.md");
    let started = Instant::now();
    let out = hook(tmp.path())
        .write_stdin(payload(tmp.path(), "Edit", &file))
        .assert()
        .success()
        .get_output()
        .clone();
    assert!(out.stdout.is_empty(), "stdout: {:?}", out.stdout);
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "took {:?}",
        started.elapsed()
    );
}

#[test]
fn rust_file_outside_a_workspace_is_silent() {
    let tmp = TempDir::new().unwrap();
    let file = tmp.path().join("lonely.rs");
    std::fs::write(&file, "fn main() {}\n").unwrap();
    let out = hook(tmp.path())
        .write_stdin(payload(tmp.path(), "Write", &file))
        .assert()
        .success()
        .get_output()
        .clone();
    assert!(out.stdout.is_empty());
}

#[test]
fn broken_edit_returns_blocking_feedback() {
    let (_tmp, root) = copy_fixture("broken-type");
    let file = root.join("src/lib.rs");
    let out = hook(&root)
        .write_stdin(payload(&root, "Edit", &file))
        .assert()
        .success()
        .get_output()
        .clone();
    let json: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["decision"], "block");
    let reason = json["reason"].as_str().unwrap();
    assert!(reason.contains("E0308"), "reason: {reason}");
    // Recorded with source = "hook".
    let stats = std::fs::read_to_string(root.join("target/orq/stats.jsonl")).unwrap();
    assert!(stats.contains("\"source\":\"hook\""), "stats: {stats}");
}

#[test]
fn clean_edit_is_silent() {
    let (_tmp, root) = copy_fixture("multi");
    let file = root.join("crates/base/src/lib.rs");
    assert!(file.is_file());
    // cwd elsewhere: the hook must locate the workspace from the file path.
    let out = hook(&root)
        .write_stdin(payload(&root, "MultiEdit", &file))
        .assert()
        .success()
        .get_output()
        .clone();
    assert!(
        out.stdout.is_empty(),
        "stdout: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn malformed_stdin_does_not_block() {
    let tmp = TempDir::new().unwrap();
    for input in ["", "not json", "[1,2", "{\"tool_input\": 3}"] {
        let out = hook(tmp.path())
            .write_stdin(input)
            .assert()
            .success()
            .get_output()
            .clone();
        assert!(out.stdout.is_empty(), "input {input:?}");
    }
}

#[test]
fn hook_without_fix_never_edits_files() {
    let (_tmp, root) = copy_fixture("clippy-warn");
    let file = root.join("src/lib.rs");
    let before = std::fs::read_to_string(&file).unwrap();
    let out = hook(&root)
        .env_remove("ORQ_HOOK_FIX")
        .write_stdin(payload(&root, "Edit", &file))
        .assert()
        .success()
        .get_output()
        .clone();
    assert!(out.stdout.is_empty());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), before);
}

#[test]
fn hook_with_fix_edits_and_tells_the_agent() {
    let (_tmp, root) = copy_fixture("clippy-warn");
    let file = root.join("src/lib.rs");
    let out = hook(&root)
        .arg("--fix")
        .write_stdin(payload(&root, "Edit", &file))
        .assert()
        .success()
        .get_output()
        .clone();
    let json: Value = serde_json::from_slice(&out.stdout).unwrap();
    let ctx = json["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert_eq!(json["hookSpecificOutput"]["hookEventName"], "PostToolUse");
    assert!(ctx.contains("fixed: 1 file (src/lib.rs)"), "{ctx}");
    // A pass is not a block.
    assert!(json.get("decision").is_none());
    let source = std::fs::read_to_string(&file).unwrap();
    assert!(!source.contains("return v.len() == 0"), "{source}");
}

#[test]
fn hook_fix_can_be_enabled_with_an_environment_variable() {
    let (_tmp, root) = copy_fixture("clippy-warn");
    let file = root.join("src/lib.rs");
    let out = hook(&root)
        .env("ORQ_HOOK_FIX", "1")
        .write_stdin(payload(&root, "Edit", &file))
        .assert()
        .success()
        .get_output()
        .clone();
    let json: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(json["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap()
        .contains("fixed:"));
}

#[test]
fn hook_with_fix_on_clean_code_stays_silent() {
    let (_tmp, root) = copy_fixture("multi");
    let file = root.join("crates/base/src/lib.rs");
    let out = hook(&root)
        .arg("--fix")
        .write_stdin(payload(&root, "Edit", &file))
        .assert()
        .success()
        .get_output()
        .clone();
    assert!(out.stdout.is_empty());
}
