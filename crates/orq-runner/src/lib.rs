//! Runs the verification ladder (`fmt` → `check` → `clippy` → `test`) with
//! cargo and turns the output into [`orq_core::VerifyResult`] data.
//!
//! Only the crates of the [`ImpactSet`] are built (`-p <crate>`), or the whole
//! workspace when `full_rebuild` is set. The runner respects the
//! environment's `CARGO_TARGET_DIR` and never cleans `target/`.

#![warn(missing_docs)]

pub mod explain;
mod failures;
pub mod fix;
mod index;
mod messages;
mod targets;
mod test_out;
mod util;

use std::collections::HashSet;
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
    /// The requested operation is not available.
    #[error("{0}")]
    Unsupported(String),
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
            RunnerError::Unsupported(message) => CoreError::Process {
                command: "orq".to_string(),
                message,
            },
        }
    }
}

/// Executes verification steps with cargo.
#[derive(Debug, Clone)]
pub struct Runner {
    config: Config,
    cargo: Option<OsString>,
    failing_first: bool,
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
        build: &BuildOptions,
    ) -> Result<VerifyResult, CoreError> {
        self.verify_with(root, impact, up_to, build)
            .await
            .map_err(CoreError::from)
    }
}

impl Runner {
    async fn run_cargo(
        &self,
        root: &Path,
        args: &[String],
        extra_env: &[(&str, &str)],
    ) -> Result<Captured, RunnerError> {
        run_cargo_with(self.cargo_bin(), root, args, extra_env).await
    }

    /// `true` if `cargo nextest` is available.
    async fn nextest_installed(&self, root: &Path) -> bool {
        matches!(
            self.run_cargo(root, &["nextest".into(), "--version".into()], &[]).await,
            Ok(out) if out.success
        )
    }

    /// Creates a runner using `config` (notably [`Config::nextest`]).
    pub fn new(config: Config) -> Self {
        Self {
            config,
            cargo: None,
            failing_first: true,
        }
    }

    /// Enables or disables "failing tests first" (enabled by default).
    ///
    /// When enabled, the Test step first runs only the tests that failed in
    /// the previous run of the same workspace (remembered in
    /// `<target>/orq/last_failures.json`). If any of them still fails the
    /// step stops there: the report holds just those failures plus a `Note`
    /// diagnostic saying the rest of the suite was not run. Otherwise the
    /// full suite runs as usual.
    #[must_use]
    pub fn with_failing_first(mut self, enabled: bool) -> Self {
        self.failing_first = enabled;
        self
    }

    /// Uses `cargo` as the cargo executable instead of `$CARGO` / `cargo`.
    ///
    /// Mostly useful to wrap cargo (for instance to count invocations).
    #[must_use]
    pub fn with_cargo_bin(mut self, cargo: impl Into<OsString>) -> Self {
        self.cargo = Some(cargo.into());
        self
    }

    /// Compiles (but does not run) the tests of the crates in `impact`, so a
    /// later Test step only has to execute them. Used by the daemon after a
    /// change.
    ///
    /// Runs `cargo nextest run --no-run` or `cargo test --no-run` (matching
    /// the nextest configuration) with exactly the package and build
    /// arguments of the Test step, so the artifacts are reused. Compiler
    /// errors and diagnostics are discarded: the next real verify reports
    /// them. Only a missing `cargo-nextest` with `nextest = "always"` or a
    /// failure to spawn cargo is an error.
    pub async fn prebuild_tests(
        &self,
        root: &Path,
        impact: &ImpactSet,
        build: &BuildOptions,
    ) -> Result<(), RunnerError> {
        if impact.is_empty() {
            return Ok(());
        }
        if !root.is_dir() {
            return Err(RunnerError::InvalidRoot(root.to_path_buf()));
        }
        let use_nextest = self.use_nextest(root).await?;
        let mut args: Vec<String> = if use_nextest {
            vec!["nextest".into(), "run".into()]
        } else {
            vec!["test".into()]
        };
        args.extend(compile_package_args(impact));
        args.push("--no-run".into());
        args.extend(build.cargo_args());
        let out = self.run_cargo(root, &args, &[]).await?;
        tracing::debug!(success = out.success, "prebuilt tests");
        Ok(())
    }

