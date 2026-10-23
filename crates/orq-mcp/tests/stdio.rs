//! Speaks line-delimited JSON-RPC with the server over an in-memory duplex.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;

use serde_json::{json, Value};
use tokio::io::{duplex, AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, ReadHalf};

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

fn fixture(name: &str) -> tempfile::TempDir {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
    let dir = tempfile::tempdir().unwrap();
    copy_dir(&src.join(name), dir.path());
    dir
}

struct Client {
    reader: BufReader<ReadHalf<DuplexStream>>,
    writer: tokio::io::WriteHalf<DuplexStream>,
    next_id: u64,
}

impl Client {
    async fn send(&mut self, msg: Value) {
        let mut line = msg.to_string();
        line.push('\n');
        self.writer.write_all(line.as_bytes()).await.unwrap();
        self.writer.flush().await.unwrap();
    }

    async fn request(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .await;
        loop {
            let mut line = String::new();
            let n = self.reader.read_line(&mut line).await.unwrap();
            assert!(n > 0, "server closed the stream");
            let value: Value = serde_json::from_str(line.trim()).unwrap();
            if value.get("id") == Some(&json!(id)) {
                assert!(value.get("error").is_none(), "error response: {value}");
                return value["result"].clone();
            }
        }
    }

    async fn call(&mut self, name: &str, arguments: Value) -> Value {
        self.request("tools/call", json!({"name": name, "arguments": arguments}))
            .await
    }
}

async fn start(root: &Path) -> (Client, tokio::task::JoinHandle<()>) {
    let (client_io, server_io) = duplex(1 << 20);
    let (sr, sw) = tokio::io::split(server_io);
    let root = root.to_path_buf();
    let handle = tokio::spawn(async move {
        orq_mcp::serve_io(Some(root), sr, sw).await.unwrap();
    });
    let (cr, cw) = tokio::io::split(client_io);
    let mut client = Client {
        reader: BufReader::new(cr),
        writer: cw,
        next_id: 0,
    };
    let init = client
        .request(
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "test", "version": "0"}
            }),
        )
        .await;
    assert!(init["capabilities"]["tools"].is_object());
    client
        .send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
        .await;
    (client, handle)
}

fn text(result: &Value) -> String {
    result["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

#[tokio::test]
async fn lists_the_tools() {
    let dir = fixture("single");
    let (mut client, _server) = start(dir.path()).await;
    let list = client.request("tools/list", json!({})).await;
    let mut names: Vec<&str> = list["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "apply_fix",
            "explain_error",
            "impact",
            "run_affected_tests",
            "verify"
        ]
    );
    for tool in list["tools"].as_array().unwrap() {
        assert!(tool["description"].as_str().unwrap().len() > 40);
        assert_eq!(tool["inputSchema"]["type"], "object");
    }
}

#[tokio::test]
async fn verify_reports_type_error_without_tool_error() {
    let dir = fixture("broken-type");
    let (mut client, _server) = start(dir.path()).await;
    let result = client
        .call("verify", json!({"files": ["src/lib.rs"], "up_to": "check"}))
        .await;
    assert!(text(&result).contains("E0308"), "{result}");
    assert_ne!(result["isError"], json!(true));
    assert!(result["structuredContent"].is_object(), "{result}");
}

#[tokio::test]
async fn explain_error_and_unknown_args() {
    let dir = fixture("single");
    let (mut client, _server) = start(dir.path()).await;
    let result = client.call("explain_error", json!({"code": "E0308"})).await;
    assert!(text(&result).contains("Expected type"), "{result}");
    assert_ne!(result["isError"], json!(true));

    let bad = client.call("explain_error", json!({})).await;
    assert_eq!(bad["isError"], json!(true));
}

#[tokio::test]
async fn impact_lists_dependents() {
    let dir = fixture("multi");
    let (mut client, _server) = start(dir.path()).await;
    let result = client
        .call("impact", json!({"files": ["crates/base/src/lib.rs"]}))
        .await;
    assert_ne!(result["isError"], json!(true), "{result}");
    let structured = &result["structuredContent"];
    assert_eq!(structured["full_rebuild"], json!(false));
    let changed = structured["changed"].as_array().unwrap();
    assert!(changed.iter().any(|c| c == "base"), "{structured}");
    assert!(
        !structured["dependents"].as_array().unwrap().is_empty(),
        "{structured}"
    );
    assert!(text(&result).starts_with("impact:"));
}

#[tokio::test]
async fn build_args_are_in_the_schema() {
    let dir = fixture("single");
    let (mut client, _server) = start(dir.path()).await;
    let list = client.request("tools/list", json!({})).await;
    for tool in list["tools"].as_array().unwrap() {
        let name = tool["name"].as_str().unwrap();
        let props = &tool["inputSchema"]["properties"];
        let has = ["features", "all_features", "no_default_features", "target"]
            .iter()
            .all(|k| props.get(k).is_some());
        assert_eq!(
            has,
            name == "verify" || name == "run_affected_tests",
            "{name}"
        );
    }
}

#[tokio::test]
async fn verify_accepts_build_args() {
    let dir = fixture("multi");
    let (mut client, _server) = start(dir.path()).await;
    let result = client
        .call(
            "verify",
            json!({"files": ["crates/leaf/src/lib.rs"], "up_to": "check", "all_features": true,
                   "no_default_features": true}),
        )
        .await;
    // The runner may or may not honour the features yet; either way the call
    // must be accepted and answer with a report, not an argument error.
    assert!(result["structuredContent"].is_object(), "{result}");
    assert!(!text(&result).contains("invalid arguments"), "{result}");
}

#[tokio::test]
async fn apply_fix_fixes_clippy_lints_and_reports() {
    let dir = fixture("clippy-warn");
    let (mut client, _server) = start(dir.path()).await;
    let result = client
        .call(
            "apply_fix",
            json!({"files": ["src/lib.rs"], "up_to": "clippy"}),
        )
        .await;
    assert_ne!(result["isError"], json!(true), "{result}");
    assert!(
        text(&result).starts_with("fixed 1 file(s): src/lib.rs"),
        "{result}"
    );
    assert_eq!(
        result["structuredContent"]["fixed"],
        json!(["src/lib.rs"]),
        "{result}"
    );
    assert!(result["structuredContent"]["report"].is_object());
}
