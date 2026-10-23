//! Unix implementation of the daemon.
//!
//! # Security model
//!
//! Only the user who started the daemon can talk to it:
//!
//! - The socket lives in a private directory (`ipc::prepare_socket_dir`, mode
//!   0700, owned by the user, never a symlink) and the socket file itself is
//!   set to mode 0600 right after `bind`, before anything is accepted.
//! - Every accepted connection is checked with `SO_PEERCRED`/`LOCAL_PEERCRED`
//!   (`peer_cred`). A peer whose uid differs from the daemon's is closed
//!   without reading a byte; if the credentials cannot be obtained the
//!   connection is refused (fail closed).
//! - Requests are one JSON line, capped at 1 MiB and 30 s. Clients exceeding
//!   either limit are dropped, so a local process cannot wedge handlers.

use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use notify::{EventKind, RecursiveMode};
use notify_debouncer_full::{new_debouncer, DebounceEventResult};
use orq_cache::RedbCache;
use orq_compress::LlmCompressor;
use orq_core::ipc::{self, DaemonRequest, DaemonResponse, DaemonStatus};
use orq_core::{
    CompactReport, Config, CoreError, ImpactAnalyzer, Pipeline, ResultCache, Step, StepRunner,
    VerifyRequest,
};
use orq_impact::CargoImpactAnalyzer;
use orq_runner::Runner;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{mpsc, watch, Mutex, Notify};

use crate::DaemonOptions;

/// Largest accepted request line, in bytes.
const MAX_REQUEST_BYTES: u64 = 1024 * 1024;
/// How long a client may take to deliver its request line.
const READ_TIMEOUT: Duration = Duration::from_secs(30);

/// Whether a peer with uid `peer` (if known) may talk to a daemon owned by
/// `own`. Unknown credentials are refused.
fn peer_allowed(peer: Option<u32>, own: u32) -> bool {
    peer == Some(own)
}

type DaemonPipeline = Pipeline<CargoImpactAnalyzer, Runner, RedbCache, LlmCompressor>;

fn process_error(message: impl Into<String>) -> CoreError {
    CoreError::Process {
        command: "orq daemon".into(),
        message: message.into(),
    }
}

/// Removes the socket file when dropped, on every exit path.
struct SocketGuard(PathBuf);

impl Drop for SocketGuard {
    fn drop(&mut self) {
        if let Err(e) = std::fs::remove_file(&self.0) {
            if e.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!("could not remove socket {}: {e}", self.0.display());
            }
        }
    }
}

/// State shared by the accept loop, connection handlers and the warm-up worker.
struct Shared {
    root: PathBuf,
    config: Config,
    pipeline: DaemonPipeline,
    /// Only one cargo invocation at a time per workspace.
    cargo: Mutex<()>,
    /// `true` while a warm-up is queued or running.
    warming: watch::Sender<bool>,
    /// Set when a change arrived; consumed by the warm-up worker.
    pending: AtomicBool,
    /// Bumped on every relevant change; lets a running prebuild notice newer work.
    change_gen: watch::Sender<u64>,
    wake_warmup: Notify,
    /// Wakes the main loop to reset the idle timer.
    activity: Notify,
    stop: Notify,
    in_flight: AtomicUsize,
    requests: AtomicU64,
    started: Instant,
}

