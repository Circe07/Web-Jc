//! Tests for build options and for the single-compile optimization.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::Path;

use orq_core::{BuildOptions, Config, CrateId, ImpactSet, Outcome, Severity, Step};
use orq_runner::Runner;

fn crate_in(root: &Path, name: &str) -> ImpactSet {
    ImpactSet {
        changed: vec![CrateId {
            name: name.to_string(),
            manifest_path: root.join("Cargo.toml"),
        }],
        ..ImpactSet::default()
    }
}

fn write_crate(root: &Path, manifest_extra: &str, lib: &str) {
    std::fs::write(
        root.join("Cargo.toml"),
        format!(
            "[package]\nname = \"feat\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n{manifest_extra}\n[workspace]\n"
        ),
    )
    .expect("manifest");
    std::fs::create_dir_all(root.join("src")).expect("mkdir");
    std::fs::write(root.join("src/lib.rs"), lib).expect("lib");
}

#[tokio::test]
async fn features_reach_the_compiler() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_crate(
        dir.path(),
        "[features]\nx = []\n",
        "#[cfg(feature = \"x\")]\ncompile_error!(\"x on\");\n",
    );
    let impact = crate_in(dir.path(), "feat");
    let runner = Runner::new(Config::default());

    let plain = runner
        .verify_with(dir.path(), &impact, Step::Clippy, &BuildOptions::default())
        .await
        .expect("runner");
    assert_eq!(plain.outcome, Outcome::Pass, "{:#?}", plain.diagnostics);

    let build = BuildOptions {
        features: vec!["x".to_string()],
        ..BuildOptions::default()
    };
    for up_to in [Step::Check, Step::Clippy] {
        let with = runner
            .verify_with(dir.path(), &impact, up_to, &build)
            .await
            .expect("runner");
        assert_eq!(
            with.outcome,
            Outcome::Fail {
                failed_step: Step::Check
            }
        );
        assert!(
            with.diagnostics
                .iter()
                .any(|d| d.severity == Severity::Error && d.message.contains("x on")),
            "{:#?}",
            with.diagnostics
        );
    }
}

#[tokio::test]
async fn missing_target_surfaces_cargo_stderr() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_crate(dir.path(), "", "pub fn f() {}\n");
    let build = BuildOptions {
        target: Some("orq-not-a-real-target".to_string()),
        ..BuildOptions::default()
    };
    let result = Runner::new(Config::default())
        .verify_with(
            dir.path(),
            &crate_in(dir.path(), "feat"),
            Step::Check,
            &build,
        )
        .await
        .expect("runner");
    assert_eq!(
        result.outcome,
        Outcome::Fail {
            failed_step: Step::Check
        }
    );
    assert!(result
        .diagnostics
        .iter()
        .any(|d| d.severity == Severity::Error && d.step == Step::Check));
}

/// A cargo wrapper logs the subcommand of every invocation, then runs the
/// real cargo: up to `Test` there must be exactly one `clippy` and no `check`.
#[cfg(unix)]
#[tokio::test]
async fn only_one_compile_for_check_and_clippy() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().expect("tempdir");
    write_crate(dir.path(), "", "pub fn f() {}\n");
    let log = dir.path().join("invocations.log");
    let wrapper = dir.path().join("cargo-wrapper.sh");
    let real = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    std::fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\necho \"$1\" >> '{}'\nexec '{}' \"$@\"\n",
            log.display(),
            real
        ),
    )
    .expect("wrapper");
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).expect("chmod");

    let result = Runner::new(Config::default())
        .with_cargo_bin(&wrapper)
        .verify_with(
            dir.path(),
            &crate_in(dir.path(), "feat"),
            Step::Test,
            &BuildOptions::default(),
        )
        .await
        .expect("runner");
    assert_eq!(result.outcome, Outcome::Pass, "{:#?}", result.diagnostics);
    let steps: Vec<Step> = result.steps.iter().map(|s| s.step).collect();
    assert_eq!(steps, Step::ALL);
    assert_eq!(result.steps[2].duration_ms, 0);

    let invoked = std::fs::read_to_string(&log).expect("log");
    let count = |sub: &str| invoked.lines().filter(|l| *l == sub).count();
    assert_eq!(count("clippy"), 1, "{invoked}");
    assert_eq!(count("check"), 0, "{invoked}");
    assert_eq!(count("fmt"), 1, "{invoked}");
}
