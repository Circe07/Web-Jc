//! `orq hook`: Claude Code `PostToolUse` hook entry point (PLAN.md T5.4).
//!
//! Claude Code runs the command after `Edit`, `Write` or `MultiEdit`
//! succeeds and pipes a JSON payload on stdin (`cwd`, `tool_name`,
//! `tool_input.file_path`, ...). Behaviour, per the official hooks docs
//! (<https://code.claude.com/docs/en/hooks>):
//!
//! - Pass, irrelevant file, or any internal error: exit 0 with no output, so
//!   the agent is never blocked or distracted.
//! - Fail: exit 0 and print the JSON decision
//!   `{"decision":"block","reason":"<compact report>"}`. For `PostToolUse`
//!   the `reason` is shown to Claude next to the tool result (the tool
//!   already ran, so nothing is undone). The structured form is preferred
//!   over exit code 2 + stderr because it is the documented way to attach
//!   feedback and keeps stderr for logs.

use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Args;
use orq_compress::LlmCompressor;
use orq_core::{root, stats, ChangeSet, Compressor, Config, Step, VerifyRequest};
use orq_daemon::client::{self, Mode};
use serde_json::{json, Value};

use crate::fix;

/// Default ladder depth of the hook: `check` keeps the feedback loop fast;
/// clippy and tests run when the agent calls `orq verify` / the MCP tool.
const DEFAULT_UP_TO: Step = Step::Check;
/// Default report budget of the hook: failures are injected into the agent's
/// context after every edit, so they must stay small.
const DEFAULT_BUDGET: usize = 600;

/// Arguments of `orq hook`.
#[derive(Args)]
pub struct HookArgs {
    /// Workspace root (default: the workspace containing the edited file).
    #[arg(long)]
    pub root: Option<PathBuf>,
    /// Last ladder step to run (fmt, check, clippy, test). Default `check`.
    #[arg(long)]
    pub up_to: Option<Step>,
    /// Token budget of the report fed back to Claude. Default 600.
    #[arg(long)]
    pub budget: Option<usize>,
    /// Apply the machine-applicable rustc/clippy fixes to the affected crates
    /// before verifying (also `ORQ_HOOK_FIX=1`). Off by default: it edits
    /// files and runs clippy after every edit. When it changes files, the
    /// agent is told which.
    #[arg(long)]
    pub fix: bool,
}

/// `true` if the hook should fix before verifying: the flag, or
/// `ORQ_HOOK_FIX` set to `1` / `true`.
fn fix_enabled(args: &HookArgs) -> bool {
    args.fix
        || std::env::var("ORQ_HOOK_FIX")
            .is_ok_and(|v| matches!(v.trim(), "1" | "true" | "TRUE" | "True"))
}

/// Runs `orq hook` and returns the exit code. Never fails: internal errors
/// are logged to stderr and map to exit 0 so the agent is not blocked.
pub fn run(args: &HookArgs) -> Result<u8> {
    let mut input = String::new();
    if let Err(e) = std::io::stdin().read_to_string(&mut input) {
        tracing::warn!("orq hook: cannot read stdin: {e}");
        return Ok(0);
    }
    match handle(args, &input) {
        Ok(Some(output)) => {
            println!("{output}");
            Ok(0)
        }
        Ok(None) => Ok(0),
        Err(e) => {
            tracing::warn!("orq hook: {e:#}");
            Ok(0)
        }
    }
}

/// Edited files that matter to cargo, from the hook payload.
fn edited_rust_files(payload: &Value) -> Vec<PathBuf> {
    if let Some(event) = payload.get("hook_event_name").and_then(Value::as_str) {
        if event != "PostToolUse" {
            return Vec::new();
        }
    }
    if let Some(tool) = payload.get("tool_name").and_then(Value::as_str) {
        if !matches!(tool, "Edit" | "Write" | "MultiEdit" | "NotebookEdit") {
            return Vec::new();
        }
    }
    let Some(input) = payload.get("tool_input") else {
        return Vec::new();
    };
    ["file_path", "notebook_path", "path"]
        .iter()
        .filter_map(|k| input.get(*k).and_then(Value::as_str))
        .map(PathBuf::from)
        .filter(|p| is_cargo_relevant(p))
        .collect()
}

/// `.rs`, `Cargo.toml`, `Cargo.lock` and `build.rs` files.
fn is_cargo_relevant(path: &Path) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    matches!(name, "Cargo.toml" | "Cargo.lock") || path.extension().is_some_and(|e| e == "rs")
}

