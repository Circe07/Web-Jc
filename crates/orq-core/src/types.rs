use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::{CoreError, DEFAULT_TOKEN_BUDGET};

/// Absolute path to the root of the workspace being verified.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct WorkspaceRoot(pub PathBuf);

/// Identifies one package of the workspace.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CrateId {
    /// Package name as written in its `Cargo.toml` (what `cargo -p` takes).
    pub name: String,
    /// Path to the package's `Cargo.toml`, as reported by `cargo metadata`
    /// (absolute).
    pub manifest_path: PathBuf,
}

/// Set of changed files.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChangeSet {
    /// Paths relative to the workspace root.
    pub files: Vec<PathBuf>,
}

/// Crates affected by a [`ChangeSet`].
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImpactSet {
    /// Crates that contain changed files.
    pub changed: Vec<CrateId>,
    /// Transitive reverse dependents of `changed`, excluding `changed` itself.
    pub dependents: Vec<CrateId>,
    /// `true` if a workspace-wide input changed (root `Cargo.toml`,
    /// `Cargo.lock`, a `build.rs`, the toolchain, `.cargo/config*`...), in
    /// which case the whole workspace must be verified.
    pub full_rebuild: bool,
}

impl ImpactSet {
    /// `changed ∪ dependents`, sorted and without duplicates.
    pub fn affected(&self) -> Vec<CrateId> {
        let mut all: Vec<CrateId> = self
            .changed
            .iter()
            .chain(&self.dependents)
            .cloned()
            .collect();
        all.sort();
        all.dedup();
        all
    }

    /// `true` if nothing needs to be verified.
    pub fn is_empty(&self) -> bool {
        !self.full_rebuild && self.changed.is_empty() && self.dependents.is_empty()
    }
}

/// One rung of the verification ladder, in execution order.
#[derive(
    Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord, Default,
)]
#[serde(rename_all = "lowercase")]
pub enum Step {
    /// `cargo fmt --check`.
    Fmt,
    /// `cargo check`.
    Check,
    /// `cargo clippy`.
    Clippy,
    /// Tests (`cargo nextest run` or `cargo test`).
    #[default]
    Test,
}

impl Step {
    /// Every step, in ladder order.
    pub const ALL: [Step; 4] = [Step::Fmt, Step::Check, Step::Clippy, Step::Test];

    /// The steps to run when verifying up to and including `self`.
    pub fn ladder(self) -> impl Iterator<Item = Step> {
        Self::ALL.into_iter().filter(move |s| *s <= self)
    }

    /// Lowercase name, as used in config, CLI and JSON.
    pub fn as_str(self) -> &'static str {
        match self {
            Step::Fmt => "fmt",
            Step::Check => "check",
            Step::Clippy => "clippy",
            Step::Test => "test",
        }
    }
}

impl fmt::Display for Step {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Step {
    type Err = CoreError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Step::ALL
            .into_iter()
            .find(|step| step.as_str().eq_ignore_ascii_case(s))
            .ok_or_else(|| CoreError::Parse(format!("unknown step `{s}`")))
    }
}

/// Diagnostic severity, mirroring rustc's levels.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Stops the ladder.
    Error,
    /// Reported but does not stop the ladder.
    Warning,
    /// Extra information.
    Note,
    /// Extra help.
    Help,
}

/// One compiler, clippy or rustfmt diagnostic.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Diagnostic {
    /// Severity level.
    pub severity: Severity,
    /// Error or lint code, e.g. `"E0308"` or `"clippy::needless_return"`.
    pub code: Option<String>,
    /// Main message.
    pub message: String,
    /// File of the primary span, relative to the workspace root when possible.
    pub file: Option<PathBuf>,
    /// 1-based line of the primary span.
    pub line: Option<u32>,
    /// 1-based column of the primary span.
    pub column: Option<u32>,
    /// Relevant source lines (plain text, no ANSI escapes), starting at
    /// `line`.
    pub snippet: Option<String>,
    /// Label of the primary span, e.g. "expected `u64`, found `String`".
    #[serde(default)]
    pub label: Option<String>,
    /// Suggested replacement from rustc/clippy, if any.
    pub suggestion: Option<String>,
    /// Crate the diagnostic was emitted for.
    pub crate_id: Option<CrateId>,
    /// Step that produced it.
    pub step: Step,
}

/// A failed test.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TestFailure {
    /// Full test name, e.g. `tests::adds`.
    pub name: String,
    /// Crate the test belongs to.
    pub crate_id: CrateId,
    /// Captured output (panic message, assertion diff...).
    pub output: String,
}

