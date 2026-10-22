//! Runs the verification ladder (`fmt` → `check` → `clippy` → `test`) with
//! cargo and turns the output into [`orq_core::VerifyResult`] data.
//!
//! Only the crates of the [`ImpactSet`] are built (`-p <crate>`), or the whole
//! workspace when `full_rebuild` is set. The runner respects the
//! environment's `CARGO_TARGET_DIR` and never cleans `target/`.

#![warn(missing_docs)]

pub mod explain;
mod index;
mod messages;
mod test_out;
mod util;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use orq_core::{
    BuildOptions, Config, CoreError, Diagnostic, ImpactSet, NextestMode, Outcome, Severity, Step,
    StepReport, StepRunner, TestFailure, VerifyResult,
};
use tokio::process::Command;

use crate::index::CrateIndex;
use crate::util::{strip_ansi, tail_lines};

/// Errors raised while running the ladder.
///
/// Compiler errors and failed tests are *results*, not errors; this type only
/// covers problems running cargo itself.
#[derive(Debug, thiserror::Error)]
pub enum RunnerError {
    /// A process could not be spawned.
    #[error("failed to run `{command}`: {source}")]
    Spawn {
        /// Command line that was attempted.
        command: String,
        /// Underlying error.
        #[source]
        source: std::io::Error,
    },
    /// `nextest = "always"` but `cargo nextest` is not installed.
    #[error("`nextest = \"always\"` is configured but cargo-nextest is not installed")]
    NextestMissing,
    /// The workspace root is not a directory.
    #[error("workspace root `{0}` is not a directory")]
    InvalidRoot(PathBuf),
}

impl From<RunnerError> for CoreError {
    fn from(err: RunnerError) -> Self {
        match err {
            RunnerError::Spawn { command, source } => CoreError::Process {
                command,
                message: source.to_string(),
            },
            RunnerError::NextestMissing => CoreError::Process {
                command: "cargo nextest".to_string(),
                message: "cargo-nextest is not installed".to_string(),
            },
            RunnerError::InvalidRoot(path) => CoreError::io(
                path,
                std::io::Error::new(std::io::ErrorKind::NotFound, "not a directory"),
            ),
        }
    }
}

/// Executes verification steps with cargo.
#[derive(Debug, Clone)]
pub struct Runner {
    config: Config,
}

/// Raw output of one process.
struct Captured {
    success: bool,
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

/// What one step produced.
#[derive(Default)]
struct StepOutput {
    diagnostics: Vec<Diagnostic>,
    test_failures: Vec<TestFailure>,
    /// `false` if the step produced errors or failed tests.
    passed: bool,
}

impl StepRunner for Runner {
    async fn run(
        &self,
        root: &Path,
        impact: &ImpactSet,
        up_to: Step,
        _build: &BuildOptions,
    ) -> Result<VerifyResult, CoreError> {
        // TODO(T4.2): pass `build.cargo_args()` to every cargo invocation.
        self.verify(root, impact, up_to)
            .await
            .map_err(CoreError::from)
    }
}

impl Runner {
    /// Creates a runner using `config` (notably [`Config::nextest`]).
    pub fn new(config: Config) -> Self {
        Self { config }
    }

    /// Runs the ladder up to and including `up_to` on the crates of `impact`.
    ///
    /// Steps run in order and the ladder stops at the first step with errors
    /// (warnings do not stop it). The result has `cache_hit = false` and
    /// `impact = impact.clone()`. If `impact.is_empty()` nothing is run and
    /// the result is a pass with no steps.
    pub async fn verify(
        &self,
        root: &Path,
        impact: &ImpactSet,
        up_to: Step,
    ) -> Result<VerifyResult, RunnerError> {
        let started = Instant::now();
        let mut result = VerifyResult {
            outcome: Outcome::Pass,
            steps: Vec::new(),
            diagnostics: Vec::new(),
            test_failures: Vec::new(),
            impact: impact.clone(),
            cache_hit: false,
            total_ms: 0,
        };
        if impact.is_empty() {
            return Ok(result);
        }
        if !root.is_dir() {
            return Err(RunnerError::InvalidRoot(root.to_path_buf()));
        }

        let index = CrateIndex::new(root, impact.affected());
        for step in up_to.ladder() {
            let step_started = Instant::now();
            let output = match step {
                Step::Fmt => self.run_fmt(root, impact, &index).await?,
                Step::Check | Step::Clippy => self.run_compiler(step, root, impact, &index).await?,
                Step::Test => self.run_tests(root, impact, &index).await?,
            };
            result.steps.push(StepReport {
                step,
                duration_ms: millis(step_started.elapsed()),
                passed: output.passed,
            });
            result.diagnostics.extend(output.diagnostics);
            result.test_failures.extend(output.test_failures);
            if !output.passed {
                result.outcome = Outcome::Fail { failed_step: step };
                break;
            }
        }
        result.total_ms = millis(started.elapsed());
        Ok(result)
    }

