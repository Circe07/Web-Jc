//! orq-daemon: keeps workspace state warm and answers verification requests
//! over IPC (see PLAN.md T2.2 and `orq_core::ipc`).

use std::path::Path;
use std::time::Duration;

use orq_core::CoreError;

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
pub async fn serve(root: &Path, options: DaemonOptions) -> Result<(), CoreError> {
    let _ = (root, options);
    Err(CoreError::Process {
        command: "orq daemon".into(),
        message: "not implemented yet (PLAN.md T2.2)".into(),
    })
}