pub(crate) async fn serve(root: &Path, options: DaemonOptions) -> Result<(), CoreError> {
    let root = root.canonicalize().map_err(|e| CoreError::io(root, e))?;
    let config = Config::load(&root)?;
    let socket = ipc::socket_path(&root);
    ipc::prepare_socket_dir(&socket)?;

    if socket.exists() {
        let probe_root = root.clone();
        let answer =
            tokio::task::spawn_blocking(move || ipc::request(&probe_root, &DaemonRequest::Status))
                .await
                .map_err(|e| process_error(e.to_string()))??;
        if answer.is_some() {
            return Err(process_error(format!(
                "already running for {} (socket {})",
                root.display(),
                socket.display()
            )));
        }
        tracing::info!("removing stale socket {}", socket.display());
        std::fs::remove_file(&socket).map_err(|e| CoreError::io(&socket, e))?;
    }

    let listener = UnixListener::bind(&socket).map_err(|e| CoreError::io(&socket, e))?;
    let _guard = SocketGuard(socket.clone());
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| CoreError::io(&socket, e))?;
    let own_uid = std::fs::metadata(&socket)
        .map_err(|e| CoreError::io(&socket, e))?
        .uid();

    let pipeline = Pipeline {
        analyzer: CargoImpactAnalyzer::new(config.clone()),
        runner: Runner::new(config.clone()),
        cache: RedbCache::open(&root)
            .map_err(|e| tracing::warn!("cache disabled: {e}"))
            .ok(),
        compressor: LlmCompressor::new(config.lang),
        build: config.build.clone(),
    };
    let (warming, _) = watch::channel(false);
    let (change_gen, _) = watch::channel(0_u64);
    let shared = Arc::new(Shared {
        root: root.clone(),
        config,
        pipeline,
        cargo: Mutex::new(()),
        warming,
        pending: AtomicBool::new(false),
        change_gen,
        wake_warmup: Notify::new(),
        activity: Notify::new(),
        stop: Notify::new(),
        in_flight: AtomicUsize::new(0),
        requests: AtomicU64::new(0),
        started: Instant::now(),
    });

    // File watching: the debouncer callback is synchronous, so forward its
    // events into a tokio channel.
    let (events_tx, mut events_rx) = mpsc::unbounded_channel::<Vec<PathBuf>>();
    let mut debouncer = new_debouncer(
        options.debounce,
        None,
        move |result: DebounceEventResult| {
            match result {
                Ok(events) => {
                    let paths: Vec<PathBuf> = events
                        .into_iter()
                        .filter(|e| !matches!(e.event.kind, EventKind::Access(_)))
                        .flat_map(|e| e.event.paths)
                        .collect();
                    if !paths.is_empty() {
                        // The receiver only disappears while shutting down.
                        let _ = events_tx.send(paths);
                    }
                }
                Err(errors) => {
                    for e in errors {
                        tracing::warn!("watcher error: {e}");
                    }
                }
            }
        },
    )
    .map_err(|e| process_error(format!("cannot start file watcher: {e}")))?;
    debouncer
        .watch(&root, RecursiveMode::Recursive)
        .map_err(|e| process_error(format!("cannot watch {}: {e}", root.display())))?;

    let worker = tokio::spawn(warmup_worker(Arc::clone(&shared)));
    tracing::info!("daemon listening on {}", socket.display());

    let idle = tokio::time::sleep(options.idle_timeout);
    tokio::pin!(idle);
    let result = loop {
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => {
                    shared.in_flight.fetch_add(1, Ordering::SeqCst);
                    idle.as_mut().reset(tokio::time::Instant::now() + options.idle_timeout);
                    let shared = Arc::clone(&shared);
                    tokio::spawn(async move {
                        match stream.peer_cred() {
                            Ok(cred) if peer_allowed(Some(cred.uid()), own_uid) => {
                                handle_connection(&shared, stream).await;
                            }
                            Ok(cred) => tracing::warn!(
                                "rejected connection from uid {} (daemon uid {own_uid})",
                                cred.uid()
                            ),
                            Err(e) => tracing::warn!("rejected connection, no peer credentials: {e}"),
                        }
                        shared.in_flight.fetch_sub(1, Ordering::SeqCst);
                        shared.activity.notify_one();
                    });
                }
                Err(e) => tracing::warn!("accept failed: {e}"),
            },
            Some(paths) = events_rx.recv() => on_changes(&shared, &paths),
            () = shared.activity.notified() => {
                idle.as_mut().reset(tokio::time::Instant::now() + options.idle_timeout);
            }
            () = shared.stop.notified() => {
                tracing::info!("stop requested");
                break Ok(());
            }
            () = &mut idle => {
                if shared.in_flight.load(Ordering::SeqCst) > 0 {
                    idle.as_mut().reset(tokio::time::Instant::now() + options.idle_timeout);
                } else {
                    tracing::info!("idle timeout reached");
                    break Ok(());
                }
            }
        }
    };

    drop(debouncer);
    worker.abort();
    result
}

