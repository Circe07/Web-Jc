//! Failing tests first, fail-fast and prebuild of test binaries.
#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

use std::path::Path;
use std::process::Command;

use common::{calls, cargo_wrapper, copy_fixture, single};
use orq_core::{BuildOptions, Config, NextestMode, Outcome, Severity, Step, VerifyResult};
use orq_runner::Runner;

fn runner(script: &Path) -> Runner {
    Runner::new(Config {
        nextest: NextestMode::Never,
        ..Config::default()
    })
    .with_cargo_bin(script)
}

async fn verify(runner: &Runner, root: &Path) -> VerifyResult {
    runner
        .verify(root, &single(root, "failing-test"), Step::Test)
        .await
        .expect("runner")
}

#[tokio::test]
async fn previously_failing_tests_run_first_and_fail_fast() {
    let log_dir = tempfile::tempdir().unwrap();
    let (script, log) = cargo_wrapper(log_dir.path());
    let ws = tempfile::tempdir().unwrap();
    copy_fixture("failing-test", ws.path());
    let runner = runner(&script);

    // First run: the whole suite, one failure, recorded.
    let first = verify(&runner, ws.path()).await;
    assert_eq!(first.test_failures.len(), 1);
    assert_eq!(first.test_failures[0].name, "tests::adds_wrongly");
    let test_calls = calls(&log, "test");
    assert_eq!(test_calls.len(), 1, "{test_calls:?}");
    assert!(!test_calls[0].contains("--exact"));
    let recorded = orq_core::root::target_dir(ws.path()).join("orq/last_failures.json");
    let text = std::fs::read_to_string(&recorded).expect("last_failures.json");
    assert!(text.contains("tests::adds_wrongly"), "{text}");

    // Second run: only the failing test runs, and it still fails: fail fast.
    let second = verify(&runner, ws.path()).await;
    let test_calls = calls(&log, "test");
    assert_eq!(test_calls.len(), 2, "{test_calls:?}");
    assert!(
        test_calls[1].ends_with("-- --exact tests::adds_wrongly"),
        "{}",
        test_calls[1]
    );
    assert_eq!(
        second.outcome,
        Outcome::Fail {
            failed_step: Step::Test
        }
    );
    assert_eq!(second.test_failures.len(), 1);
    assert_eq!(second.test_failures[0].name, "tests::adds_wrongly");
    let note = second
        .diagnostics
        .iter()
        .find(|d| d.severity == Severity::Note)
        .expect("a note saying the rest was not run");
    assert_eq!(note.step, Step::Test);
    assert!(note.message.contains("not run"), "{}", note.message);

    // Fix the test: the failing test passes, so the full suite runs too.
    let lib = ws.path().join("src/lib.rs");
    let source = std::fs::read_to_string(&lib).unwrap();
    std::fs::write(&lib, source.replace("add(2, 2), 5", "add(2, 2), 4")).unwrap();
    let third = verify(&runner, ws.path()).await;
    assert_eq!(third.outcome, Outcome::Pass, "{:#?}", third.diagnostics);
    assert!(third
        .diagnostics
        .iter()
        .all(|d| d.severity != Severity::Note));
    let test_calls = calls(&log, "test");
    assert_eq!(test_calls.len(), 4, "fast run + full run: {test_calls:?}");
    assert!(test_calls[2].contains("--exact"));
    assert!(!test_calls[3].contains("--exact"));
    assert!(!recorded.exists(), "no failures left to remember");

    // And now every run is a plain full run again.
    verify(&runner, ws.path()).await;
    assert_eq!(calls(&log, "test").len(), 5);
}

#[tokio::test]
async fn failing_first_can_be_disabled() {
    let log_dir = tempfile::tempdir().unwrap();
    let (script, log) = cargo_wrapper(log_dir.path());
    let ws = tempfile::tempdir().unwrap();
    copy_fixture("failing-test", ws.path());
    let runner = runner(&script).with_failing_first(false);

    verify(&runner, ws.path()).await;
    let second = verify(&runner, ws.path()).await;
    assert!(calls(&log, "test").iter().all(|c| !c.contains("--exact")));
    assert!(second
        .diagnostics
        .iter()
        .all(|d| d.severity != Severity::Note));
}

#[tokio::test]
async fn prebuild_compiles_test_binaries_for_the_next_test_step() {
    let log_dir = tempfile::tempdir().unwrap();
    let (script, log) = cargo_wrapper(log_dir.path());
    let ws = tempfile::tempdir().unwrap();
    copy_fixture("single", ws.path());
    let runner = runner(&script);
    let impact = single(ws.path(), "single");

    runner
        .prebuild_tests(ws.path(), &impact, &BuildOptions::default())
        .await
        .expect("prebuild");
    let test_calls = calls(&log, "test");
    assert_eq!(test_calls.len(), 1);
    assert!(test_calls[0].contains("--no-run"), "{}", test_calls[0]);

    // Everything the Test step needs is already built: cargo reports every
    // artifact as fresh.
    let out = Command::new("cargo")
        .args(["test", "-p", "single", "--no-fail-fast", "--no-run"])
        .arg("--message-format=json")
        .current_dir(ws.path())
        .output()
        .expect("cargo");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let artifacts: Vec<&str> = stdout
        .lines()
        .filter(|l| l.contains("\"reason\":\"compiler-artifact\""))
        .collect();
    assert!(!artifacts.is_empty());
    assert!(
        artifacts.iter().all(|l| l.contains("\"fresh\":true")),
        "stale artifacts after prebuild: {artifacts:?}"
    );

    // The Test step then does not need to compile anything.
    let result = runner
        .verify(ws.path(), &impact, Step::Test)
        .await
        .expect("runner");
    assert_eq!(result.outcome, Outcome::Pass, "{:#?}", result.diagnostics);
}

#[tokio::test]
async fn prebuild_ignores_compile_errors() {
    let ws = tempfile::tempdir().unwrap();
    copy_fixture("broken-type", ws.path());
    Runner::new(Config::default())
        .prebuild_tests(
            ws.path(),
            &single(ws.path(), "broken-type"),
            &BuildOptions::default(),
        )
        .await
        .expect("compile errors are not runner errors");
}
