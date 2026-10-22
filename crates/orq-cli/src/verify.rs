//! `orq verify`: impact, cache, runner, compressor; or delegation to a daemon.

use anyhow::{Context, Result};
use orq_compress::LlmCompressor;
use orq_core::{root, stats, ChangeSet, Compressor, Config, CrateId, VerifyRequest, VerifyResult};
use orq_daemon::client::{self, Mode};
use serde_json::{json, Value};

use crate::VerifyArgs;

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
        build: None,
    };

    let no_daemon = args.no_daemon || std::env::var_os("ORQ_NO_DAEMON").is_some_and(|v| v == "1");
    let mode = if no_daemon {
        Mode::InProcess
    } else {
        Mode::Auto
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("building tokio runtime")?;
    let verified = runtime.block_on(client::verify(&request, &config, mode))?;
    let result = verified.result;
    let report = LlmCompressor::new(config.lang).compress(&result, budget);

    let record =
        stats::StatsRecord::new(&result, &report, request.up_to, verified.via_daemon, "cli");
    if let Err(e) = stats::append(&root, &record) {
        tracing::warn!("could not record stats: {e}");
    }

    let output = if args.json {
        let json = enrich(report.json, &result);
        serde_json::to_string_pretty(&json).context("serializing report")?
    } else {
        report.text
    };
    println!("{}", output.trim_end());
    Ok(if result.outcome.is_pass() { 0 } else { 1 })
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