/// Reacts to a debounced batch of changed absolute paths.
fn on_changes(shared: &Arc<Shared>, paths: &[PathBuf]) {
    let mut relevant = false;
    for path in paths {
        let Ok(rel) = path.strip_prefix(&shared.root) else {
            continue;
        };
        if rel
            .components()
            .any(|c| c.as_os_str() == "target" || c.as_os_str() == ".git")
        {
            continue;
        }
        if shared.config.is_ignored(rel) {
            continue;
        }
        if rel.file_name().is_some_and(|n| n == "Cargo.toml") {
            shared.pipeline.analyzer.invalidate(&shared.root);
        }
        relevant = true;
    }
    if relevant {
        shared.pending.store(true, Ordering::SeqCst);
        shared.change_gen.send_modify(|g| *g += 1);
        shared.warming.send_replace(true);
        shared.wake_warmup.notify_one();
    }
}

/// Step the warm-up runs: the configured `up_to` capped at `Clippy`, so the
/// background cargo invocation (and its artifacts) match a real verify.
fn warmup_step(config: &Config) -> Step {
    config.up_to.min(Step::Clippy)
}

/// Runs one background warm-up per burst of changes.
async fn warmup_worker(shared: Arc<Shared>) {
    loop {
        shared.wake_warmup.notified().await;
        while shared.pending.swap(false, Ordering::SeqCst) {
            if let Err(e) = warm_up(&shared).await {
                tracing::warn!("warm-up failed: {e}");
            }
        }
        shared.warming.send_replace(false);
        shared.activity.notify_one();
    }
}

/// One warm-up: the compile step at [`warmup_step`] (result cached under that
/// step's key), then, when `up_to == Test`, `Runner::prebuild_tests`.
///
/// The prebuild is abandoned (its cargo is killed on drop) as soon as a newer
/// change arrives, so a long prebuild cannot starve the next round. The
/// duration is appended to the stats as `source = "daemon-warmup"` with a
/// minimal [`CompactReport`] (`estimated_tokens = 0`, empty `text`/`json`).
async fn warm_up(shared: &Arc<Shared>) -> Result<(), CoreError> {
    let _cargo = shared.cargo.lock().await;
    let started = Instant::now();
    let root = shared.root.clone();
    let detector = Arc::clone(shared);
    let impact = tokio::task::spawn_blocking(move || {
        let changes = detector.pipeline.analyzer.detect_changes(&root)?;
        detector.pipeline.analyzer.analyze(&root, &changes)
    })
    .await
    .map_err(|e| process_error(e.to_string()))??;
    if impact.is_empty() {
        return Ok(());
    }
    let step = warmup_step(&shared.config);
    let build = &shared.pipeline.build;
    tracing::info!("warm-up: compiling up to {step:?}");
    let mut result = shared
        .pipeline
        .runner
        .run(&shared.root, &impact, step, build)
        .await?;
    if let Some(cache) = &shared.pipeline.cache {
        match cache.key(&shared.root, &impact, step, build) {
            Ok(key) => {
                if let Err(e) = cache.put(&key, &result) {
                    tracing::warn!("warm-up cache store failed: {e}");
                }
            }
            Err(e) => tracing::warn!("warm-up cache key failed: {e}"),
        }
    }
    if shared.config.up_to == Step::Test && result.outcome.is_pass() {
        let mut changes = shared.change_gen.subscribe();
        changes.mark_unchanged();
        tracing::info!("warm-up: prebuilding test binaries");
        tokio::select! {
            built = shared.pipeline.runner.prebuild_tests(&shared.root, &impact, build) => {
                if let Err(e) = built {
                    tracing::warn!("warm-up prebuild failed: {e}");
                }
            }
            _ = changes.changed() => {
                tracing::info!("warm-up: new change arrived, abandoning prebuild");
            }
        }
    }
    result.total_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    tracing::info!("warm-up finished in {} ms", result.total_ms);
    let report = CompactReport {
        text: String::new(),
        json: serde_json::Value::Null,
        estimated_tokens: 0,
        truncated: false,
    };
    let record = orq_core::stats::StatsRecord::new(&result, &report, step, true, "daemon-warmup");
    if let Err(e) = orq_core::stats::append(&shared.root, &record) {
        tracing::warn!("warm-up stats failed: {e}");
    }
    Ok(())
}