    /// Whether the Test step uses nextest, per the configuration.
    async fn use_nextest(&self, root: &Path) -> Result<bool, RunnerError> {
        Ok(match self.config.nextest {
            NextestMode::Never => false,
            NextestMode::Always => {
                if !self.nextest_installed(root).await {
                    return Err(RunnerError::NextestMissing);
                }
                true
            }
            NextestMode::Auto => self.nextest_installed(root).await,
        })
    }

    fn cargo_bin(&self) -> OsString {
        self.cargo.clone().unwrap_or_else(cargo_bin)
    }

    /// Runs the ladder up to and including `up_to` on the crates of `impact`
    /// with the default [`BuildOptions`].
    ///
    /// See [`Runner::verify_with`].
    pub async fn verify(
        &self,
        root: &Path,
        impact: &ImpactSet,
        up_to: Step,
    ) -> Result<VerifyResult, RunnerError> {
        self.verify_with(root, impact, up_to, &BuildOptions::default())
            .await
    }

    /// Runs the ladder up to and including `up_to` on the crates of `impact`.
    ///
    /// `build.cargo_args()` is appended to every cargo invocation except
    /// `cargo fmt`.
    ///
    /// Steps run in order and the ladder stops at the first step with errors
    /// (warnings do not stop it). The result has `cache_hit = false` and
    /// `impact = impact.clone()`. If `impact.is_empty()` nothing is run and
    /// the result is a pass with no steps.
    ///
    /// Performance: `cargo fmt --check` runs concurrently with the first
    /// compile, and when the ladder reaches `Clippy` a single
    /// `cargo clippy` run provides both the `Check` and the `Clippy` steps
    /// (clippy already runs a full `cargo check` underneath). Diagnostics
    /// whose code starts with `clippy::` belong to `Clippy`, everything else
    /// to `Check`. The shared run's wall time is reported as the duration of
    /// `Check`; `Clippy` reports 0 ms because it costs nothing extra. If
    /// `Check` fails, `Clippy` is not reported (it never really ran).
    pub async fn verify_with(
        &self,
        root: &Path,
        impact: &ImpactSet,
        up_to: Step,
        build: &BuildOptions,
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
        let build_args = build.cargo_args();

        // Fmt, concurrently with the first compile when there is one.
        let fmt_started = Instant::now();
        let (fmt, compile) = if up_to == Step::Fmt {
            (self.run_fmt(root, impact, &index).await, None)
        } else {
            let (fmt, compile) = tokio::join!(
                self.run_fmt(root, impact, &index),
                self.run_compile_ladder(up_to, root, impact, &index, build)
            );
            (fmt, Some(compile))
        };
        let fmt_ms = millis(fmt_started.elapsed());
        let fmt = fmt?;
        result.steps.push(StepReport {
            step: Step::Fmt,
            duration_ms: fmt_ms,
            passed: fmt.passed,
        });
        let mut failed = (!fmt.passed).then_some(Step::Fmt);
        result.diagnostics.extend(fmt.diagnostics);

        if let Some(compile) = compile {
            let (reports, diagnostics, compile_failed) = compile?;
            result.steps.extend(reports);
            result.diagnostics.extend(diagnostics);
            // A rustfmt crash wins; the compile results are kept anyway.
            failed = failed.or(compile_failed);
        }

        if failed.is_none() && up_to == Step::Test {
            let step_started = Instant::now();
            let output = self.run_tests(root, impact, &index, &build_args).await?;
            result.steps.push(StepReport {
                step: Step::Test,
                duration_ms: millis(step_started.elapsed()),
                passed: output.passed,
            });
            result.diagnostics.extend(output.diagnostics);
            result.test_failures.extend(output.test_failures);
            if !output.passed {
                failed = Some(Step::Test);
            }
        }
        if let Some(failed_step) = failed {
            result.outcome = Outcome::Fail { failed_step };
        }
        result.total_ms = millis(started.elapsed());
        Ok(result)
    }

