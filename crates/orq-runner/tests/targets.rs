//! Target selection for check and clippy (`Targets::Auto` / `Targets::All`).
#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

use std::path::{Path, PathBuf};

use common::{calls, cargo_wrapper, fixture, krate, single};
use orq_core::{BuildOptions, Config, ImpactSet, Outcome, Step, Targets, VerifyResult};
use orq_runner::Runner;

async fn run(
    root: &Path,
    impact: &ImpactSet,
    up_to: Step,
    targets: Targets,
) -> (VerifyResult, Vec<String>) {
    let log_dir = tempfile::tempdir().unwrap();
    let (script, log) = cargo_wrapper(log_dir.path());
    let runner = Runner::new(Config::default()).with_cargo_bin(script);
    let build = BuildOptions {
        targets,
        ..BuildOptions::default()
    };
    let result = runner
        .verify_with(root, impact, up_to, &build)
        .await
        .expect("runner");
    let compile = calls(
        &log,
        if up_to >= Step::Clippy {
            "clippy"
        } else {
            "check"
        },
    );
    (result, compile)
}

fn lib_with_bench(root: &Path) {
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(root.join("benches")).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"nb\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n",
    )
    .unwrap();
    std::fs::write(root.join("src/lib.rs"), "pub fn f() {}\n").unwrap();
    // `#![feature]` is rejected by stable compilers with E0554.
    std::fs::write(
        root.join("benches/b.rs"),
        "#![feature(test)]\nextern crate test;\n",
    )
    .unwrap();
}

fn on_nightly() -> bool {
    std::process::Command::new("rustc")
        .arg("-V")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("nightly"))
        .unwrap_or(false)
}

#[tokio::test]
async fn multi_workspace_gets_per_package_target_flags() {
    let root = fixture("multi");
    let all = ImpactSet {
        changed: vec![krate(&root, "crates/base", "base")],
        dependents: vec![
            krate(&root, "crates/mid", "mid"),
            krate(&root, "crates/app", "app"),
        ],
        ..ImpactSet::default()
    };
    // `app` has a lib and a bin.
    let (result, compile) = run(&root, &all, Step::Check, Targets::Auto).await;
    assert_eq!(result.outcome, Outcome::Pass, "{:#?}", result.diagnostics);
    assert_eq!(compile.len(), 1);
    assert!(
        compile[0].contains("--lib --bins --tests"),
        "{}",
        compile[0]
    );
    assert!(!compile[0].contains("--all-targets"));

    // Libraries only: no `--bins`.
    let libs = ImpactSet {
        changed: vec![krate(&root, "crates/base", "base")],
        ..ImpactSet::default()
    };
    let (result, compile) = run(&root, &libs, Step::Clippy, Targets::Auto).await;
    assert_eq!(result.outcome, Outcome::Pass, "{:#?}", result.diagnostics);
    assert!(compile[0].contains("--lib --tests"), "{}", compile[0]);
    assert!(!compile[0].contains("--bins"));

    // `All` keeps the old behaviour.
    let (result, compile) = run(&root, &all, Step::Check, Targets::All).await;
    assert_eq!(result.outcome, Outcome::Pass);
    assert!(compile[0].contains("--all-targets"), "{}", compile[0]);
}

#[tokio::test]
async fn single_package_and_bin_only_package_work() {
    let root = fixture("single");
    let (result, compile) = run(&root, &single(&root, "single"), Step::Check, Targets::Auto).await;
    assert_eq!(result.outcome, Outcome::Pass, "{:#?}", result.diagnostics);
    assert!(compile[0].contains("--lib --tests"), "{}", compile[0]);

    // `--lib` would fail with "no library targets found" here.
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname = \"bo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n",
    )
    .unwrap();
    std::fs::write(dir.path().join("src/main.rs"), "fn main() {}\n").unwrap();
    let (result, compile) = run(
        dir.path(),
        &single(dir.path(), "bo"),
        Step::Clippy,
        Targets::Auto,
    )
    .await;
    assert_eq!(result.outcome, Outcome::Pass, "{:#?}", result.diagnostics);
    assert!(compile[0].contains("--bins --tests"), "{}", compile[0]);
    assert!(!compile[0].contains("--lib"));
}

#[tokio::test]
async fn nightly_only_bench_is_skipped_unless_it_changed() {
    if on_nightly() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    lib_with_bench(dir.path());
    let impact = single(dir.path(), "nb");

    // Auto, bench untouched: passes and does not build the bench.
    let (result, compile) = run(dir.path(), &impact, Step::Check, Targets::Auto).await;
    assert_eq!(result.outcome, Outcome::Pass, "{:#?}", result.diagnostics);
    assert!(!compile[0].contains("--benches"), "{}", compile[0]);

    // All: E0554 from the bench.
    let (result, _) = run(dir.path(), &impact, Step::Check, Targets::All).await;
    assert_eq!(
        result.outcome,
        Outcome::Fail {
            failed_step: Step::Check
        }
    );
    assert!(result
        .diagnostics
        .iter()
        .any(|d| d.code.as_deref() == Some("E0554")));

    // Auto, bench changed: it is built (and fails the same way).
    let changed = ImpactSet {
        changed_files: vec![PathBuf::from("benches/b.rs")],
        ..impact
    };
    let (result, compile) = run(dir.path(), &changed, Step::Check, Targets::Auto).await;
    assert!(compile[0].contains("--benches"), "{}", compile[0]);
    assert!(result
        .diagnostics
        .iter()
        .any(|d| d.code.as_deref() == Some("E0554")));
}

#[tokio::test]
async fn changed_examples_are_included() {
    let dir = tempfile::tempdir().unwrap();
    lib_with_bench(dir.path());
    std::fs::remove_dir_all(dir.path().join("benches")).unwrap();
    std::fs::create_dir(dir.path().join("examples")).unwrap();
    std::fs::write(dir.path().join("examples/e.rs"), "fn main() {}\n").unwrap();
    let impact = ImpactSet {
        changed_files: vec![PathBuf::from("examples/e.rs")],
        ..single(dir.path(), "nb")
    };
    let (result, compile) = run(dir.path(), &impact, Step::Check, Targets::Auto).await;
    assert_eq!(result.outcome, Outcome::Pass, "{:#?}", result.diagnostics);
    assert!(compile[0].contains("--examples"), "{}", compile[0]);
    assert!(!compile[0].contains("--benches"));
}