async fn handle_connection(shared: &Arc<Shared>, stream: UnixStream) {
    let (read, mut write) = stream.into_split();
    let mut line = String::new();
    let mut reader = BufReader::new(read.take(MAX_REQUEST_BYTES));
    match tokio::time::timeout(READ_TIMEOUT, reader.read_line(&mut line)).await {
        Err(_) => {
            tracing::warn!("dropping client: no complete request within {READ_TIMEOUT:?}");
            return;
        }
        Ok(Err(e)) => {
            tracing::warn!("could not read request: {e}");
            return;
        }
        Ok(Ok(_)) => {}
    }
    if !line.ends_with('\n') && line.len() as u64 >= MAX_REQUEST_BYTES {
        tracing::warn!("dropping client: request exceeds {MAX_REQUEST_BYTES} bytes");
        return;
    }
    shared.requests.fetch_add(1, Ordering::SeqCst);
    let (response, stop) = match parse_request(&line) {
        Ok(request) => respond(shared, request).await,
        Err(message) => (DaemonResponse::Error { message }, false),
    };
    let mut out = match serde_json::to_string(&response) {
        Ok(out) => out,
        Err(e) => {
            tracing::warn!("could not encode response: {e}");
            return;
        }
    };
    out.push('\n');
    if let Err(e) = write.write_all(out.as_bytes()).await {
        tracing::warn!("could not write response: {e}");
    }
    let _ = write.shutdown().await;
    if stop {
        shared.stop.notify_one();
    }
}

async fn respond(shared: &Arc<Shared>, request: DaemonRequest) -> (DaemonResponse, bool) {
    match request {
        DaemonRequest::Status => (
            DaemonResponse::Status(DaemonStatus {
                root: shared.root.clone(),
                pid: std::process::id(),
                uptime_ms: u64::try_from(shared.started.elapsed().as_millis()).unwrap_or(u64::MAX),
                requests: shared.requests.load(Ordering::SeqCst),
                warming: *shared.warming.borrow(),
            }),
            false,
        ),
        DaemonRequest::Stop => (DaemonResponse::Stopping, true),
        DaemonRequest::Verify(mut request) => {
            request.root.clone_from(&shared.root);
            (verify(shared, &request).await, false)
        }
    }
}

/// Waits for an in-flight warm-up, then verifies with exclusive cargo access.
async fn verify(shared: &Arc<Shared>, request: &VerifyRequest) -> DaemonResponse {
    let mut warming = shared.warming.subscribe();
    if warming.wait_for(|w| !*w).await.is_err() {
        return DaemonResponse::Error {
            message: "daemon is shutting down".into(),
        };
    }
    let _cargo = shared.cargo.lock().await;
    match shared.pipeline.verify(request).await {
        Ok(result) => DaemonResponse::Verify(result),
        Err(e) => DaemonResponse::Error {
            message: e.to_string(),
        },
    }
}

fn parse_request(line: &str) -> Result<DaemonRequest, String> {
    if line.trim().is_empty() {
        return Err("empty request".into());
    }
    serde_json::from_str(line.trim()).map_err(|e| format!("bad request: {e}"))
}

#[cfg(test)]
mod tests {
    use super::peer_allowed;

    #[test]
    fn same_uid_is_allowed() {
        assert!(peer_allowed(Some(1000), 1000));
    }

    #[test]
    fn other_uid_is_refused() {
        assert!(!peer_allowed(Some(1001), 1000));
        assert!(!peer_allowed(Some(0), 1000));
    }

    #[test]
    fn unknown_credentials_are_refused() {
        assert!(!peer_allowed(None, 1000));
    }
}
