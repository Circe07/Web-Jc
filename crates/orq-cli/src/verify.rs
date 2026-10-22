//! `orq verify`: impact, cache, runner, compressor; or delegation to a daemon.

use std::fs::OpenOptions;
use std::path::Path;

use anyhow::{Context, Result};
use orq_cache::RedbCache;
use orq_compress::LlmCompressor;
use orq_core::ipc::{self, DaemonRequest, DaemonResponse};
use orq_core::{
    ChangeSet, CompactReport, Compressor, Config, CrateId, Pipeline, VerifyRequest, VerifyResult,
};
use orq_impact::CargoImpactAnalyzer;
use orq_runner::Runner;
use serde_json::{json, Value};

use crate::{root, VerifyArgs};

/// Runs `orq verify` and returns the process exit code (0 pass, 1 fail).
pub fn run(args: &VerifyArgs) -> Result<u8> {
    let root = root::resolve(args.root.as_deref())?;
    let config = Config::load(&root)?;
    let budget = args.budget.unwrap_or(config.budget);
    let request = VerifyRequest {
        root: root.clone(),
        changes: (!args.files.is_empty()).then(|| ChangeSet {
            files: args
                .files
                .iter()
                .map(|f| root::relative_to_root(&root, f))
                .collect(),
        }),
        up_to: args.up_to.unwrap_or(config.up_to),
        token_budget: budget,
        use_cache: !args.no_cache,
    };

    let no_daemon = args.no_daemon || std::env::var_os("ORQ_NO_DAEMON").is_some_and(|v| v == "1");
    let delegated = if no_daemon {
        None
    } else {
        delegate(&root, &request)
    };
    let (result, report) = match delegated {
        Some(result) => {
            let report = LlmCompressor::new(config.lang).compress(&result, budget);
            (result, report)
        }
        None => in_process(&root, config, &request, budget)?,
    };

    let output = if args.json {
        let json = enrich(report.json, &result);
        serde_json::to_string_pretty(&json).context("serializing report")?
    } else {
        report.text
    };
    println!("{}", output.trim_end());
    Ok(if result.outcome.is_pass() { 0 } else { 1 })
}

fn delegate(root: &Path, request: &VerifyRequest) -> Option<VerifyResult> {
    match ipc::request(root, &DaemonRequest::Verify(request.clone())) {
        Ok(Some(DaemonResponse::Verify(result))) => Some(result),
        Ok(None) => None,
        Ok(Some(DaemonResponse::Error { message })) => {
            eprintln!("orq: daemon error: {message}; running in-process");
            None
        }
        Ok(Some(other)) => {
            eprintln!("orq: unexpected daemon response {other:?}; running in-process");
            None
        }
        Err(e) => {
            eprintln!("orq: daemon unavailable ({e}); running in-process");
            None
        }
    }
}

fn in_process(
    root: &Path,
    config: Config,
    request: &VerifyRequest,
    budget: usize,
) -> Result<(VerifyResult, CompactReport)> {
    let _lock = acquire_lock(root)?;
    let pipeline = Pipeline {
        analyzer: CargoImpactAnalyzer::new(config.clone()),
        runner: Runner::new(config.clone()),
        cache: RedbCache::open(root)
            .map_err(|e| tracing::warn!("cache disabled: {e}"))
            .ok(),
        compressor: LlmCompressor::new(config.lang),
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("building tokio runtime")?;
    let result = runtime.block_on(pipeline.verify(request))?;
    let report = pipeline.report(&result, budget);
    Ok((result, report))
}

/// Takes the exclusive per-workspace lock `target/orq/lock` (PLAN.md 7.2).
/// The lock is released when the returned file is dropped.
fn acquire_lock(root: &Path) -> Result<std::fs::File> {
    let dir = root::target_dir(root).join("orq");
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let path = dir.join("lock");
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .with_context(|| format!("opening {}", path.display()))?;
    file.lock()
        .with_context(|| format!("locking {}", path.display()))?;
    Ok(file)
}

/// Adds top-level `impact` (crate names) and `cache_hit` when missing.
fn enrich(mut json: Value, result: &VerifyResult) -> Value {
    let names =
        |crates: &[CrateId]| -> Vec<String> { crates.iter().map(|c| c.name.clone()).collect() };
    if let Value::Object(map) = &mut json {
        map.entry("impact").or_insert_with(|| {
            json!({
                "changed": names(&result.impact.changed),
                "dependents": names(&result.impact.dependents),
                "full_rebuild": result.impact.full_rebuild,
            })
        });
        map.entry("cache_hit").or_insert(json!(result.cache_hit));
    }
    json
}
