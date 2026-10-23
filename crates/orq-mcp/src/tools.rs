//! Tool definitions and implementations.

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use orq_compress::LlmCompressor;
use orq_core::{
    estimate_tokens, stats, BuildOptions, ChangeSet, Compressor, Config, CoreError, CrateId,
    ImpactAnalyzer, Step, VerifyRequest,
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
    fix: bool,
    #[serde(flatten)]
    build: BuildArgs,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct TestArgs {
    files: Option<Vec<String>>,
    budget: Option<usize>,
    #[serde(flatten)]
    build: BuildArgs,
}

/// Optional cargo feature/target selection shared by `verify` and
/// `run_affected_tests`.
#[derive(Deserialize, Default)]
#[serde(default)]
struct BuildArgs {
    features: Vec<String>,
    all_features: bool,
    no_default_features: bool,
    target: Option<String>,
}

impl BuildArgs {
    /// Layers the given arguments over `base`; `None` if none was given.
    fn options(self, base: &BuildOptions) -> Option<BuildOptions> {
        if self.features.is_empty()
            && !self.all_features
            && !self.no_default_features
            && self.target.is_none()
        {
            return None;
        }
        let mut build = base.clone();
        if !self.features.is_empty() {
            build.features = self.features;
        }
        build.all_features |= self.all_features;
        build.no_default_features |= self.no_default_features;
        if self.target.is_some() {
            build.target = self.target;
        }
        Some(build)
    }
}

/// JSON schema properties of the build arguments.
fn build_properties() -> Value {
    json!({
        "features": {
            "type": "array",
            "items": {"type": "string"},
            "description": "Cargo features to enable (`--features`). Omit to use the orq.toml [build] default."
        },
        "all_features": {
            "type": "boolean",
            "description": "Enable all cargo features (`--all-features`)."
        },
        "no_default_features": {
            "type": "boolean",
            "description": "Disable default cargo features (`--no-default-features`)."
        },
        "target": {
            "type": "string",
            "description": "Target triple to build for (`--target`), e.g. wasm32-unknown-unknown."
        }
    })
}

/// Properties of a tool: `base` plus the build arguments.
fn with_build(base: Value) -> Value {
    let mut props = base;
    if let (Value::Object(p), Value::Object(b)) = (&mut props, build_properties()) {
        p.extend(b);
    }
    props
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct FixArgs {
    files: Option<Vec<String>>,
    up_to: Option<Step>,
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

/// The tools, with descriptions written for the model.
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
         iterating, `clippy` for lints, `test` (default) before you finish. Pass `fix: true` \
         to apply the machine-applicable compiler and clippy suggestions first: the report \
         starts with a `fixed: ...` line naming the files orq edited (re-read them), so you \
         skip the turn that would only apply mechanical lint fixes. Pass `features`, \
         `all_features`, `no_default_features` or `target` to verify another feature set or \
         target; otherwise the orq.toml [build] defaults apply.",
        schema(json!({
            "type": "object",
            "properties": with_build(json!({
                "files": files,
                "up_to": {
                    "type": "string",
                    "enum": ["check", "clippy", "test"],
                    "description": "Last step to run. Default: from orq config (usually test)."
                },
                "budget": budget,
                "fix": {
                    "type": "boolean",
                    "description": "Apply the machine-applicable compiler/clippy fixes to the affected crates before verifying. The report says which files were edited. Default false."
                },
            })),
            "additionalProperties": false
        })),
    );
    verify.annotations = Some(titled("Verify workspace"));
    let mut tests = Tool::new(
        "run_affected_tests",
        "Run the tests of the crates affected by your changes (and their dependents), after \
         fmt, check and clippy pass. Shortcut for `verify` with up_to=test. Use it instead of \
         `cargo test` / `cargo nextest` when you want to confirm behaviour before finishing. \
         Accepts the same optional `features`, `all_features`, `no_default_features` and \
         `target` arguments as `verify`. Returns a compact report.",
        schema(json!({
            "type": "object",
            "properties": with_build(json!({"files": files, "budget": budget})),
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
    let mut fix = Tool::new(
        "apply_fix",
        "Apply the compiler and clippy suggestions that are marked machine-applicable, in one \
         call, and then verify again. Use it when a `verify` report shows `suggestion:` lines or \
         clippy lints, BEFORE editing by hand: it is cheaper and less error-prone. It only \
         applies safe, machine-applicable fixes (it never touches code that does not compile); \
         review the diff afterwards. Returns the list of fixed files plus the compact verify \
         report of the result.",
        schema(json!({
            "type": "object",
            "properties": {
                "files": files,
                "up_to": {
                    "type": "string",
                    "enum": ["check", "clippy", "test"],
                    "description": "Last step of the verification after fixing. Default: from orq config."
                },
                "budget": budget,
            },
            "additionalProperties": false
        })),
    );
    fix.annotations = Some(titled("Apply machine-applicable fixes"));
    vec![verify, tests, explain, impact, fix]
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
            Ok(a) => verify(state, a.files, a.up_to, a.budget, a.build, a.fix).await,
            Err(e) => Err(e),
        },
        "run_affected_tests" => match parse::<TestArgs>(args) {
            Ok(a) => verify(state, a.files, Some(Step::Test), a.budget, a.build, false).await,
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
        "apply_fix" => match parse::<FixArgs>(args) {
            Ok(a) => apply_fix(state, a.files, a.up_to, a.budget).await,
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
    build: BuildArgs,
    fix: bool,
) -> Result<CallToolResult, String> {
    let budget = budget.unwrap_or(state.config.budget);
    let up_to = up_to.unwrap_or(state.config.up_to);
    let build = build.options(&state.config.build);
    let mut change_set = files
        .filter(|f| !f.is_empty())
        .map(|f| changes(&state.root, f));
    // `fix`: apply the machine-applicable suggestions first, so the report
    // describes the code the agent will actually have.
    let mut fixed: Option<Vec<String>> = None;
    let mut note: Option<String> = None;
    if fix && up_to != Step::Fmt {
        let effective = build.clone().unwrap_or_else(|| state.config.build.clone());
        let (set, outcome) = fix_files(state, change_set.take(), &effective)
            .await
            .map_err(|e| format!("verify failed: {e}"))?;
        change_set = Some(set);
        fixed = Some(
            outcome
                .files_changed
                .iter()
                .map(|p| p.display().to_string())
                .collect(),
        );
        note = outcome.summary();
    }
    let report_budget = match &note {
        Some(note) => budget.saturating_sub(estimate_tokens(note) + 1),
        None => budget,
    };
    let request = VerifyRequest {
        root: state.root.clone(),
        changes: change_set,
        up_to,
        token_budget: report_budget,
        use_cache: true,
        build,
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
    let report = LlmCompressor::new(state.config.lang).compress(&result, report_budget);
    let record = stats::StatsRecord::new(&result, &report, up_to, verified.via_daemon, "mcp");
    if let Err(e) = stats::append(&state.root, &record) {
        tracing::warn!("could not record stats: {e}");
    }
    let text = match &note {
        Some(note) => format!("{note}\n{}", report.text.trim_end()),
        None => report.text,
    };
    let mut out = CallToolResult::success(vec![ContentBlock::text(text)]);
    let mut json = report.json;
    if let (Some(fixed), Value::Object(map)) = (fixed, &mut json) {
        map.insert("fixed".to_string(), json!(fixed));
    }
    out.structured_content = Some(json);
    Ok(out)
}

/// Detects (or takes) the changed files, analyzes their impact and applies
/// the machine-applicable fixes to the affected crates. Returns the change
/// set, so the verification that follows covers the same crates.
async fn fix_files(
    state: &State,
    explicit: Option<ChangeSet>,
    build: &BuildOptions,
) -> Result<(ChangeSet, orq_runner::fix::FixOutcome), String> {
    let root = state.root.clone();
    let config = state.config.clone();
    let (change_set, impact) = tokio::task::spawn_blocking(move || -> Result<_, CoreError> {
        let analyzer = CargoImpactAnalyzer::new(config);
        let change_set = match explicit {
            Some(set) => set,
            None => analyzer.detect_changes(&root)?,
        };
        let impact = analyzer.analyze(&root, &change_set)?;
        Ok((change_set, impact))
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;
    let outcome = orq_runner::fix::apply_fixes(&state.root, &impact, build)
        .await
        .map_err(|e| e.to_string())?;
    Ok((change_set, outcome))
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

async fn apply_fix(
    state: &State,
    files: Option<Vec<String>>,
    up_to: Option<Step>,
    budget: Option<usize>,
) -> Result<CallToolResult, String> {
    let budget = budget.unwrap_or(state.config.budget);
    let up_to = up_to.unwrap_or(state.config.up_to);
    let explicit = files
        .filter(|f| !f.is_empty())
        .map(|f| changes(&state.root, f));
    let (change_set, outcome) = fix_files(state, explicit, &state.config.build)
        .await
        .map_err(|e| format!("apply_fix failed: {e}"))?;
    let fixed: Vec<String> = outcome
        .files_changed
        .iter()
        .map(|p| p.display().to_string())
        .collect();

    let request = VerifyRequest {
        root: state.root.clone(),
        changes: Some(change_set),
        up_to,
        token_budget: budget,
        use_cache: true,
        build: None,
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
    let record = stats::StatsRecord::new(&result, &report, up_to, verified.via_daemon, "fix");
    if let Err(e) = stats::append(&state.root, &record) {
        tracing::warn!("could not record stats: {e}");
    }

    let mut summary = format!("fixed {} file(s)", fixed.len());
    if !fixed.is_empty() {
        summary.push_str(": ");
        summary.push_str(&fixed.join(", "));
    }
    if !outcome.cargo_ok && fixed.is_empty() {
        summary.push_str(" (cargo could not apply fixes; the code may not compile)");
    }
    let mut out = CallToolResult::success(vec![ContentBlock::text(format!(
        "{summary}\n{}",
        report.text.trim_end()
    ))]);
    out.structured_content = Some(json!({"fixed": fixed, "report": report.json}));
    Ok(out)
}