/// Overall result of a verification.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum Outcome {
    /// Every step up to `up_to` passed.
    Pass,
    /// The ladder stopped at `failed_step`.
    Fail {
        /// First step that produced errors.
        failed_step: Step,
    },
}

impl Outcome {
    /// `true` for [`Outcome::Pass`].
    pub fn is_pass(self) -> bool {
        matches!(self, Outcome::Pass)
    }
}

/// Timing and result of one executed step.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StepReport {
    /// Which step.
    pub step: Step,
    /// Wall-clock duration in milliseconds.
    pub duration_ms: u64,
    /// `false` if the step produced errors (warnings do not count).
    pub passed: bool,
}

/// Full, uncompressed result of a verification.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VerifyResult {
    /// Pass or the failing step.
    pub outcome: Outcome,
    /// Steps that were executed, in order.
    pub steps: Vec<StepReport>,
    /// All diagnostics, in the order they were emitted.
    pub diagnostics: Vec<Diagnostic>,
    /// Failed tests (only when the Test step ran).
    pub test_failures: Vec<TestFailure>,
    /// What was verified.
    pub impact: ImpactSet,
    /// `true` if this result came from the cache.
    pub cache_hit: bool,
    /// Total wall-clock time in milliseconds.
    pub total_ms: u64,
}

/// Cargo feature and target selection, applied to every step of the ladder.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(default, deny_unknown_fields)]
pub struct BuildOptions {
    /// `--features a,b` (empty = none).
    pub features: Vec<String>,
    /// `--all-features`.
    pub all_features: bool,
    /// `--no-default-features`.
    pub no_default_features: bool,
    /// `--target <triple>`.
    pub target: Option<String>,
}

impl BuildOptions {
    /// The cargo arguments these options translate to, in a stable order.
    pub fn cargo_args(&self) -> Vec<String> {
        let mut args = Vec::new();
        if self.all_features {
            args.push("--all-features".to_string());
        }
        if self.no_default_features {
            args.push("--no-default-features".to_string());
        }
        if !self.features.is_empty() {
            args.push("--features".to_string());
            args.push(self.features.join(","));
        }
        if let Some(target) = &self.target {
            args.push("--target".to_string());
            args.push(target.clone());
        }
        args
    }
}

/// Request to verify a workspace (CLI, daemon IPC and MCP all use it).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct VerifyRequest {
    /// Workspace root.
    pub root: PathBuf,
    /// Changed files; `None` means detect them with `git diff`.
    pub changes: Option<ChangeSet>,
    /// Last step to run.
    pub up_to: Step,
    /// Token budget for the compact report.
    pub token_budget: usize,
    /// Whether to read and write the result cache.
    pub use_cache: bool,
    /// Feature/target selection; `None` uses the defaults of whoever runs
    /// the pipeline (`Config::build`).
    pub build: Option<BuildOptions>,
}

impl Default for VerifyRequest {
    fn default() -> Self {
        Self {
            root: PathBuf::from("."),
            changes: None,
            up_to: Step::Test,
            token_budget: DEFAULT_TOKEN_BUDGET,
            use_cache: true,
            build: None,
        }
    }
}

/// What the agent sees: a compressed [`VerifyResult`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompactReport {
    /// Compact text ready for the LLM.
    pub text: String,
    /// Structured equivalent of `text`.
    pub json: serde_json::Value,
    /// [`crate::estimate_tokens`] of `text`.
    pub estimated_tokens: usize,
    /// `true` if content was dropped to fit the budget.
    pub truncated: bool,
}

/// Key of a cached [`VerifyResult`] (a BLAKE3 hash).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct CacheKey(pub [u8; 32]);

impl CacheKey {
    /// Raw bytes, e.g. for use as a database key.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Lowercase hex representation.
    pub fn to_hex(&self) -> String {
        self.0.iter().map(|b| format!("{b:02x}")).collect()
    }
}

