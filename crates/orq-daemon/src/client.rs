//! Client side, shared by the CLI and the MCP server: verify through the
//! daemon when one is running, otherwise in-process under the workspace lock.

use std::fs::{File, OpenOptions};
use std::path::Path;

use orq_cache::RedbCache;
use orq_compress::LlmCompressor;
use orq_core::ipc::{self, DaemonRequest, DaemonResponse};
use orq_core::{root, Config, CoreError, Pipeline, VerifyRequest, VerifyResult};
use orq_impact::CargoImpactAnalyzer;
use orq_runner::Runner;

/// Whether [`verify`] may delegate to a daemon.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// Use the daemon if one answers, else run in-process.
    #[default]
    Auto,
    /// Never talk to a daemon.
    InProcess,
}

/// Result of [`verify`].
#[derive(Debug, Clone)]
pub struct Verified {
    /// Full verification result.
    pub result: VerifyResult,
    /// `true` if a daemon produced it.
    pub via_daemon: bool,
}

/// Verifies `request` (its `root` must be the workspace root).
///
/// Daemon problems are logged and fall back to in-process mode. In-process
/// runs hold an exclusive lock on `<target>/orq/lock` (PLAN.md §7.2).
pub async fn verify(
    request: &VerifyRequest,
    config: &Config,
    mode: Mode,
) -> Result<Verified, CoreError> {
    if mode == Mode::Auto {
        if let Some(result) = delegate(request).await {
            return Ok(Verified {
                result,
                via_daemon: true,
            });
        }
    }
    let lock_root = request.root.clone();
    let _lock = tokio::task::spawn_blocking(move || acquire_lock(&lock_root))
        .await
        .map_err(|e| CoreError::Process {
            command: "lock".into(),
            message: e.to_string(),
        })??;
    let pipeline = Pipeline {
        analyzer: CargoImpactAnalyzer::new(config.clone()),
        runner: Runner::new(config.clone()),
        cache: RedbCache::open(&request.root)
            .map_err(|e| tracing::warn!("cache disabled: {e}"))
            .ok(),
        compressor: LlmCompressor::new(config.lang),
    };
    let result = pipeline.verify(request).await?;
    Ok(Verified {
        result,
        via_daemon: false,
    })
}

async fn delegate(request: &VerifyRequest) -> Option<VerifyResult> {
    let root = request.root.clone();
    let message = DaemonRequest::Verify(request.clone());
    let answer = tokio::task::spawn_blocking(move || ipc::request(&root, &message)).await;
    match answer {
        Ok(Ok(Some(DaemonResponse::Verify(result)))) => Some(result),
        Ok(Ok(None)) => None,
        Ok(Ok(Some(DaemonResponse::Error { message }))) => {
            tracing::warn!("daemon error: {message}; running in-process");
            None
        }
        Ok(Ok(Some(other))) => {
            tracing::warn!("unexpected daemon response {other:?}; running in-process");
            None
        }
        Ok(Err(e)) => {
            tracing::warn!("daemon unavailable ({e}); running in-process");
            None
        }
        Err(e) => {
            tracing::warn!("daemon request task failed ({e}); running in-process");
            None
        }
    }
}

/// Takes the exclusive per-workspace lock; released when the file is dropped.
fn acquire_lock(root: &Path) -> Result<File, CoreError> {
    let dir = root::target_dir(root).join("orq");
    std::fs::create_dir_all(&dir).map_err(|e| CoreError::io(&dir, e))?;
    let path = dir.join("lock");
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(|e| CoreError::io(&path, e))?;
    file.lock().map_err(|e| CoreError::io(&path, e))?;
    Ok(file)
}
