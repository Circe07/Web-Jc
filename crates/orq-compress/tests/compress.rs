#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use orq_compress::LlmCompressor;
use orq_core::{
    estimate_tokens, Compressor, CrateId, Diagnostic, ImpactSet, Lang, Outcome, Severity, Step,
    StepReport, TestFailure, VerifyResult,
};

fn krate(name: &str) -> CrateId {
    CrateId {
        name: name.into(),
        manifest_path: PathBuf::from(format!("/ws/crates/{name}/Cargo.toml")),
    }
}

fn diag(sev: Severity, code: Option<&str>, msg: &str, file: &str, line: u32) -> Diagnostic {
    Diagnostic {
        severity: sev,
        code: code.map(Into::into),
        message: msg.into(),
        file: Some(PathBuf::from(file)),
        line: Some(line),
        column: Some(17),
        snippet: Some(format!("    let id: u64 = req.id;\n    other({line});")),
        suggestion: None,
        crate_id: Some(krate("api")),
        step: Step::Check,
    }
}

fn base(outcome: Outcome, diagnostics: Vec<Diagnostic>) -> VerifyResult {
    VerifyResult {
        outcome,
        steps: vec![StepReport {
            step: Step::Check,
            duration_ms: 1800,
            passed: false,
        }],
        diagnostics,
        test_failures: vec![],
        impact: ImpactSet {
            changed: vec![krate("api")],
            dependents: vec![],
            full_rebuild: false,
        },
        cache_hit: false,
        total_ms: 1800,
    }
}

fn fail_check(diagnostics: Vec<Diagnostic>) -> VerifyResult {
    base(
        Outcome::Fail {
            failed_step: Step::Check,
        },
        diagnostics,
    )
}

fn plan_example() -> VerifyResult {
    let mut e1 = diag(
        Severity::Error,
        Some("E0308"),
        "mismatched types",
        "crates/api/src/handler.rs",
        42,
    );
    e1.snippet = Some("    let id: u64 = req.id;".into());
    e1.suggestion = Some("req.id.parse::<u64>()?".into());
    let e2 = diag(
        Severity::Error,
        Some("E0599"),
        "no method named `foo` found",
        "crates/api/src/handler.rs",
        50,
    );
    let mut ds = vec![e1, e2];
    for i in 0..3 {
        let mut w = diag(
            Severity::Warning,
            Some("unused_variables"),
            "unused variable: `x`",
            "crates/api/src/lib.rs",
            10 + i,
        );
        w.snippet = None;
        ds.push(w);
    }
    fail_check(ds)
}

#[test]
fn snapshot_fail_small_en() {
    let r = LlmCompressor::new(Lang::En).compress(&plan_example(), 1500);
    assert!(!r.truncated);
    insta::assert_snapshot!(r.text);
    insta::assert_json_snapshot!(r.json);
}

#[test]
fn snapshot_fail_small_es() {
    let r = LlmCompressor::new(Lang::Es).compress(&plan_example(), 1500);
    insta::assert_snapshot!(r.text);
}

#[test]
fn snapshot_pass() {
    let mut r = base(Outcome::Pass, vec![]);
    r.steps = [Step::Check, Step::Clippy, Step::Test]
        .into_iter()
        .map(|step| StepReport {
            step,
            duration_ms: 1000,
            passed: true,
        })
        .collect();
    r.impact.dependents = vec![krate("cli")];
    r.impact.changed = vec![krate("api")];
    r.total_ms = 3200;
    let out = LlmCompressor::new(Lang::En).compress(&r, 1500);
    assert_eq!(out.text.lines().count(), 1);
    insta::assert_snapshot!(out.text);
    let es = LlmCompressor::new(Lang::Es).compress(&r, 1500);
    insta::assert_snapshot!(es.text);
}

#[test]
fn snapshot_pass_full_rebuild() {
    let mut r = base(Outcome::Pass, vec![]);
    r.impact.full_rebuild = true;
    r.cache_hit = true;
    let out = LlmCompressor::new(Lang::En).compress(&r, 1500);
    insta::assert_snapshot!(out.text);
}

#[test]
fn snapshot_cascade() {
    let mut ds = vec![diag(
        Severity::Error,
        Some("E0432"),
        "unresolved import `crate::missing`",
        "src/lib.rs",
        1,
    )];
    for i in 0..4 {
        ds.push(diag(
            Severity::Error,
            Some("E0425"),
            "cannot find value `v` in this scope",
            "src/lib.rs",
            10 + i,
        ));
    }
    ds.push(diag(
        Severity::Error,
        Some("E0308"),
        "mismatched types",
        "src/other.rs",
        3,
    ));
    let r = LlmCompressor::new(Lang::En).compress(&fail_check(ds), 1500);
    insta::assert_snapshot!(r.text);
}

#[test]
fn dedups_and_orders_errors_before_warnings() {
    let w = diag(Severity::Warning, Some("dead_code"), "unused", "a.rs", 1);
    let e = diag(
        Severity::Error,
        Some("E0308"),
        "mismatched types",
        "a.rs",
        2,
    );
    let r = fail_check(vec![w.clone(), e.clone(), e.clone(), w]);
    let out = LlmCompressor::new(Lang::En).compress(&r, 1500);
    assert_eq!(out.text.matches("E0308").count(), 1);
    assert_eq!(out.text.matches("dead_code").count(), 1);
    assert!(out.text.find("E0308") < out.text.find("dead_code"));
    assert!(out.text.starts_with("FAIL check (1 error, 1 warning) ·"));
}

