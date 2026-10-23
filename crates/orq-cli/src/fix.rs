//! `orq fix`: apply machine-applicable fixes, then verify (PLAN.md T5.3).

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Args;
use orq_compress::LlmCompressor;
use orq_core::{root, stats, ChangeSet, Compressor, Config, ImpactAnalyzer, Step, VerifyRequest};
use orq_daemon::client::{self, Mode};
use orq_impact::CargoImpactAnalyzer;
use serde_json::json;

/// Arguments of `orq fix`.
#[derive(Args)]
pub struct FixArgs {
    /// Workspace root (default: the workspace containing the current directory).
    #[arg(long)]
    pub root: Option<PathBuf>,
    /// Changed files (relative to the current directory); default: detect via git.
    #[arg(long, value_delimiter = ',')]
    pub files: Vec<PathBuf>,
    /// Last ladder step of the verification that follows (fmt, check, clippy, test).
    #[arg(long)]
    pub up_to: Option<Step>,
    /// Token budget of the report.
    #[arg(long)]
    pub budget: Option<usize>,
    /// Print JSON (`{"fixed": [...], "report": {...}}`) instead of text.
    #[arg(long)]
    pub json: bool,
    /// Never delegate to a daemon (also `ORQ_NO_DAEMON=1`).
    #[arg(long, hide = true)]
    pub no_daemon: bool,
}

/// Runs `orq fix` and returns the exit code (that of the verify that follows).
pub fn run(args: &FixArgs) -> Result<u8> {
    let root = root::resolve(args.root.as_deref())?;
    let config = Config::load(&root)?;
    let budget = args.budget.unwrap_or(config.budget);
    let up_to = args.up_to.unwrap_or(config.up_to);

    let analyzer = CargoImpactAnalyzer::new(config.clone());
    let changes = if args.files.is_empty() {
        analyzer.detect_changes(&root)?
    } else {
        ChangeSet {
            files: args
                .files
                .iter()
                .map(|f| root::relative_to_root(&root, f))
                .collect(),
        }
    };
    let impact = analyzer.analyze(&root, &changes)?;

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("building tokio runtime")?;
    let outcome = runtime
        .block_on(orq_runner::fix::apply_fixes(&root, &impact, &config.build))
        .context("applying fixes")?;
    let fixed: Vec<String> = outcome
        .files_changed
        .iter()
        .map(|p| p.display().to_string())
        .collect();
    let mut summary = format!("fixed {} file(s)", fixed.len());
    if !fixed.is_empty() {
        summary.push_str(": ");
        summary.push_str(&fixed.join(", "));
    }
    if !outcome.cargo_ok && fixed.is_empty() {
        summary.push_str(" (cargo could not apply fixes; the code may not compile)");
    }

    let request = VerifyRequest {
        root: root.clone(),
        changes: Some(changes),
        up_to,
        token_budget: budget,
        use_cache: true,
        build: None,
    };
    let no_daemon = args.no_daemon || std::env::var_os("ORQ_NO_DAEMON").is_some_and(|v| v == "1");
    let mode = if no_daemon {
        Mode::InProcess
    } else {
        Mode::Auto
    };
    let verified = runtime.block_on(client::verify(&request, &config, mode))?;
    let result = verified.result;
    let report = LlmCompressor::new(config.lang).compress(&result, budget);
    let record = stats::StatsRecord::new(&result, &report, up_to, verified.via_daemon, "fix");
    if let Err(e) = stats::append(&root, &record) {
        tracing::warn!("could not record stats: {e}");
    }

    if args.json {
        let out = json!({"fixed": fixed, "report": report.json});
        println!(
            "{}",
            serde_json::to_string_pretty(&out).context("serializing report")?
        );
    } else {
        println!("{summary}");
        println!("{}", report.text.trim_end());
    }
    Ok(if result.outcome.is_pass() { 0 } else { 1 })
}