impl fmt::Display for CacheKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::de::DeserializeOwned;

    fn roundtrip<T: Serialize + DeserializeOwned + PartialEq + fmt::Debug>(value: &T) {
        let json = serde_json::to_string(value).unwrap();
        let back: T = serde_json::from_str(&json).unwrap();
        assert_eq!(&back, value, "roundtrip of {json}");
    }

    fn krate(name: &str) -> CrateId {
        CrateId {
            name: name.into(),
            manifest_path: PathBuf::from(format!("/ws/crates/{name}/Cargo.toml")),
        }
    }

    fn sample_result() -> VerifyResult {
        VerifyResult {
            outcome: Outcome::Fail {
                failed_step: Step::Check,
            },
            steps: vec![
                StepReport {
                    step: Step::Fmt,
                    duration_ms: 12,
                    passed: true,
                },
                StepReport {
                    step: Step::Check,
                    duration_ms: 1800,
                    passed: false,
                },
            ],
            diagnostics: vec![Diagnostic {
                severity: Severity::Error,
                code: Some("E0308".into()),
                message: "mismatched types".into(),
                file: Some(PathBuf::from("crates/api/src/handler.rs")),
                line: Some(42),
                column: Some(17),
                snippet: Some("let id: u64 = req.id;".into()),
                label: Some("expected `u64`, found `String`".into()),
                suggestion: Some("req.id.parse::<u64>()?".into()),
                crate_id: Some(krate("api")),
                step: Step::Check,
            }],
            test_failures: vec![TestFailure {
                name: "tests::adds".into(),
                crate_id: krate("api"),
                output: "assertion failed".into(),
            }],
            impact: ImpactSet {
                changed: vec![krate("api")],
                dependents: vec![krate("cli")],
                full_rebuild: false,
            },
            cache_hit: false,
            total_ms: 1812,
        }
    }

    #[test]
    fn roundtrip_all_types() {
        roundtrip(&WorkspaceRoot(PathBuf::from("/ws")));
        roundtrip(&ChangeSet {
            files: vec![PathBuf::from("crates/api/src/lib.rs")],
        });
        roundtrip(&sample_result());
        roundtrip(&Outcome::Pass);
        roundtrip(&VerifyRequest::default());
        roundtrip(&VerifyRequest {
            changes: Some(ChangeSet::default()),
            up_to: Step::Clippy,
            ..VerifyRequest::default()
        });
        roundtrip(&CompactReport {
            text: "PASS".into(),
            json: serde_json::json!({ "outcome": "pass" }),
            estimated_tokens: 1,
            truncated: false,
        });
        roundtrip(&CacheKey([7; 32]));
    }

    #[test]
    fn enums_use_lowercase_json() {
        assert_eq!(serde_json::to_string(&Step::Clippy).unwrap(), "\"clippy\"");
        assert_eq!(serde_json::to_string(&Severity::Help).unwrap(), "\"help\"");
        assert_eq!(
            serde_json::to_value(Outcome::Fail {
                failed_step: Step::Test
            })
            .unwrap(),
            serde_json::json!({ "status": "fail", "failed_step": "test" })
        );
        assert_eq!(
            serde_json::to_value(Outcome::Pass).unwrap(),
            serde_json::json!({ "status": "pass" })
        );
    }

    #[test]
    fn verify_request_fills_defaults() {
        let req: VerifyRequest = serde_json::from_str(r#"{"root":"/ws"}"#).unwrap();
        assert_eq!(req.root, PathBuf::from("/ws"));
        assert_eq!(req.up_to, Step::Test);
        assert_eq!(req.token_budget, DEFAULT_TOKEN_BUDGET);
        assert!(req.use_cache);
        assert!(req.changes.is_none());
    }

    #[test]
    fn step_ladder_and_parsing() {
        assert_eq!(
            Step::Clippy.ladder().collect::<Vec<_>>(),
            vec![Step::Fmt, Step::Check, Step::Clippy]
        );
        assert_eq!(Step::Fmt.ladder().count(), 1);
        assert_eq!("CHECK".parse::<Step>().unwrap(), Step::Check);
        assert!("build".parse::<Step>().is_err());
        assert_eq!(Step::Test.to_string(), "test");
    }

    #[test]
    fn impact_affected_is_sorted_union() {
        let impact = ImpactSet {
            changed: vec![krate("mid"), krate("base")],
            dependents: vec![krate("app"), krate("mid")],
            full_rebuild: false,
        };
        let names: Vec<_> = impact.affected().into_iter().map(|c| c.name).collect();
        assert_eq!(names, ["app", "base", "mid"]);
        assert!(!impact.is_empty());
        assert!(ImpactSet::default().is_empty());
    }

    #[test]
    fn build_options_cargo_args() {
        assert!(BuildOptions::default().cargo_args().is_empty());
        let opts = BuildOptions {
            features: vec!["a".into(), "b".into()],
            all_features: false,
            no_default_features: true,
            target: Some("wasm32-unknown-unknown".into()),
        };
        assert_eq!(
            opts.cargo_args(),
            [
                "--no-default-features",
                "--features",
                "a,b",
                "--target",
                "wasm32-unknown-unknown"
            ]
        );
        roundtrip(&opts);
    }

    #[test]
    fn cache_key_hex() {
        let mut bytes = [0u8; 32];
        bytes[0] = 0xab;
        bytes[31] = 0x01;
        let hex = CacheKey(bytes).to_hex();
        assert_eq!(hex.len(), 64);
        assert!(hex.starts_with("ab00"));
        assert!(hex.ends_with("01"));
    }
}
