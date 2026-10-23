//! Running the cases and measuring each mode.

use std::path::{Path, PathBuf};

use orq_cache::RedbCache;
use orq_compress::LlmCompressor;
use orq_core::{
    estimate_tokens, ChangeSet, Config, NextestMode, Outcome, Pipeline, Step, VerifyRequest,
};
use orq_impact::CargoImpactAnalyzer;
use orq_runner::Runner;
use serde::Serialize;

use crate::capture;
use crate::facts::{first_error_correct, recall, CaseSpec};
use crate::EvalError;

/// Mode name of the raw output of the failing step.
pub const RAW_STEP: &str = "raw-step";
/// Mode name of the raw output of `cargo build && cargo test`.
pub const RAW_LADDER: &str = "raw-ladder";

/// Mode name of orq's report at `budget`.
pub fn orq_mode(budget: usize) -> String {
    format!("orq@{budget}")
}

/// Measurements of one mode (raw output or orq at one budget) on one case.
#[derive(Debug, Clone, Serialize)]
pub struct ModeResult {
    /// `raw-step`, `raw-ladder` or `orq@<budget>`.
    pub mode: String,
    /// Estimated tokens of the text.
    pub tokens: usize,
    /// Fraction of facts found.
    pub fact_recall: f64,
    /// Facts found.
    pub facts_found: usize,
    /// Facts in the case.
    pub facts_total: usize,
    /// The facts not found, with the missing parts.
    pub missing: Vec<String>,
    /// Whether the first error shown is the first fact with a code; `None`
    /// when no fact has a code.
    pub first_error_correct: Option<bool>,
    /// For orq modes: whether the report dropped content to fit the budget.
    pub truncated: Option<bool>,
}

/// A fact present in the raw output that orq's report lost.
#[derive(Debug, Clone, Serialize)]
pub struct LostFact {
    /// The orq mode (`orq@<budget>`).
    pub mode: String,
    /// The fact and what was missing.
    pub fact: String,
}

/// Everything measured on one case.
#[derive(Debug, Clone, Serialize)]
pub struct CaseResult {
    /// Case id.
    pub id: String,
    /// Case category.
    pub category: String,
    /// Case difficulty.
    pub difficulty: String,
    /// Step where the project is expected to fail.
    pub expected_fail_step: String,
    /// `pass` or `fail`, as orq saw it.
    pub orq_outcome: String,
    /// The step orq reported as failed, if any.
    pub orq_failed_step: Option<String>,
    /// `orq_failed_step == expected_fail_step`.
    pub fail_step_correct: bool,
    /// One entry per mode: raw-step, raw-ladder, then orq per budget.
    pub modes: Vec<ModeResult>,
    /// Facts the raw output of the failing step had and orq lost.
    pub lost_facts: Vec<LostFact>,
}

/// Raw texts behind a [`CaseResult`], kept for inspection.
#[derive(Debug, Clone)]
pub struct CaseTexts {
    /// `(mode, text)` in the same order as [`CaseResult::modes`].
    pub texts: Vec<(String, String)>,
}

/// Options of a run.
#[derive(Debug, Clone)]
pub struct RunOptions {
    /// Directory holding the cases.
    pub cases: PathBuf,
    /// Token budgets to compress at.
    pub budgets: Vec<usize>,
    /// Only these case ids (all when empty).
    pub only: Vec<String>,
}

