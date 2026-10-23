//! The Test step runs only the tests a change can affect: an edit to one
//! integration test file runs that test target alone.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::{Path, PathBuf};

use orq_core::{Config, CrateId, ImpactSet, Outcome, Step, VerifyResult};
use orq_runner::Runner;

fn write(root: &Path, rel: &str, text: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// One crate, `app`, with a passing `tests/good.rs` and a failing
/// `tests/bad.rs`.
fn workspace(root: &Path) -> CrateId {
    write(
        root,
        "Cargo.toml",
        "[workspace]\nmembers = [\"app\"]\nresolver = \"2\"\n",
    );
    write(
        root,
        "app/Cargo.toml",
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write(root, "app/src/lib.rs", "pub fn two() -> u32 {\n    2\n}\n");
    write(
        root,
        "app/tests/good.rs",
        "#[test]\nfn two_is_two() {\n    assert_eq!(app::two(), 2);\n}\n",
    );
    write(
        root,
        "app/tests/bad.rs",
        "#[test]\nfn two_is_three() {\n    assert_eq!(app::two(), 3);\n}\n",
    );
    CrateId {
        name: "app".into(),
        manifest_path: root.join("app/Cargo.toml"),
    }
}

async fn verify(root: &Path, krate: CrateId, changed: &str) -> VerifyResult {
    let impact = ImpactSet {
        changed: vec![krate],
        changed_files: vec![PathBuf::from(changed)],
        ..ImpactSet::default()
    };
    Runner::new(Config::default())
        .with_failing_first(false)
        .verify(root, &impact, Step::Test)
        .await
        .expect("runner must not fail")
}

#[tokio::test]
async fn editing_one_integration_test_runs_only_that_target() {
    let dir = tempfile::tempdir().unwrap();
    let krate = workspace(dir.path());
    let result = verify(dir.path(), krate, "app/tests/good.rs").await;
    assert_eq!(result.outcome, Outcome::Pass, "{result:#?}");
}

#[tokio::test]
async fn editing_the_library_runs_every_test_of_the_crate() {
    let dir = tempfile::tempdir().unwrap();
    let krate = workspace(dir.path());
    let result = verify(dir.path(), krate, "app/src/lib.rs").await;
    assert!(
        matches!(
            result.outcome,
            Outcome::Fail {
                failed_step: Step::Test
            }
        ),
        "{result:#?}"
    );
    assert!(result
        .test_failures
        .iter()
        .any(|f| f.name.contains("two_is_three")));
}
