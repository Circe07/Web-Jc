//! Tool definitions and implementations.

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use orq_compress::LlmCompressor;
use orq_core::{
    stats, ChangeSet, Compressor, Config, CoreError, CrateId, ImpactAnalyzer, Step, VerifyRequest,
};
use orq_daemon::client::{self, Mode};
use orq_impact::CargoImpactAnalyzer;
use rmcp::model::{CallToolResult, ContentBlock, JsonObject, Tool, ToolAnnotations};
use serde::Deserialize;
use serde_json::{json, Value};

/// Budget used by `explain_error`.
const EXPLAIN_BUDGET: usize = 400;

/// Shared server state.
pub(crate) struct State {
    pub(crate) root: PathBuf,
    pub(crate) config: Config,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct VerifyArgs {
    files: Option<Vec<String>>,
    up_to: Option<Step>,
    budget: Option<usize>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct TestArgs {
    files: Option<Vec<String>>,
    budget: Option<usize>,
}

#[derive(Deserialize)]
struct ExplainArgs {
    code: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ImpactArgs {
    files: Option<Vec<String>>,
}

fn schema(value: Value) -> Arc<JsonObject> {
    match value {
        Value::Object(map) => Arc::new(map),
        _ => Arc::new(JsonObject::new()),
    }
}

fn titled(title: &str) -> ToolAnnotations {
    ToolAnnotations::with_title(title)
}

/// The four tools, with descriptions written for the model.
pub(crate) fn definitions() -> Vec<Tool> {
    let files = json!({
        "type": "array",
        "items": {"type": "string"},
        "description": "Changed files, relative to the workspace root. Omit to detect them with git."
    });
    let budget = json!({
        "type": "integer",
        "minimum": 1,
        "description": "Token budget for the report (default: from orq config)."
    });
    let mut verify = Tool::new(
        "verify",
        "Verify the Rust workspace after you edit code. Call this after EVERY edit instead of \
         `cargo build`, `cargo check`, `cargo clippy` or `cargo test`: it detects which crates \
         your changes affect, runs only fmt/check/clippy/tests on those, caches results, and \
         returns a report that is already compact (errors deduplicated and ranked, no noise). \
         Do not run cargo commands yourself. A failing build is a normal result (read the \
         report), not a tool failure. Use `up_to` to choose how far to go: `check` while \
         iterating, `clippy` for lints, `test` (default) before you finish.",
        schema(json!({
            "type": "object",
            "properties": {
                "files": files,
                "up_to": {
                    "type": "string",
                    "enum": ["check", "clippy", "test"],
                    "description": "Last step to run. Default: from orq config (usually test)."
                },
                "budget": budget,
            },
            "additionalProperties": false
        })),
    );
    verify.annotations = Some(titled("Verify workspace"));
    let mut tests = Tool::new(
        "run_affected_tests",
        "Run the tests of the crates affected by your changes (and their dependents), after \
         fmt, check and clippy pass. Shortcut for `verify` with up_to=test. Use it instead of \
         `cargo test` / `cargo nextest` when you want to confirm behaviour before finishing. \
         Returns a compact report.",
        schema(json!({
            "type": "object",
            "properties": {"files": files, "budget": budget},
            "additionalProperties": false
        })),
    );
    tests.annotations = Some(titled("Run affected tests"));
    let mut explain = Tool::new(
        "explain_error",
        "Explain a rustc error code (for example `E0308`) in a few lines with the typical fix. \
         Use it when a `verify` report shows a code you do not know how to fix; it is cheaper \
         than guessing or searching the web.",
        schema(json!({
            "type": "object",
            "properties": {
                "code": {"type": "string", "description": "Error code such as E0308."}
            },
            "required": ["code"],
            "additionalProperties": false
        })),
    );
    explain.annotations = Some(titled("Explain rustc error"));
    let mut impact = Tool::new(
        "impact",
        "Show which crates a set of changed files affects: the crates that contain the files, \
         their transitive dependents, and whether the whole workspace must be rebuilt. Use it \
         to plan work and to predict what `verify` will check. It does not run any build step.",
        schema(json!({
            "type": "object",
            "properties": {"files": files},
            "additionalProperties": false
        })),
    );
    impact.annotations = Some(titled("Impact of changes"));
    vec![verify, tests, explain, impact]
}

/// Dispatches a call. `None` means the tool does not exist.
pub(crate) async fn call(
    state: &State,
    name: &str,
    arguments: Option<JsonObject>,
) -> Option<CallToolResult> {
    let args = Value::Object(arguments.unwrap_or_default());
    let result = match name {
        "verify" => match parse::<VerifyArgs>(args) {
            Ok(a) => verify(state, a.files, a.up_to, a.budget).await,
            Err(e) => Err(e),
        },
        "run_affected_tests" => match parse::<TestArgs>(args) {
            Ok(a) => verify(state, a.files, Some(Step::Test), a.budget).await,
            Err(e) => Err(e),
        },
        "explain_error" => match parse::<ExplainArgs>(args) {
            Ok(a) => explain(&a.code),
            Err(e) => Err(e),
        },
        "impact" => match parse::<ImpactArgs>(args) {
            Ok(a) => impact(state, a.files).await,
            Err(e) => Err(e),
        },
        _ => return None,
    };
    Some(result.unwrap_or_else(|message| {
        tracing::warn!("tool `{name}` failed: {message}");
        CallToolResult::error(vec![ContentBlock::text(message)])
    }))
}

fn parse<T: for<'de> Deserialize<'de>>(args: Value) -> Result<T, String> {
    serde_json::from_value(args).map_err(|e| format!("invalid arguments: {e}"))
}

fn changes(root: &Path, files: Vec<String>) -> ChangeSet {
    ChangeSet {
        files: files
            .iter()
            .map(|f| relative_to(root, Path::new(f)))
            .collect(),
    }
}

/// Files are relative to the workspace root (not to the server's cwd);
/// absolute paths inside the root are made relative to it.
fn relative_to(root: &Path, file: &Path) -> PathBuf {
    if file.is_absolute() {
        file.strip_prefix(root).unwrap_or(file).to_path_buf()
    } else {
        file.components()
            .filter(|c| !matches!(c, Component::CurDir))
            .collect()
    }
}

fn names(crates: &[CrateId]) -> Vec<String> {
    crates.iter().map(|c| c.name.clone()).collect()
}

fn no_daemon() -> bool {
    std::env::var_os("ORQ_NO_DAEMON").is_some_and(|v| v == "1")
}

async fn verify(
    state: &State,
    files: Option<Vec<String>>,
    up_to: Option<Step>,
    budget: Option<usize>,
) -> Result<CallToolResult, String> {
    let budget = budget.unwrap_or(state.config.budget);
    let up_to = up_to.unwrap_or(state.config.up_to);
    let request = VerifyRequest {
        root: state.root.clone(),
        changes: files
            .filter(|f| !f.is_empty())
            .map(|f| changes(&state.root, f)),
        up_to,
        token_budget: budget,
        use_cache: true,
    };
    let mode = if no_daemon() {
        Mode::InProcess
    } else {
        Mode::Auto
    };
    let verified = client::verify(&request, &state.config, mode)
        .await
        .map_err(|e| format!("verify failed: {e}"))?;
    let result = verified.result;
    let report = LlmCompressor::new(state.config.lang).compress(&result, budget);
    let record = stats::StatsRecord::new(&result, &report, up_to, verified.via_daemon, "mcp");
    if let Err(e) = stats::append(&state.root, &record) {
        tracing::warn!("could not record stats: {e}");
    }
    let mut out = CallToolResult::success(vec![ContentBlock::text(report.text)]);
    out.structured_content = Some(report.json);
    Ok(out)
}

fn explain(code: &str) -> Result<CallToolResult, String> {
    let text = orq_runner::explain::explain(code, EXPLAIN_BUDGET)
        .map_err(|e| format!("explain failed: {e}"))?;
    Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
}

async fn impact(state: &State, files: Option<Vec<String>>) -> Result<CallToolResult, String> {
    let root = state.root.clone();
    let config = state.config.clone();
    let explicit = files.filter(|f| !f.is_empty());
    let set = tokio::task::spawn_blocking(move || -> Result<_, CoreError> {
        let analyzer = CargoImpactAnalyzer::new(config);
        let change_set = match explicit {
            Some(f) => changes(&root, f),
            None => analyzer.detect_changes(&root)?,
        };
        analyzer.analyze(&root, &change_set)
    })
    .await
    .map_err(|e| format!("impact failed: {e}"))?
    .map_err(|e| format!("impact failed: {e}"))?;
    let changed = names(&set.changed);
    let dependents = names(&set.dependents);
    let text = if set.full_rebuild {
        "impact: full rebuild (workspace-wide input changed)".to_string()
    } else if changed.is_empty() {
        "impact: no crate affected".to_string()
    } else {
        format!(
            "impact: changed [{}], dependents [{}]",
            changed.join(", "),
            dependents.join(", ")
        )
    };
    let mut out = CallToolResult::success(vec![ContentBlock::text(text)]);
    out.structured_content = Some(json!({
        "changed": changed,
        "dependents": dependents,
        "full_rebuild": set.full_rebuild,
    }));
    Ok(out)
}