    /// Runs the compile part of the ladder (`Check`, and `Clippy` when
    /// `up_to >= Clippy`) with a single cargo invocation.
    ///
    /// Returns the step reports, the diagnostics and the failed step, if any.
    async fn run_compile_ladder(
        &self,
        up_to: Step,
        root: &Path,
        impact: &ImpactSet,
        index: &CrateIndex,
        build: &BuildOptions,
    ) -> Result<(Vec<StepReport>, Vec<Diagnostic>, Option<Step>), RunnerError> {
        let started = Instant::now();
        let build_args = build.cargo_args();
        let use_clippy = up_to >= Step::Clippy;
        let mut args = vec![if use_clippy { "clippy" } else { "check" }.to_string()];
        args.extend(compile_package_args(impact));
        args.extend(
            targets::compile_target_args(self.cargo_bin(), root, impact, build.targets).await,
        );
        args.push("--message-format=json".into());
        args.extend(build_args);
        // Passed to clippy-driver only: RUSTFLAGS would rebuild every dependency.
        if use_clippy && self.config.deny_warnings {
            args.extend(["--".into(), "-D".into(), "warnings".into()]);
        }
        let out = self.run_cargo(root, &args, &[]).await?;
        let duration_ms = millis(started.elapsed());

        let mut diagnostics = if use_clippy {
            messages::parse_clippy_messages(&out.stdout, index)
        } else {
            messages::parse_compiler_messages(&out.stdout, Step::Check, index)
        };
        let has_error_in = |diagnostics: &[Diagnostic], step: Step| {
            diagnostics
                .iter()
                .any(|d| d.step == step && d.severity == Severity::Error)
        };
        let check_error = has_error_in(&diagnostics, Step::Check);
        let clippy_error = has_error_in(&diagnostics, Step::Clippy);
        if !out.success && !check_error && !clippy_error {
            // Failure outside the compiler (bad manifest, unknown package,
            // target not installed...): surface cargo's stderr.
            diagnostics.push(process_error(Step::Check, &args[0], &out));
        }
        let check_failed = check_error || (!out.success && !clippy_error);
        let mut reports = vec![StepReport {
            step: Step::Check,
            duration_ms,
            passed: !check_failed,
        }];
        let mut failed = check_failed.then_some(Step::Check);
        if use_clippy && !check_failed {
            reports.push(StepReport {
                step: Step::Clippy,
                duration_ms: 0,
                passed: !clippy_error,
            });
            if clippy_error {
                failed = Some(Step::Clippy);
            }
        }
        Ok((reports, diagnostics, failed))
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
        let out = self.run_cargo(root, &args, &[]).await?;
        let mut diagnostics = messages::parse_fmt_diffs(&out.stdout, index);
        if !out.success && diagnostics.is_empty() {
            // rustfmt itself failed (syntax error, missing component...).
            diagnostics.push(process_error(Step::Fmt, "fmt", &out));
        }
        let passed = !diagnostics.iter().any(|d| d.severity == Severity::Error);
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
        build_args: &[String],
    ) -> Result<StepOutput, RunnerError> {
        let use_nextest = self.use_nextest(root).await?;
        // Crates whose remembered failures this run replaces.
        let scope: Option<HashSet<String>> =
            (!impact.full_rebuild).then(|| impact.affected().into_iter().map(|c| c.name).collect());
        let in_scope = |r: &failures::Recorded| scope.as_ref().is_none_or(|s| s.contains(&r.krate));

        let previous: Vec<failures::Recorded> = if self.failing_first {
            failures::load(root).into_iter().filter(in_scope).collect()
        } else {
            Vec::new()
        };
        let (fast, unrunnable): (Vec<_>, Vec<_>) = previous
            .into_iter()
            .partition(|r| failures::is_plain_name(&r.name));

        if !fast.is_empty() {
            let names: Vec<&str> = fast.iter().map(|r| r.name.as_str()).collect();
            let (mut output, finished) = self
                .exec_tests(root, impact, index, build_args, use_nextest, Some(&names))
                .await?;
            if !output.passed {
                if finished && !output.test_failures.is_empty() {
                    let mut kept = failures::record(&output.test_failures);
                    kept.extend(unrunnable);
                    failures::save(root, scope.as_ref(), kept);
                    output
                        .diagnostics
                        .push(not_run_note(output.test_failures.len()));
                }
                return Ok(output);
            }
        }

        let (output, finished) = self
            .exec_tests(root, impact, index, build_args, use_nextest, None)
            .await?;
        if finished {
            failures::save(
                root,
                scope.as_ref(),
                failures::record(&output.test_failures),
            );
        }
        Ok(output)
    }

