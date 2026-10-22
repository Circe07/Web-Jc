//! orq-daemon: keeps workspace state warm and answers verification requests
//! over IPC (see PLAN.md T2.2 and `orq_core::ipc`).
//!
//! The daemon watches the workspace (debounced, ignoring `target/` and
//! `.git/`), keeps the cargo metadata in memory, runs a background
//! `cargo check` warm-up after every change and serves one JSON line per
//! connection on the Unix socket given by [`orq_core::ipc::socket_path`].
//!
//! Windows (named pipes) is not implemented yet: [`serve`] returns a clear
//! [`CoreError`] there.

use std::path::Path;
use std::time::Duration;

use orq_core::CoreError;

pub mod client;
#[cfg(unix)]
mod server;

/// Daemon tuning knobs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonOptions {
    /// Shut down after this long without requests (default 30 minutes).
    pub idle_timeout: Duration,
    /// Debounce window for file-system events (default 300 ms).
    pub debounce: Duration,
}

impl Default for DaemonOptions {
    fn default() -> Self {
        Self {
            idle_timeout: Duration::from_secs(30 * 60),
            debounce: Duration::from_millis(300),
        }
    }
}

/// Serves the workspace at `root` in the foreground until a
/// [`orq_core::ipc::DaemonRequest::Stop`] arrives or the idle timeout expires.
///
/// Listens on [`orq_core::ipc::socket_path`] and removes the socket on exit.
/// Fails with an "already running" error if another daemon answers on that
/// socket; a stale socket file is removed.
#[cfg(unix)]
pub async fn serve(root: &Path, options: DaemonOptions) -> Result<(), CoreError> {
    server::serve(root, options).await
}

/// Serves the workspace at `root` in the foreground.
///
/// Not implemented on this platform yet (named pipes are pending): always
/// returns an error.
#[cfg(not(unix))]
pub async fn serve(root: &Path, options: DaemonOptions) -> Result<(), CoreError> {
    let _ = (root, options);
    Err(CoreError::Process {
        command: "orq daemon".into(),
        message: "the daemon is only supported on Unix platforms for now".into(),
    })
}
