//! `orq fix`: apply machine-applicable fixes, then verify (PLAN.md T5.3).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Args;
use orq_compress::LlmCompressor;
use orq_core::{
    root, stats, BuildOptions, ChangeSet, Compressor, Config, ImpactAnalyzer, Step, VerifyRequest,
};
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

    let explicit = (!args.files.is_empty()).then(|| ChangeSet {
        files: args
            .files
            .iter()
            .map(|f| root::relative_to_root(&root, f))
            .collect(),
    });
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("building tokio runtime")?;
    let (changes, outcome) =
        runtime.block_on(fix_first(&root, &config, explicit, &config.build))?;
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

/// The budget left for the report once the one-line `note` (such as the
/// `fixed: ...` line) is printed above it, so both together stay within
/// `budget`.
pub(crate) fn budget_after_note(budget: usize, note: Option<&str>) -> usize {
    match note {
        // +1 token for the newline between the two.
        Some(note) => budget.saturating_sub(orq_core::estimate_tokens(note) + 1),
        None => budget,
    }
}

/// `text` with `note` as its first line.
pub(crate) fn with_note(note: Option<&str>, text: &str) -> String {
    match note {
        Some(note) => format!("{note}\n{}", text.trim_end()),
        None => text.trim_end().to_string(),
    }
}

/// Applies the machine-applicable fixes to the crates affected by `explicit`
/// (or by the files the VCS reports as changed) and returns the change set
/// that was used, so the verification that follows covers the same crates.
///
/// Shared by `orq fix`, `orq verify --fix` and the hook.
pub(crate) async fn fix_first(
    root: &Path,
    config: &Config,
    explicit: Option<ChangeSet>,
    build: &BuildOptions,
) -> Result<(ChangeSet, orq_runner::fix::FixOutcome)> {
    let analyzer = CargoImpactAnalyzer::new(config.clone());
    let changes = match explicit {
        Some(changes) => changes,
        None => analyzer.detect_changes(root)?,
    };
    let impact = analyzer.analyze(root, &changes)?;
    let outcome = orq_runner::fix::apply_fixes(root, &impact, build)
        .await
        .context("applying fixes")?;
    Ok((changes, outcome))
}
