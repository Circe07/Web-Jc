//! A workspace member whose name is shared with a registry package in the
//! graph (tokio and serde hit this) must still be selectable: `-p <name>` is
//! ambiguous there, so the runner passes `path+file://...#name` specs.
//!
//! Needs crates.io. The test is skipped (passes) when the registry cannot be
//! reached, so offline runs stay green.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::Path;

use orq_core::{BuildOptions, Config, CrateId, ImpactSet, Outcome, Step};
use orq_runner::Runner;

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(path, text).expect("write");
}

#[tokio::test]
async fn member_named_like_a_registry_dependency_is_not_ambiguous() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write(
        &root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"itoa\", \"app\"]\nresolver = \"2\"\n",
    );
    // Workspace member named like the registry crate, at another version.
    write(
        &root.join("itoa/Cargo.toml"),
        "[package]\nname = \"itoa\"\nversion = \"0.0.1\"\nedition = \"2021\"\n",
    );
    write(&root.join("itoa/src/lib.rs"), "pub fn one() -> u8 { 1 }\n");
    write(
        &root.join("app/Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
         [dependencies]\nitoa = \"1\"\n",
    );
    write(
        &root.join("app/src/lib.rs"),
        "pub fn s() -> String { itoa::Buffer::new().format(7).to_string() }\n",
    );
    // Resolve (and download) first; skip when offline.
    let fetched = std::process::Command::new(std::env::var("CARGO").unwrap_or("cargo".into()))
        .args(["fetch", "--quiet"])
        .current_dir(root)
        .status();
    if !fetched.is_ok_and(|s| s.success()) {
        eprintln!("skipping: crates.io not reachable");
        return;
    }

    let root = root.canonicalize().expect("canonical");
    // Selecting `app` (which pulls the registry itoa) together with the
    // member `itoa` is what makes `-p itoa` ambiguous.
    let krate = |name: &str| CrateId {
        name: name.into(),
        manifest_path: root.join(name).join("Cargo.toml"),
    };
    let impact = ImpactSet {
        changed: vec![krate("app"), krate("itoa")],
        ..ImpactSet::default()
    };
    let result = Runner::new(Config::default())
        .verify_with(&root, &impact, Step::Clippy, &BuildOptions::default())
        .await
        .expect("runner");
    assert_eq!(result.outcome, Outcome::Pass, "{:#?}", result.diagnostics);
}