/// Parses the payload, verifies and returns the JSON to print on failure.
fn handle(args: &HookArgs, input: &str) -> Result<Option<String>> {
    let payload: Value = serde_json::from_str(input).context("malformed hook JSON on stdin")?;
    let files = edited_rust_files(&payload);
    if files.is_empty() {
        return Ok(None);
    }

    // Resolve relative paths against the payload `cwd`, then work from the
    // file's directory so the root lookup finds the right workspace.
    let cwd = payload
        .get("cwd")
        .and_then(Value::as_str)
        .map(PathBuf::from);
    let absolute = |p: &Path| match (&cwd, p.is_absolute()) {
        (Some(cwd), false) => cwd.join(p),
        _ => p.to_path_buf(),
    };
    let files: Vec<PathBuf> = files.iter().map(|p| absolute(p)).collect();
    let anchor = files
        .first()
        .and_then(|f| f.parent())
        .filter(|d| d.is_dir())
        .map(Path::to_path_buf)
        .or(cwd)
        .context("no directory to anchor the workspace lookup")?;
    std::env::set_current_dir(&anchor).context("entering the edited file's directory")?;

    let root = match root::resolve(args.root.as_deref()) {
        Ok(root) => root,
        Err(e) => {
            tracing::debug!("orq hook: not in a cargo workspace: {e}");
            return Ok(None);
        }
    };
    let config = Config::load(&root)?;
    let relative: Vec<PathBuf> = files
        .iter()
        .map(|f| root::relative_to_root(&root, f))
        .filter(|f| f.is_relative() && !config.is_ignored(f))
        .collect();
    if relative.is_empty() {
        return Ok(None);
    }

    let up_to = args.up_to.unwrap_or(DEFAULT_UP_TO);
    let budget = args.budget.unwrap_or(DEFAULT_BUDGET);
    let mode = if std::env::var_os("ORQ_NO_DAEMON").is_some_and(|v| v == "1") {
        Mode::InProcess
    } else {
        Mode::Auto
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("building tokio runtime")?;

    let changes = ChangeSet { files: relative };
    let mut note: Option<String> = None;
    if fix_enabled(args) && up_to != Step::Fmt {
        // A failure to fix must not hide the verification: log and go on.
        match runtime.block_on(fix::fix_first(
            &root,
            &config,
            Some(changes.clone()),
            &config.build,
        )) {
            Ok((_, outcome)) => note = outcome.summary(),
            Err(e) => tracing::warn!("orq hook: could not apply fixes: {e:#}"),
        }
    }
    let report_budget = fix::budget_after_note(budget, note.as_deref());
    let request = VerifyRequest {
        root: root.clone(),
        changes: Some(changes),
        up_to,
        token_budget: report_budget,
        use_cache: true,
        build: None,
    };
    let verified = runtime.block_on(client::verify(&request, &config, mode))?;
    let result = verified.result;
    let report = LlmCompressor::new(config.lang).compress(&result, report_budget);

    let record = stats::StatsRecord::new(&result, &report, up_to, verified.via_daemon, "hook");
    if let Err(e) = stats::append(&root, &record) {
        tracing::warn!("could not record stats: {e}");
    }

    if result.outcome.is_pass() {
        // Passing, but orq edited files: the agent must know.
        return Ok(note.map(|note| {
            json!({
                "hookSpecificOutput": {
                    "hookEventName": "PostToolUse",
                    "additionalContext": format!("orq: {note}"),
                }
            })
            .to_string()
        }));
    }
    let reason = format!(
        "orq verify failed after your edit:\n{}",
        fix::with_note(note.as_deref(), &report.text)
    );
    Ok(Some(
        json!({
            "decision": "block",
            "reason": reason,
        })
        .to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(tool: &str, path: &str) -> Value {
        json!({
            "hook_event_name": "PostToolUse",
            "tool_name": tool,
            "tool_input": {"file_path": path},
        })
    }

    #[test]
    fn filters_by_extension() {
        assert_eq!(
            edited_rust_files(&payload("Edit", "/a/src/lib.rs")).len(),
            1
        );
        assert_eq!(
            edited_rust_files(&payload("Write", "/a/Cargo.toml")).len(),
            1
        );
        assert_eq!(edited_rust_files(&payload("Write", "/a/build.rs")).len(), 1);
        assert!(edited_rust_files(&payload("Edit", "/a/README.md")).is_empty());
        assert!(edited_rust_files(&payload("Edit", "/a/Makefile")).is_empty());
    }

    #[test]
    fn ignores_other_events_and_tools() {
        assert!(edited_rust_files(&payload("Bash", "/a/lib.rs")).is_empty());
        let mut p = payload("Edit", "/a/lib.rs");
        p["hook_event_name"] = json!("PreToolUse");
        assert!(edited_rust_files(&p).is_empty());
    }

    #[test]
    fn malformed_input_is_an_error_not_a_panic() {
        let args = HookArgs {
            root: None,
            up_to: None,
            budget: None,
            fix: false,
        };
        assert!(handle(&args, "not json").is_err());
        assert!(handle(&args, "{}").unwrap().is_none());
    }
}
