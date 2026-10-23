//! `orq mcp` spoken to over real stdio (line-delimited JSON-RPC).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::{json, Value};

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

#[test]
fn mcp_binary_serves_verify_over_stdio() {
    let dir = tempfile::tempdir().unwrap();
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
    copy_dir(&fixtures.join("broken-type"), dir.path());

    let bin = assert_cmd::cargo::cargo_bin("orq");
    let mut child = Command::new(bin)
        .args(["mcp", "--root"])
        .arg(dir.path())
        .env("ORQ_NO_DAEMON", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());

    let mut request = |msg: Value, expect_reply: bool| -> Value {
        writeln!(stdin, "{msg}").unwrap();
        stdin.flush().unwrap();
        if !expect_reply {
            return Value::Null;
        }
        let mut line = String::new();
        assert!(stdout.read_line(&mut line).unwrap() > 0, "server exited");
        serde_json::from_str(line.trim()).unwrap()
    };

    let init = request(
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": {"name": "test", "version": "0"}
        }}),
        true,
    );
    assert!(init["result"]["capabilities"]["tools"].is_object());
    request(
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        false,
    );
    let list = request(
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}}),
        true,
    );
    assert_eq!(list["result"]["tools"].as_array().unwrap().len(), 5);
    let call = request(
        json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {
            "name": "verify",
            "arguments": {"files": ["src/lib.rs"], "up_to": "check"}
        }}),
        true,
    );
    let text = call["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("E0308"), "{call}");
    assert_ne!(call["result"]["isError"], json!(true));

    drop(stdin);
    let status = child.wait().unwrap();
    assert!(status.success());
}
