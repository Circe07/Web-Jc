//! Daemon IPC protocol: one JSON line per request and one per response over a
//! Unix socket (named pipes on Windows are not implemented yet).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{CoreError, VerifyRequest, VerifyResult};

/// A request sent to the daemon.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum DaemonRequest {
    /// Verify; answered with [`DaemonResponse::Verify`].
    Verify(VerifyRequest),
    /// Health check; answered with [`DaemonResponse::Status`].
    Status,
    /// Shut down; answered with [`DaemonResponse::Stopping`].
    Stop,
}

/// The daemon's answer to a [`DaemonRequest`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum DaemonResponse {
    /// Result of a verification.
    Verify(VerifyResult),
    /// Daemon state.
    Status(DaemonStatus),
    /// The daemon is shutting down.
    Stopping,
    /// The request failed.
    Error {
        /// What went wrong.
        message: String,
    },
}

/// State reported by [`DaemonRequest::Status`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DaemonStatus {
    /// Workspace the daemon serves.
    pub root: PathBuf,
    /// Process id.
    pub pid: u32,
    /// Milliseconds since start.
    pub uptime_ms: u64,
    /// Requests served.
    pub requests: u64,
    /// `true` while a background warm-up `cargo check` is running.
    pub warming: bool,
}

/// Socket path of the daemon for `root`:
/// `$XDG_RUNTIME_DIR/orq-<hash>.sock`, or the system temp dir when
/// `XDG_RUNTIME_DIR` is unset. `<hash>` is 16 hex digits of a stable (FNV-1a)
/// hash of the canonical root, so client and daemon always agree.
pub fn socket_path(root: &Path) -> PathBuf {
    let canonical = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    dir.join(format!(
        "orq-{:016x}.sock",
        fnv1a(canonical.to_string_lossy().as_bytes())
    ))
}

fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, b| {
        (hash ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// Sends `request` to the daemon serving `root` and waits for its answer.
///
/// Returns `Ok(None)` when no daemon is listening (no socket, or a stale one),
/// so callers can fall back to running in-process.
#[cfg(unix)]
pub fn request(root: &Path, request: &DaemonRequest) -> Result<Option<DaemonResponse>, CoreError> {
    use std::io::{BufRead, BufReader, ErrorKind, Write};
    use std::os::unix::net::UnixStream;

    let path = socket_path(root);
    let mut stream = match UnixStream::connect(&path) {
        Ok(stream) => stream,
        Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::ConnectionRefused) => {
            return Ok(None)
        }
        Err(e) => return Err(CoreError::io(path, e)),
    };
    let mut line = serde_json::to_string(request).map_err(|e| CoreError::Parse(e.to_string()))?;
    line.push('\n');
    stream
        .write_all(line.as_bytes())
        .map_err(|e| CoreError::io(&path, e))?;

    let mut answer = String::new();
    BufReader::new(stream)
        .read_line(&mut answer)
        .map_err(|e| CoreError::io(&path, e))?;
    if answer.trim().is_empty() {
        return Err(CoreError::Parse("daemon closed the connection".into()));
    }
    serde_json::from_str(&answer)
        .map(Some)
        .map_err(|e| CoreError::Parse(format!("bad daemon response: {e}")))
}

/// Sends `request` to the daemon serving `root`.
///
/// Not implemented on this platform yet: always reports that no daemon is
/// running.
#[cfg(not(unix))]
pub fn request(
    _root: &Path,
    _request: &DaemonRequest,
) -> Result<Option<DaemonResponse>, CoreError> {
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_json_shape() {
        assert_eq!(
            serde_json::to_value(DaemonRequest::Status).unwrap(),
            serde_json::json!({ "type": "status" })
        );
        let verify = DaemonRequest::Verify(VerifyRequest::default());
        let json = serde_json::to_string(&verify).unwrap();
        assert!(json.starts_with(r#"{"type":"verify","data":{"#), "{json}");
        assert_eq!(
            serde_json::from_str::<DaemonRequest>(&json).unwrap(),
            verify
        );
        let error = DaemonResponse::Error {
            message: "x".into(),
        };
        let back: DaemonResponse =
            serde_json::from_str(&serde_json::to_string(&error).unwrap()).unwrap();
        assert_eq!(back, error);
    }

    #[test]
    fn socket_path_is_stable_and_per_root() {
        let a = socket_path(Path::new("/definitely/not/a/real/root-a"));
        let b = socket_path(Path::new("/definitely/not/a/real/root-b"));
        assert_eq!(a, socket_path(Path::new("/definitely/not/a/real/root-a")));
        assert_ne!(a, b);
        let name = a.file_name().unwrap().to_string_lossy().into_owned();
        assert!(
            name.starts_with("orq-") && name.ends_with(".sock"),
            "{name}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn request_without_daemon_is_none() {
        let dir = std::env::temp_dir().join("orq-core-ipc-no-daemon");
        let response = request(&dir, &DaemonRequest::Status).unwrap();
        assert!(response.is_none());
    }
}