#[test]
fn test_failures_are_reported() {
    let mut r = base(
        Outcome::Fail {
            failed_step: Step::Test,
        },
        vec![],
    );
    r.test_failures = vec![TestFailure {
        name: "tests::adds".into(),
        crate_id: krate("api"),
        output: "thread 'tests::adds' panicked at src/lib.rs:9:5:\nassertion `left == right` failed\n  left: 3\n right: 4\n".into(),
    }];
    let out = LlmCompressor::new(Lang::En).compress(&r, 1500);
    insta::assert_snapshot!(out.text);
    insta::assert_json_snapshot!(out.json);
}

#[test]
fn drops_warnings_then_errors_when_over_budget() {
    let mut ds = Vec::new();
    for i in 0..20 {
        ds.push(diag(
            Severity::Error,
            Some("E0308"),
            "mismatched types in a rather long message that takes tokens",
            "src/a.rs",
            i + 1,
        ));
    }
    for i in 0..20 {
        ds.push(diag(
            Severity::Warning,
            Some("dead_code"),
            "unused thing",
            "src/b.rs",
            i + 1,
        ));
    }
    let out = LlmCompressor::new(Lang::En).compress(&fail_check(ds), 300);
    assert!(out.truncated);
    assert!(out.estimated_tokens <= 300);
    assert!(out.text.contains("20 warnings hidden"), "{}", out.text);
    assert!(out.text.contains("more errors"), "{}", out.text);
    insta::assert_snapshot!(out.text);
}

#[test]
fn tiny_budget_hard_cuts_header() {
    let out = LlmCompressor::new(Lang::En).compress(&plan_example(), 3);
    assert!(out.truncated);
    assert!(out.estimated_tokens <= 3);
    let zero = LlmCompressor::new(Lang::En).compress(&plan_example(), 0);
    assert_eq!(zero.estimated_tokens, 0);
}

/// Deterministic pseudo-random generator so the property test needs no crate.
struct Lcg(u64);
impl Lcg {
    fn next(&mut self, n: u64) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 33) % n
    }
}

fn synthetic(rng: &mut Lcg, n_diags: usize, n_tests: usize, long: bool) -> VerifyResult {
    let codes = ["E0308", "E0599", "E0432", "E0425", "E0277", "E0382"];
    let words = "the quick brown fox jumps over lazy dog and mismatched types expected found";
    let reps = if long { 12 } else { 1 };
    let mut ds = Vec::new();
    for i in 0..n_diags {
        let is_err = rng.next(3) != 0;
        let mut d = diag(
            if is_err {
                Severity::Error
            } else {
                Severity::Warning
            },
            Some(codes[rng.next(codes.len() as u64) as usize]),
            &words.repeat(reps),
            &format!("crates/c{}/src/m{}.rs", rng.next(3), rng.next(4)),
            rng.next(200) as u32 + 1,
        );
        d.snippet = Some(
            (0..(1 + rng.next(12)))
                .map(|k| {
                    format!(
                        "    let value_{i}_{k} = compute({}, {});",
                        words.repeat(reps),
                        k
                    )
                })
                .collect::<Vec<_>>()
                .join("\n"),
        );
        d.suggestion = Some(words.repeat(reps));
        ds.push(d);
    }
    let mut r = fail_check(ds);
    r.outcome = if rng.next(2) == 0 {
        Outcome::Fail {
            failed_step: Step::Clippy,
        }
    } else {
        Outcome::Fail {
            failed_step: Step::Test,
        }
    };
    r.impact.changed = (0..rng.next(12))
        .map(|i| krate(&format!("crate_with_long_name_{i}")))
        .collect();
    r.test_failures = (0..n_tests)
        .map(|i| TestFailure {
            name: format!("module::submodule::tests::test_number_{i}"),
            crate_id: krate("api"),
            output: words.repeat(reps * 3).replace(' ', "\n"),
        })
        .collect();
    r
}

#[test]
fn never_exceeds_budget() {
    let mut rng = Lcg(42);
    for lang in [Lang::En, Lang::Es] {
        let c = LlmCompressor::new(lang);
        for budget in [200usize, 500, 1500] {
            for n in 0..60usize {
                for long in [false, true] {
                    let n_tests = if n % 5 == 0 { n / 3 } else { 0 };
                    let r = synthetic(&mut rng, n, n_tests, long);
                    let out = c.compress(&r, budget);
                    assert!(
                        out.estimated_tokens <= budget,
                        "budget {budget}, n {n}: {} tokens\n{}",
                        out.estimated_tokens,
                        out.text
                    );
                    assert_eq!(out.estimated_tokens, estimate_tokens(&out.text));
                    assert!(out.text.starts_with("FAIL"));
                }
            }
        }
    }
}

#[test]
fn header_survives_at_min_useful_budget() {
    let mut rng = Lcg(7);
    for n in [0usize, 5, 40] {
        let r = synthetic(&mut rng, n, 3, true);
        let out = LlmCompressor::new(Lang::En).compress(&r, LlmCompressor::MIN_USEFUL_BUDGET);
        assert!(out.estimated_tokens <= LlmCompressor::MIN_USEFUL_BUDGET);
        assert!(out.text.starts_with("FAIL "));
        assert!(out.text.contains('s'), "{}", out.text);
    }
}

#[test]
fn pass_never_exceeds_budget() {
    let mut r = base(Outcome::Pass, vec![]);
    r.impact.changed = (0..50)
        .map(|i| krate(&format!("a_long_crate_name_{i}")))
        .collect();
    for budget in [20usize, 200, 1500] {
        let out = LlmCompressor::new(Lang::Es).compress(&r, budget);
        assert!(out.estimated_tokens <= budget);
        assert_eq!(out.text.lines().count(), 1);
    }
}