    /// Runs the tests, all of them or only those named in `only` (exact
    /// names). The flag says whether the tests really ran (as opposed to a
    /// build failure or crash), i.e. whether the failure list is meaningful.
    async fn exec_tests(
        &self,
        root: &Path,
        impact: &ImpactSet,
        index: &CrateIndex,
        build_args: &[String],
        use_nextest: bool,
        only: Option<&[&str]>,
    ) -> Result<(StepOutput, bool), RunnerError> {
        let mut args: Vec<String> = if use_nextest {
            vec!["nextest".into(), "run".into()]
        } else {
            vec!["test".into()]
        };
        args.extend(compile_package_args(impact));
        args.push("--no-fail-fast".into());
        args.extend(build_args.iter().cloned());
        if let Some(names) = only {
            if use_nextest {
                let filter = names
                    .iter()
                    .map(|n| format!("test(={n})"))
                    .collect::<Vec<_>>()
                    .join(" | ");
                args.extend(["-E".into(), filter]);
            } else {
                args.push("--".into());
                args.push("--exact".into());
                args.extend(names.iter().map(|n| (*n).to_string()));
            }
        }
        let out = self
            .run_cargo(root, &args, &[("RUST_BACKTRACE", "0")])
            .await?;

        let test_failures = if use_nextest {
            test_out::parse_nextest(&out.stderr, index)
        } else {
            test_out::parse_cargo_test(&out.stdout, &out.stderr, index)
        };
        let mut diagnostics = Vec::new();
        // nextest exits with 4 when there is simply nothing to run.
        let nothing_to_run = use_nextest && out.code == Some(4);
        let finished = out.success || nothing_to_run || !test_failures.is_empty();
        let passed = if out.success || nothing_to_run {
            true
        } else {
            if test_failures.is_empty() {
                // Build failure or crash rather than failing tests.
                diagnostics.push(process_error(Step::Test, &args[0], &out));
            }
            false
        };
        Ok((
            StepOutput {
                diagnostics,
                test_failures,
                passed,
            },
            finished,
        ))
    }
}

/// Note added to the report when only the previously failing tests ran.
fn not_run_note(failed: usize) -> Diagnostic {
    Diagnostic {
        severity: Severity::Note,
        code: None,
        message: format!(
            "Fail-fast: only the {failed} test(s) that failed in the previous run were executed \
             and still fail; the rest of the test suite was not run. Fix them and verify again \
             to run the full suite."
        ),
        file: None,
        line: None,
        column: None,
        snippet: None,
        label: None,
        suggestion: None,
        crate_id: None,
        step: Step::Test,
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

/// `--workspace` for a full rebuild, otherwise one `-p <spec>` per crate.
///
/// The spec is the crate's directory as a `path+file://` package ID spec, not its
/// name: when a registry dependency has the same name as a workspace member
/// (tokio, serde...), `-p <name>` is ambiguous and cargo refuses to run.
/// `cargo fmt` does not understand specs, so it keeps [`package_args`].
pub(crate) fn compile_package_args(impact: &ImpactSet) -> Vec<String> {
    if impact.full_rebuild {
        return vec!["--workspace".to_string()];
    }
    impact
        .affected()
        .into_iter()
        .flat_map(|c| ["-p".to_string(), package_spec(&c)])
        .collect()
}

/// Unambiguous package ID spec for a workspace member (falls back to the
/// name when the manifest path is not usable as a `file://` URL).
fn package_spec(krate: &orq_core::CrateId) -> String {
    // Cargo matches the canonical path (on macOS `/var` is a symlink to
    // `/private/var`), so resolve symlinks first.
    let dir = krate
        .manifest_path
        .parent()
        .filter(|d| d.is_absolute())
        .map(|d| d.canonicalize().unwrap_or_else(|_| d.to_path_buf()));
    match dir.as_deref().and_then(|d| d.to_str()) {
        // The `#name` fragment matters: without it cargo takes the last
        // path segment as the package name.
        Some(dir) if cfg!(unix) => format!("path+file://{dir}#{}", krate.name),
        _ => krate.name.clone(),
    }
}

/// Error diagnostic describing a failed process that produced no diagnostics.
fn process_error(step: Step, cargo_cmd: &str, out: &Captured) -> Diagnostic {
    let detail = tail_lines(&strip_ansi(&out.stderr), 20);
    let message = if detail.is_empty() {
        format!("`cargo {cargo_cmd}` failed (exit code {:?})", out.code)
    } else {
        format!("`cargo {cargo_cmd}` failed: {detail}")
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
async fn run_cargo_with(
    cargo: OsString,
    root: &Path,
    args: &[String],
    extra_env: &[(&str, &str)],
) -> Result<Captured, RunnerError> {
    let command_line = format!("cargo {}", args.join(" "));
    tracing::debug!(command = %command_line, "running");
    let mut cmd = Command::new(cargo);
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
