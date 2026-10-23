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
/// `$XDG_RUNTIME_DIR/orq-<hash>.sock` when `XDG_RUNTIME_DIR` is set (it is
/// private to the user by definition), otherwise
/// `<temp>/orq-<user>/orq-<hash>.sock`, a directory that
/// [`prepare_socket_dir`] creates with mode `0700`. `<hash>` is 16 hex digits
/// of a stable (FNV-1a) hash of the canonical root, so client and daemon
/// always agree.
pub fn socket_path(root: &Path) -> PathBuf {
    let canonical = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let dir = match std::env::var_os("XDG_RUNTIME_DIR").filter(|d| !d.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => {
            let user = std::env::var("USER")
                .or_else(|_| std::env::var("USERNAME"))
                .unwrap_or_else(|_| "user".into());
            let user: String = user
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
                .collect();
            std::env::temp_dir().join(format!("orq-{user}"))
        }
    };
    dir.join(format!(
        "orq-{:016x}.sock",
        fnv1a(canonical.to_string_lossy().as_bytes())
    ))
}

/// Makes sure the directory of `socket` exists and only the current user can
/// use it. Call it before binding the daemon socket.
///
/// Inside `XDG_RUNTIME_DIR` nothing is checked (the session manager owns
/// it). Otherwise the directory is created with mode `0700`, and an existing
/// one is rejected if it is a symlink, not owned by the current user, or
/// accessible by group or others, since another local user could then
/// connect to the daemon or plant a fake socket.
#[cfg(unix)]
pub fn prepare_socket_dir(socket: &Path) -> Result<(), CoreError> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};

    let Some(dir) = socket.parent() else {
        return Ok(());
    };
    let in_runtime_dir = std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|d| !d.is_empty())
        .is_some_and(|d| dir == Path::new(&d));
    if in_runtime_dir {
        return Ok(());
    }
    match std::fs::DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(CoreError::io(dir, e)),
    }
    let meta = std::fs::symlink_metadata(dir).map_err(|e| CoreError::io(dir, e))?;
    let insecure = |why: &str| {
        CoreError::Config(format!(
            "refusing to use socket directory {}: {why}",
            dir.display()
        ))
    };
    if !meta.file_type().is_dir() {
        return Err(insecure("not a directory (symlink?)"));
    }
    // Owner check without libc: a file we create is owned by our uid.
    static PROBES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let probe = dir.join(format!(
        ".orq-probe-{}-{}",
        std::process::id(),
        PROBES.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::write(&probe, b"").map_err(|e| CoreError::io(&probe, e))?;
    let our_uid = std::fs::metadata(&probe).map(|m| m.uid());
    let _ = std::fs::remove_file(&probe);
    if our_uid.map_err(|e| CoreError::io(&probe, e))? != meta.uid() {
        return Err(insecure("owned by another user"));
    }
    if meta.permissions().mode() & 0o077 != 0 {
        return Err(insecure("accessible by other users (expected mode 0700)"));
    }
    Ok(())
}

/// No-op on platforms without Unix sockets.
#[cfg(not(unix))]
pub fn prepare_socket_dir(_socket: &Path) -> Result<(), CoreError> {
    Ok(())
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
    // Never talk to a socket in a directory another user could control.
    if let Err(e) = prepare_socket_dir(&path) {
        tracing::warn!("ignoring daemon socket: {e}");
        return Ok(None);
    }
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
    fn prepare_socket_dir_creates_private_dir_and_rejects_open_ones() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("orq-test");
        let socket = dir.join("x.sock");
        prepare_socket_dir(&socket).unwrap();
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
        prepare_socket_dir(&socket).unwrap();

        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o777)).unwrap();
        assert!(prepare_socket_dir(&socket).is_err());

        let link = tmp.path().join("orq-link");
        std::os::unix::fs::symlink(&dir, &link).unwrap();
        assert!(prepare_socket_dir(&link.join("x.sock")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn request_without_daemon_is_none() {
        let dir = std::env::temp_dir().join("orq-core-ipc-no-daemon");
        let response = request(&dir, &DaemonRequest::Status).unwrap();
        assert!(response.is_none());
    }
}
