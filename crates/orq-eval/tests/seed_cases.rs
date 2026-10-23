//! Runs `orq-eval run` on the two seed cases.

use std::path::PathBuf;
use std::process::Command;

fn cases_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../eval/cases")
}

#[test]
fn runs_the_seed_cases() {
    let out = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_orq-eval"))
        .arg("run")
        .arg("--cases")
        .arg(cases_dir())
        .arg("--out")
        .arg(out.path())
        .args(["--budgets", "200,1500"])
        .args([
            "--case",
            "type-mismatch-parse",
            "--case",
            "failing-test-add",
        ])
        .args(["--min-recall", "1.0"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(out.path().join("offline.json")).unwrap())
            .unwrap();
    let cases = json["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 2);
    for case in cases {
        assert_eq!(case["fail_step_correct"], true, "{case}");
        let modes: Vec<&str> = case["modes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["mode"].as_str().unwrap())
            .collect();
        assert_eq!(modes, ["raw-step", "raw-ladder", "orq@200", "orq@1500"]);
        // The raw output of the failing step holds every fact.
        assert_eq!(case["modes"][0]["fact_recall"], 1.0, "{case}");
    }
    let md = std::fs::read_to_string(out.path().join("offline.md")).unwrap();
    assert!(md.contains("## Summary") && md.contains("## Facts orq lost"));
    assert!(out
        .path()
        .join("reports/type-mismatch-parse/orq@1500.txt")
        .is_file());
}

#[test]
fn unknown_case_is_an_error() {
    let out = tempfile::tempdir().unwrap();
    let status = Command::new(env!("CARGO_BIN_EXE_orq-eval"))
        .arg("run")
        .arg("--cases")
        .arg(cases_dir())
        .arg("--out")
        .arg(out.path())
        .args(["--case", "no-such-case"])
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(2));
}
