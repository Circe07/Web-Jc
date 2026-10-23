//! `orq verify`: impact, cache, runner, compressor; or delegation to a daemon.

use anyhow::{Context, Result};
use orq_compress::LlmCompressor;
use orq_core::{
    root, stats, BuildOptions, ChangeSet, Compressor, Config, CrateId, Step, VerifyRequest,
    VerifyResult,
};
use orq_daemon::client::{self, Mode};
use serde_json::{json, Value};

use crate::{fix, VerifyArgs};

/// Runs `orq verify` and returns the process exit code (0 pass, 1 fail).
pub fn run(args: &VerifyArgs) -> Result<u8> {
    let root = root::resolve(args.root.as_deref())?;
    let config = Config::load(&root)?;
    let budget = args.budget.unwrap_or(config.budget);
    let mut request = VerifyRequest {
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
        build: build_options(args, &config),
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

    // `--fix`: apply the machine-applicable suggestions first, so the report
    // is about the code the agent will actually have.
    let mut fixed: Vec<String> = Vec::new();
    let mut note: Option<String> = None;
    if args.fix && request.up_to != Step::Fmt {
        let effective = request
            .build
            .clone()
            .unwrap_or_else(|| config.build.clone());
        let (changes, outcome) = runtime.block_on(fix::fix_first(
            &root,
            &config,
            request.changes.take(),
            &effective,
        ))?;
        request.changes = Some(changes);
        fixed = outcome
            .files_changed
            .iter()
            .map(|p| p.display().to_string())
            .collect();
        note = outcome.summary();
    }
    let report_budget = fix::budget_after_note(budget, note.as_deref());
    request.token_budget = report_budget;

    let verified = runtime.block_on(client::verify(&request, &config, mode))?;
    let result = verified.result;
    let report = LlmCompressor::new(config.lang).compress(&result, report_budget);

    let record =
        stats::StatsRecord::new(&result, &report, request.up_to, verified.via_daemon, "cli");
    if let Err(e) = stats::append(&root, &record) {
        tracing::warn!("could not record stats: {e}");
    }

    let output = if args.json {
        let effective = request
            .build
            .clone()
            .unwrap_or_else(|| config.build.clone());
        let mut json = enrich(report.json, &result, &effective);
        if args.fix {
            json["fixed"] = json!(fixed);
        }
        serde_json::to_string_pretty(&json).context("serializing report")?
    } else {
        fix::with_note(note.as_deref(), &report.text)
    };
    println!("{}", output.trim_end());
    Ok(if result.outcome.is_pass() { 0 } else { 1 })
}

/// Adds top-level `impact` (crate names), `cache_hit` and the effective
/// `build` options when missing.
fn enrich(mut json: Value, result: &VerifyResult, build: &BuildOptions) -> Value {
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
        map.entry("build").or_insert_with(|| json!(build));
    }
    json
}

/// Build options requested on the command line, layered over `config.build`;
/// `None` when no build flag was given (the pipeline default applies).
fn build_options(args: &VerifyArgs, config: &Config) -> Option<BuildOptions> {
    if args.features.is_empty()
        && !args.all_features
        && !args.no_default_features
        && args.target.is_none()
    {
        return None;
    }
    let mut build = config.build.clone();
    if !args.features.is_empty() {
        build.features = args.features.clone();
    }
    build.all_features |= args.all_features;
    build.no_default_features |= args.no_default_features;
    if args.target.is_some() {
        build.target = args.target.clone();
    }
    Some(build)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(extra: &[&str]) -> VerifyArgs {
        use clap::Parser;
        #[derive(Parser)]
        struct W {
            #[command(flatten)]
            v: VerifyArgs,
        }
        let argv = std::iter::once("orq").chain(extra.iter().copied());
        W::parse_from(argv).v
    }

    #[test]
    fn no_flags_means_default() {
        assert_eq!(build_options(&args(&[]), &Config::default()), None);
    }

    #[test]
    fn flags_override_config() {
        let mut config = Config::default();
        config.build.features = vec!["old".into()];
        config.build.target = Some("t1".into());
        let a = args(&["--features", "a,b", "--no-default-features"]);
        let b = build_options(&a, &config).unwrap();
        assert_eq!(b.features, ["a", "b"]);
        assert!(b.no_default_features && !b.all_features);
        assert_eq!(b.target.as_deref(), Some("t1"));
        let b = build_options(&args(&["--target", "t2", "--all-features"]), &config).unwrap();
        assert_eq!(b.features, ["old"]);
        assert!(b.all_features);
        assert_eq!(b.target.as_deref(), Some("t2"));
    }
}
