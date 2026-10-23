//! `apply_fixes` against copies of the fixtures.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::{Path, PathBuf};

use orq_core::{BuildOptions, Config, CrateId, ImpactSet, Step};
use orq_runner::fix::apply_fixes;
use orq_runner::Runner;

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name();
        if name == "target" {
            continue;
        }
        let dest = to.join(&name);
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &dest);
        } else {
            std::fs::copy(entry.path(), dest).unwrap();
        }
    }
}

fn fixture(name: &str) -> tempfile::TempDir {
    let src = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name);
    let dir = tempfile::tempdir().unwrap();
    copy_dir(&src, dir.path());
    dir
}

fn krate(root: &Path, dir: &str, name: &str) -> CrateId {
    CrateId {
        name: name.to_string(),
        manifest_path: root.join(dir).join("Cargo.toml"),
    }
}

fn clippy_count(result: &orq_core::VerifyResult) -> usize {
    result
        .diagnostics
        .iter()
        .filter(|d| d.step == Step::Clippy)
        .count()
}

#[tokio::test]
async fn fixes_machine_applicable_clippy_lints() {
    let dir = fixture("clippy-warn");
    let root = dir.path();
    let impact = ImpactSet {
        changed: vec![krate(root, "", "clippy-warn")],
        ..ImpactSet::default()
    };
    let runner = Runner::new(Config::default());
    let before = runner.verify(root, &impact, Step::Clippy).await.unwrap();
    assert!(clippy_count(&before) > 0);

    let outcome = apply_fixes(root, &impact, &BuildOptions::default())
        .await
        .unwrap();
    assert!(outcome.cargo_ok);
    assert_eq!(outcome.files_changed, [PathBuf::from("src/lib.rs")]);

    let after = runner.verify(root, &impact, Step::Clippy).await.unwrap();
    assert!(
        clippy_count(&after) < clippy_count(&before),
        "{} -> {}",
        clippy_count(&before),
        clippy_count(&after)
    );
    let source = std::fs::read_to_string(root.join("src/lib.rs")).unwrap();
    assert!(!source.contains("return v.len() == 0"), "{source}");
}

#[tokio::test]
async fn clean_workspace_changes_nothing() {
    let dir = fixture("multi");
    let root = dir.path();
    let impact = ImpactSet {
        changed: vec![krate(root, "crates/base", "base")],
        dependents: vec![
            krate(root, "crates/mid", "mid"),
            krate(root, "crates/app", "app"),
            krate(root, "crates/leaf", "leaf"),
        ],
        ..ImpactSet::default()
    };
    let outcome = apply_fixes(root, &impact, &BuildOptions::default())
        .await
        .unwrap();
    assert!(outcome.files_changed.is_empty(), "{outcome:?}");
}

#[tokio::test]
async fn empty_impact_is_a_no_op() {
    let dir = fixture("multi");
    let outcome = apply_fixes(dir.path(), &ImpactSet::default(), &BuildOptions::default())
        .await
        .unwrap();
    assert!(outcome.files_changed.is_empty() && outcome.cargo_ok);
}
