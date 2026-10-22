//! End-to-end tests: run the real cargo against `fixtures/`.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::{Path, PathBuf};

use orq_core::{Config, CrateId, ImpactSet, Outcome, Severity, Step, VerifyResult};
use orq_runner::Runner;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name)
}

fn krate(root: &Path, dir: &str, name: &str) -> CrateId {
    CrateId {
        name: name.to_string(),
        manifest_path: root.join(dir).join("Cargo.toml"),
    }
}

/// Impact set made of the single package of a standalone fixture.
fn single(root: &Path, name: &str) -> ImpactSet {
    ImpactSet {
        changed: vec![krate(root, "", name)],
        ..ImpactSet::default()
    }
}

async fn verify(root: &Path, impact: &ImpactSet, up_to: Step) -> VerifyResult {
    Runner::new(Config::default())
        .verify(root, impact, up_to)
        .await
        .expect("runner must not fail")
}

/// Snapshots `result` with durations, absolute paths and test output
/// (which differs between `cargo test` and nextest) normalized.
macro_rules! assert_result_snapshot {
    ($name:expr, $result:expr) => {
        // Cargo builds `lib` and `lib test` in parallel, so their messages can
        // interleave differently from run to run: snapshot a stable order.
        let mut normalized = $result.clone();
        normalized.diagnostics.sort_by_key(|d| {
            (d.step, d.file.clone(), d.line, d.column, d.message.clone())
        });
        insta::assert_json_snapshot!($name, normalized, {
            ".total_ms" => 0,
            ".steps[].duration_ms" => 0,
            ".**.manifest_path" => "[manifest]",
            ".test_failures[].output" => "[output]",
        })
    };
}

#[tokio::test]
async fn broken_type_reports_e0308_with_location() {
    let root = fixture("broken-type");
    let result = verify(&root, &single(&root, "broken-type"), Step::Test).await;

    assert_eq!(
        result.outcome,
        Outcome::Fail {
            failed_step: Step::Check
        }
    );
    let steps: Vec<Step> = result.steps.iter().map(|s| s.step).collect();
    assert_eq!(steps, [Step::Fmt, Step::Check], "ladder must stop at check");
    assert!(!result.cache_hit);

    let diag = result
        .diagnostics
        .iter()
        .find(|d| d.code.as_deref() == Some("E0308"))
        .expect("an E0308 diagnostic");
    assert_eq!(diag.severity, Severity::Error);
    assert_eq!(diag.file.as_deref(), Some(Path::new("src/lib.rs")));
    assert_eq!((diag.line, diag.column), (Some(6), Some(19)));
    assert_eq!(
        diag.snippet.as_deref(),
        Some("    let id: u64 = req.id.clone();")
    );
    assert_eq!(
        diag.crate_id.as_ref().map(|c| c.name.as_str()),
        Some("broken-type")
    );
    assert_result_snapshot!("broken_type", result);
}

#[tokio::test]
async fn failing_test_is_reported_by_name() {
    let root = fixture("failing-test");
    let result = verify(&root, &single(&root, "failing-test"), Step::Test).await;

    assert_eq!(
        result.outcome,
        Outcome::Fail {
            failed_step: Step::Test
        }
    );
    assert_eq!(result.steps.len(), 4);
    assert!(result.steps[..3].iter().all(|s| s.passed));
    assert!(!result.steps[3].passed);
    assert_eq!(result.test_failures.len(), 1);
    let failure = &result.test_failures[0];
    assert_eq!(failure.name, "tests::adds_wrongly");
    assert_eq!(failure.crate_id.name, "failing-test");
    assert!(
        failure.output.contains("deliberate failure"),
        "output: {}",
        failure.output
    );
    assert_result_snapshot!("failing_test", result);
}

#[tokio::test]
async fn clean_multi_workspace_passes_every_step() {
    let root = fixture("multi");
    let impact = ImpactSet {
        changed: vec![krate(&root, "crates/base", "base")],
        dependents: vec![
            krate(&root, "crates/mid", "mid"),
            krate(&root, "crates/app", "app"),
        ],
        full_rebuild: false,
    };
    let result = verify(&root, &impact, Step::Test).await;

    assert_eq!(result.outcome, Outcome::Pass, "{:#?}", result.diagnostics);
    let steps: Vec<Step> = result.steps.iter().map(|s| s.step).collect();
    assert_eq!(steps, Step::ALL);
    assert!(result.steps.iter().all(|s| s.passed));
    assert!(result.test_failures.is_empty());
    assert_eq!(result.impact, impact);
    assert_result_snapshot!("multi_clean", result);
}

#[tokio::test]
async fn clippy_warnings_do_not_stop_the_ladder() {
    let root = fixture("clippy-warn");
    let result = verify(&root, &single(&root, "clippy-warn"), Step::Test).await;

    assert_eq!(result.outcome, Outcome::Pass);
    assert_eq!(result.steps.len(), 4);
    let clippy: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| d.step == Step::Clippy)
        .collect();
    assert!(!clippy.is_empty(), "expected clippy warnings");
    assert!(clippy.iter().all(|d| d.severity == Severity::Warning));
    assert!(clippy
        .iter()
        .all(|d| d.code.as_deref().is_some_and(|c| c.starts_with("clippy::"))));
    assert!(clippy.iter().all(|d| d.line.is_some() && d.file.is_some()));
    assert_result_snapshot!("clippy_warn", result);
}

#[tokio::test]
async fn cascade_yields_many_errors() {
    let root = fixture("cascade");
    let result = verify(&root, &single(&root, "cascade"), Step::Check).await;

    assert_eq!(
        result.outcome,
        Outcome::Fail {
            failed_step: Step::Check
        }
    );
    let errors: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    assert!(errors.len() >= 10, "got {} errors", errors.len());
    assert_eq!(errors[0].code.as_deref(), Some("E0432"));
    assert_eq!(errors[0].line, Some(1));
    assert!(errors
        .iter()
        .all(|d| !d.message.starts_with("aborting due to")));
    assert!(errors.iter().all(|d| d.file.is_some()));
}

#[tokio::test]
async fn empty_impact_runs_nothing() {
    // The root does not even exist: cargo must not be invoked.
    let result = verify(
        Path::new("/definitely/not/a/workspace"),
        &ImpactSet::default(),
        Step::Test,
    )
    .await;
    assert_eq!(result.outcome, Outcome::Pass);
    assert!(result.steps.is_empty());
    assert!(result.diagnostics.is_empty());
    assert!(!result.cache_hit);
}

#[tokio::test]
async fn unformatted_code_is_a_fmt_warning() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname = \"ugly\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n",
    )
    .expect("write manifest");
    std::fs::create_dir(dir.path().join("src")).expect("mkdir");
    std::fs::write(
        dir.path().join("src/lib.rs"),
        "pub fn   add(a:i32,b:i32)->i32{a+b}\n",
    )
    .expect("write source");

    let result = verify(dir.path(), &single(dir.path(), "ugly"), Step::Fmt).await;

    assert_eq!(result.outcome, Outcome::Pass);
    assert_eq!(result.steps.len(), 1);
    assert_eq!(result.diagnostics.len(), 1);
    let diag = &result.diagnostics[0];
    assert_eq!(diag.severity, Severity::Warning);
    assert_eq!(diag.step, Step::Fmt);
    assert_eq!(diag.file.as_deref(), Some(Path::new("src/lib.rs")));
    assert_eq!(diag.line, Some(1));
}
