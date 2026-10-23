//! What the runner reports so the agent needs fewer turns: lint findings of
//! the crates that compile next to the errors of the ones that do not, and
//! the signature of the item an error talks about.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::Path;

use orq_core::{Config, CrateId, ImpactSet, Outcome, Severity, Step, VerifyResult};
use orq_runner::Runner;

fn write(root: &Path, rel: &str, text: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn krate(root: &Path, name: &str) -> CrateId {
    CrateId {
        name: name.to_string(),
        manifest_path: root.join(name).join("Cargo.toml"),
    }
}

fn manifest(name: &str, deps: &[&str]) -> String {
    let deps: String = deps
        .iter()
        .map(|d| format!("{d} = {{ path = \"../{d}\" }}\n"))
        .collect();
    format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\n{deps}")
}

async fn verify(root: &Path, names: &[&str], up_to: Step) -> VerifyResult {
    let impact = ImpactSet {
        changed: names.iter().map(|n| krate(root, n)).collect(),
        ..ImpactSet::default()
    };
    Runner::new(Config::default())
        .verify(root, &impact, up_to)
        .await
        .expect("runner must not fail")
}

/// `bad` does not compile, `dependent` needs `bad`, `good` compiles with a
/// clippy lint.
fn broken_workspace(root: &Path) {
    write(
        root,
        "Cargo.toml",
        "[workspace]\nmembers = [\"bad\", \"dependent\", \"good\"]\nresolver = \"2\"\n",
    );
    write(root, "bad/Cargo.toml", &manifest("bad", &[]));
    write(
        root,
        "bad/src/lib.rs",
        "pub fn id() -> u64 {\n    \"not a number\"\n}\n",
    );
    write(
        root,
        "dependent/Cargo.toml",
        &manifest("dependent", &["bad"]),
    );
    write(
        root,
        "dependent/src/lib.rs",
        "pub fn twice() -> u64 {\n    bad::id() * 2\n}\n",
    );
    write(root, "good/Cargo.toml", &manifest("good", &[]));
    write(
        root,
        "good/src/lib.rs",
        "pub fn one() -> u32 {\n    return 1;\n}\n",
    );
}

#[tokio::test]
async fn clippy_lints_of_crates_that_compile_ride_along_with_check_errors() {
    let dir = tempfile::tempdir().unwrap();
    broken_workspace(dir.path());
    let result = verify(dir.path(), &["bad", "dependent", "good"], Step::Clippy).await;

    assert_eq!(
        result.outcome,
        Outcome::Fail {
            failed_step: Step::Check
        }
    );
    assert!(
        result
            .diagnostics
            .iter()
            .any(|d| d.severity == Severity::Error && d.code.as_deref() == Some("E0308")),
        "{:#?}",
        result.diagnostics
    );
    let lint = result
        .diagnostics
        .iter()
        .find(|d| d.code.as_deref() == Some("clippy::needless_return"))
        .unwrap_or_else(|| panic!("no lint from `good`: {:#?}", result.diagnostics));
    assert_eq!(lint.step, Step::Clippy);
    assert_eq!(
        lint.crate_id.as_ref().map(|c| c.name.as_str()),
        Some("good")
    );
    // Check failed, so Clippy is not reported as a step that ran.
    assert!(result.steps.iter().all(|s| s.step != Step::Clippy));
}

#[tokio::test]
async fn check_only_runs_do_not_lint() {
    let dir = tempfile::tempdir().unwrap();
    broken_workspace(dir.path());
    let result = verify(dir.path(), &["bad", "dependent", "good"], Step::Check).await;
    assert!(result
        .diagnostics
        .iter()
        .all(|d| d.step != Step::Clippy && d.severity != Severity::Warning));
}

fn signature_of<'a>(
    result: &'a VerifyResult,
    error_code: &str,
) -> Option<&'a orq_core::Diagnostic> {
    let at = result
        .diagnostics
        .iter()
        .position(|d| d.severity == Severity::Error && d.code.as_deref() == Some(error_code))?;
    result
        .diagnostics
        .get(at + 1)
        .filter(|d| d.code.as_deref() == Some("orq::signature"))
}

fn single_crate(root: &Path, source: &str) {
    write(root, "app/Cargo.toml", &manifest("app", &[]));
    write(root, "app/src/lib.rs", source);
    write(
        root,
        "Cargo.toml",
        "[workspace]\nmembers = [\"app\"]\nresolver = \"2\"\n",
    );
}

#[tokio::test]
async fn wrong_argument_count_shows_the_signature_of_the_callee() {
    let dir = tempfile::tempdir().unwrap();
    single_crate(
        dir.path(),
        "pub fn add(a: u32, b: u32) -> u32 {\n    a + b\n}\n\npub fn run() -> u32 {\n    add(1)\n}\n",
    );
    let result = verify(dir.path(), &["app"], Step::Check).await;
    let sig = signature_of(&result, "E0061")
        .unwrap_or_else(|| panic!("no signature: {:#?}", result.diagnostics));
    assert_eq!(sig.severity, Severity::Help);
    assert_eq!(sig.message, "pub fn add(a: u32, b: u32) -> u32");
    assert_eq!(sig.file.as_deref(), Some(Path::new("app/src/lib.rs")));
    assert_eq!(sig.line, Some(1));
}

#[tokio::test]
async fn unknown_struct_field_shows_the_struct() {
    let dir = tempfile::tempdir().unwrap();
    single_crate(
        dir.path(),
        "pub struct Task {\n    pub id: u32,\n    pub title: String,\n}\n\npub fn make() -> Task {\n    Task { id: 1, titel: String::new() }\n}\n",
    );
    let result = verify(dir.path(), &["app"], Step::Check).await;
    let sig = signature_of(&result, "E0560")
        .unwrap_or_else(|| panic!("no signature: {:#?}", result.diagnostics));
    assert_eq!(
        sig.message,
        "pub struct Task { pub id: u32, pub title: String }"
    );
    assert_eq!(sig.line, Some(1));
}

#[tokio::test]
async fn function_in_another_module_shows_its_signature() {
    let dir = tempfile::tempdir().unwrap();
    single_crate(
        dir.path(),
        "mod util {\n    pub fn helper(x: &str) -> usize {\n        x.len()\n    }\n}\n\npub fn run() -> usize {\n    helper(\"a\")\n}\n",
    );
    let result = verify(dir.path(), &["app"], Step::Check).await;
    let sig = signature_of(&result, "E0425")
        .unwrap_or_else(|| panic!("no signature: {:#?}", result.diagnostics));
    assert_eq!(sig.message, "pub fn helper(x: &str) -> usize");
    assert_eq!(sig.line, Some(2));
}
