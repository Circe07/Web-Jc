//! Unix implementation of the daemon.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;

use notify::{EventKind, RecursiveMode};
use notify_debouncer_full::{new_debouncer, DebounceEventResult};
use orq_cache::RedbCache;
use orq_compress::LlmCompressor;
use orq_core::ipc::{self, DaemonRequest, DaemonResponse, DaemonStatus};
use orq_core::{
    Config, CoreError, ImpactAnalyzer, Pipeline, ResultCache, Step, StepRunner, VerifyRequest,
};
use orq_impact::CargoImpactAnalyzer;
use orq_runner::Runner;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{mpsc, watch, Mutex, Notify};

use crate::DaemonOptions;

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
    let shared = Arc::new(Shared {
        root: root.clone(),
        config,
        pipeline,
        cargo: Mutex::new(()),
        warming,
        pending: AtomicBool::new(false),
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
                        handle_connection(&shared, stream).await;
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
        shared.warming.send_replace(true);
        shared.wake_warmup.notify_one();
    }
}

/// Runs one background `cargo check` per burst of changes.
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

async fn warm_up(shared: &Arc<Shared>) -> Result<(), CoreError> {
    let _cargo = shared.cargo.lock().await;
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
    let result = shared
        .pipeline
        .runner
        .run(&shared.root, &impact, Step::Check, &shared.pipeline.build)
        .await?;
    if let Some(cache) = &shared.pipeline.cache {
        match cache.key(&shared.root, &impact, Step::Check, &shared.pipeline.build) {
            Ok(key) => {
                if let Err(e) = cache.put(&key, &result) {
                    tracing::warn!("warm-up cache store failed: {e}");
                }
            }
            Err(e) => tracing::warn!("warm-up cache key failed: {e}"),
        }
    }
    Ok(())
}

async fn handle_connection(shared: &Arc<Shared>, stream: UnixStream) {
    let (read, mut write) = stream.into_split();
    let mut line = String::new();
    if let Err(e) = BufReader::new(read).read_line(&mut line).await {
        tracing::warn!("could not read request: {e}");
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