    async fn run_fmt(
        &self,
        root: &Path,
        impact: &ImpactSet,
        index: &CrateIndex,
    ) -> Result<StepOutput, RunnerError> {
        let mut args = vec!["fmt".to_string()];
        if impact.full_rebuild {
            args.push("--all".into());
        } else {
            args.extend(package_args(impact));
        }
        args.extend(["--".into(), "--check".into()]);
        let out = run_cargo(root, &args, &[]).await?;
        let mut diagnostics = messages::parse_fmt_diffs(&out.stdout, index);
        if !out.success && diagnostics.is_empty() {
            // rustfmt itself failed (syntax error, missing component...).
            diagnostics.push(process_error(Step::Fmt, &out));
        }
        let passed = !diagnostics.iter().any(|d| d.severity == Severity::Error);
        Ok(StepOutput {
            diagnostics,
            test_failures: Vec::new(),
            passed,
        })
    }

    async fn run_compiler(
        &self,
        step: Step,
        root: &Path,
        impact: &ImpactSet,
        index: &CrateIndex,
    ) -> Result<StepOutput, RunnerError> {
        let mut args = vec![step.as_str().to_string()];
        args.extend(compile_package_args(impact));
        args.extend(["--all-targets".into(), "--message-format=json".into()]);
        let out = run_cargo(root, &args, &[]).await?;
        let mut diagnostics = messages::parse_compiler_messages(&out.stdout, step, index);
        let has_error = diagnostics.iter().any(|d| d.severity == Severity::Error);
        if !out.success && !has_error {
            // Failure outside the compiler (bad manifest, unknown package...).
            diagnostics.push(process_error(step, &out));
        }
        let passed = out.success && !has_error;
        Ok(StepOutput {
            diagnostics,
            test_failures: Vec::new(),
            passed,
        })
    }

    async fn run_tests(
        &self,
        root: &Path,
        impact: &ImpactSet,
        index: &CrateIndex,
    ) -> Result<StepOutput, RunnerError> {
        let use_nextest = match self.config.nextest {
            NextestMode::Never => false,
            NextestMode::Always => {
                if !nextest_installed(root).await {
                    return Err(RunnerError::NextestMissing);
                }
                true
            }
            NextestMode::Auto => nextest_installed(root).await,
        };

        let mut args: Vec<String> = if use_nextest {
            vec!["nextest".into(), "run".into()]
        } else {
            vec!["test".into()]
        };
        args.extend(compile_package_args(impact));
        args.push("--no-fail-fast".into());
        let out = run_cargo(root, &args, &[("RUST_BACKTRACE", "0")]).await?;

        let test_failures = if use_nextest {
            test_out::parse_nextest(&out.stderr, index)
        } else {
            test_out::parse_cargo_test(&out.stdout, &out.stderr, index)
        };
        let mut diagnostics = Vec::new();
        // nextest exits with 4 when there is simply nothing to run.
        let nothing_to_run = use_nextest && out.code == Some(4);
        let passed = if out.success || nothing_to_run {
            true
        } else {
            if test_failures.is_empty() {
                // Build failure or crash rather than failing tests.
                diagnostics.push(process_error(Step::Test, &out));
            }
            false
        };
        Ok(StepOutput {
            diagnostics,
            test_failures,
            passed,
        })
    }
}

/// `-p <name>` for every affected crate.
fn package_args(impact: &ImpactSet) -> Vec<String> {
    impact
        .affected()
        .into_iter()
        .flat_map(|c| ["-p".to_string(), c.name])
        .collect()
}

/// `--workspace` for a full rebuild, otherwise `-p <name>` per crate.
fn compile_package_args(impact: &ImpactSet) -> Vec<String> {
    if impact.full_rebuild {
        vec!["--workspace".to_string()]
    } else {
        package_args(impact)
    }
}

/// Error diagnostic describing a failed process that produced no diagnostics.
fn process_error(step: Step, out: &Captured) -> Diagnostic {
    let detail = tail_lines(&strip_ansi(&out.stderr), 20);
    let message = if detail.is_empty() {
        format!("`cargo {step}` failed (exit code {:?})", out.code)
    } else {
        format!("`cargo {step}` failed: {detail}")
    };
    Diagnostic {
        severity: Severity::Error,
        code: None,
        message,
        file: None,
        line: None,
        column: None,
        snippet: None,
        label: None,
        suggestion: None,
        crate_id: None,
        step,
    }
}

fn millis(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

fn cargo_bin() -> OsString {
    std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"))
}

/// Runs `cargo <args>` in `root` and captures its output.
///
/// The environment (including `CARGO_TARGET_DIR`) is inherited untouched,
/// except for `extra_env` and color settings.
async fn run_cargo(
    root: &Path,
    args: &[String],
    extra_env: &[(&str, &str)],
) -> Result<Captured, RunnerError> {
    let command_line = format!("cargo {}", args.join(" "));
    tracing::debug!(command = %command_line, "running");
    let mut cmd = Command::new(cargo_bin());
    cmd.args(args)
        .current_dir(root)
        .env("CARGO_TERM_COLOR", "never")
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    let output = cmd.output().await.map_err(|source| RunnerError::Spawn {
        command: command_line,
        source,
    })?;
    Ok(Captured {
        success: output.status.success(),
        code: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

/// `true` if `cargo nextest` is available.
async fn nextest_installed(root: &Path) -> bool {
    matches!(
        run_cargo(root, &["nextest".into(), "--version".into()], &[]).await,
        Ok(out) if out.success
    )
}