/// Case ids under `dir` (directories with a `case.toml`), sorted, filtered by
/// `only` when it is not empty.
pub fn list_cases(dir: &Path, only: &[String]) -> Result<Vec<String>, EvalError> {
    let mut ids = Vec::new();
    for entry in std::fs::read_dir(dir).map_err(|e| EvalError::io(dir, e))? {
        let entry = entry.map_err(|e| EvalError::io(dir, e))?;
        if entry.path().join("case.toml").is_file() {
            ids.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    ids.sort();
    if !only.is_empty() {
        if let Some(missing) = only.iter().find(|id| !ids.contains(id)) {
            return Err(EvalError::Run(format!(
                "case `{missing}` not found in {}",
                dir.display()
            )));
        }
        ids.retain(|id| only.contains(id));
    }
    Ok(ids)
}

/// Copies `from` to `to` recursively, skipping `target/` directories.
pub fn copy_dir(from: &Path, to: &Path) -> Result<(), EvalError> {
    std::fs::create_dir_all(to).map_err(|e| EvalError::io(to, e))?;
    for entry in std::fs::read_dir(from).map_err(|e| EvalError::io(from, e))? {
        let entry = entry.map_err(|e| EvalError::io(from, e))?;
        let path = entry.path();
        let dest = to.join(entry.file_name());
        let kind = entry.file_type().map_err(|e| EvalError::io(&path, e))?;
        if kind.is_dir() {
            if entry.file_name() != "target" {
                copy_dir(&path, &dest)?;
            }
        } else if kind.is_file() {
            std::fs::copy(&path, &dest).map_err(|e| EvalError::io(&path, e))?;
        }
    }
    Ok(())
}

/// Every `.rs` file under `root`, relative to it, skipping `target/` and
/// hidden directories.
pub fn rust_files(root: &Path) -> Result<Vec<PathBuf>, EvalError> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), EvalError> {
        for entry in std::fs::read_dir(dir).map_err(|e| EvalError::io(dir, e))? {
            let entry = entry.map_err(|e| EvalError::io(dir, e))?;
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            let kind = entry.file_type().map_err(|e| EvalError::io(&path, e))?;
            if kind.is_dir() {
                if name != "target" && !name.starts_with('.') {
                    walk(root, &path, out)?;
                }
            } else if name.ends_with(".rs") {
                if let Ok(rel) = path.strip_prefix(root) {
                    out.push(rel.to_path_buf());
                }
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(root, root, &mut out)?;
    out.sort();
    Ok(out)
}

fn measure(mode: &str, spec: &CaseSpec, text: &str, truncated: Option<bool>) -> ModeResult {
    let r = recall(&spec.facts, text);
    ModeResult {
        mode: mode.to_string(),
        tokens: estimate_tokens(text),
        fact_recall: r.ratio(),
        facts_found: r.found,
        facts_total: r.total,
        missing: r.missing,
        first_error_correct: first_error_correct(&spec.facts, text),
        truncated,
    }
}

/// Runs one case: `case_dir` holds `case.toml` and `project/`.
pub async fn run_case(
    case_dir: &Path,
    budgets: &[usize],
) -> Result<(CaseResult, CaseTexts), EvalError> {
    let spec = CaseSpec::load(&case_dir.join("case.toml"))?;
    let up_to: Step = spec
        .up_to
        .parse()
        .map_err(|e| EvalError::Run(format!("case {}: {e}", spec.id)))?;
    let work = tempfile::tempdir().map_err(|e| EvalError::io(Path::new("<tempdir>"), e))?;
    let project = work.path().join("project");
    copy_dir(&case_dir.join("project"), &project)?;

    let raw_step = capture::raw_step(&project, &spec.expected_fail_step)?;
    let raw_ladder = capture::raw_ladder(&project)?;

    let config = Config {
        nextest: NextestMode::Never,
        up_to,
        ..Config::default()
    };
    let pipeline: Pipeline<_, _, RedbCache, _> = Pipeline {
        analyzer: CargoImpactAnalyzer::new(config.clone()),
        runner: Runner::new(config.clone()).with_failing_first(false),
        cache: None,
        compressor: LlmCompressor::new(config.lang),
        build: config.build.clone(),
    };
    let request = VerifyRequest {
        root: project.clone(),
        changes: Some(ChangeSet {
            files: rust_files(&project)?,
        }),
        up_to,
        token_budget: config.budget,
        use_cache: false,
        build: None,
    };
    let result = pipeline
        .verify(&request)
        .await
        .map_err(|e| EvalError::Run(format!("case {}: orq failed: {e}", spec.id)))?;
    let failed_step = match result.outcome {
        Outcome::Pass => None,
        Outcome::Fail { failed_step } => Some(failed_step.to_string()),
    };

    let mut modes = vec![
        measure(RAW_STEP, &spec, &raw_step, None),
        measure(RAW_LADDER, &spec, &raw_ladder, None),
    ];
    let mut texts = vec![
        (RAW_STEP.to_string(), raw_step.clone()),
        (RAW_LADDER.to_string(), raw_ladder),
    ];
    let mut lost_facts = Vec::new();
    for &budget in budgets {
        let compact = pipeline.report(&result, budget);
        let mode = orq_mode(budget);
        for fact in &spec.facts {
            if fact.found_in(&raw_step) && !fact.found_in(&compact.text) {
                lost_facts.push(LostFact {
                    mode: mode.clone(),
                    fact: format!(
                        "{} (missing {})",
                        fact.describe(),
                        fact.missing_parts(&compact.text).join(", ")
                    ),
                });
            }
        }
        modes.push(measure(
            &mode,
            &spec,
            &compact.text,
            Some(compact.truncated),
        ));
        texts.push((mode, compact.text));
    }

    let case = CaseResult {
        id: spec.id.clone(),
        category: spec.category.clone(),
        difficulty: spec.difficulty.clone(),
        expected_fail_step: spec.expected_fail_step.clone(),
        orq_outcome: if failed_step.is_some() {
            "fail"
        } else {
            "pass"
        }
        .to_string(),
        fail_step_correct: failed_step.as_deref() == Some(spec.expected_fail_step.as_str()),
        orq_failed_step: failed_step,
        modes,
        lost_facts,
    };
    Ok((case, CaseTexts { texts }))
}
