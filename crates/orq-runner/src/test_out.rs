//! Extraction of failed tests from `cargo test` and `cargo nextest` output.

use std::collections::HashMap;
use std::path::Path;

use orq_core::{CrateId, TestFailure};

use crate::index::CrateIndex;
use crate::util::{strip_ansi, truncate};

/// Maximum captured output kept per failed test.
const MAX_OUTPUT_BYTES: usize = 8 * 1024;

/// Crate of one test binary, from cargo's `Running ...` / `Doc-tests ...`
/// line on stderr.
fn crate_of_marker(marker: &str, index: &CrateIndex) -> Option<CrateId> {
    let marker = marker.trim();
    if let Some(name) = marker.strip_prefix("Doc-tests ") {
        return index.for_name(name.trim());
    }
    let rest = marker.strip_prefix("Running ")?;
    let (source, binary) = match rest.split_once(" (") {
        Some((s, b)) => (s, b.trim_end_matches(')')),
        None => (rest, ""),
    };
    let source = source.strip_prefix("unittests ").unwrap_or(source).trim();
    if let Some(c) = index.for_path(Path::new(source)) {
        return Some(c);
    }
    // Fall back to the binary's name without its hash suffix.
    let stem = Path::new(binary)
        .file_name()?
        .to_string_lossy()
        .into_owned();
    let name = stem.rsplit_once('-').map_or(stem.as_str(), |(n, _)| n);
    index.for_name(name)
}

/// Parses `cargo test --no-fail-fast` output.
///
/// libtest prints one `running N tests` block per binary on stdout and cargo
/// prints one `Running ...`/`Doc-tests ...` line per binary on stderr; the
/// k-th of each belong together, which is how failures are attributed to a
/// crate.
pub(crate) fn parse_cargo_test(stdout: &str, stderr: &str, index: &CrateIndex) -> Vec<TestFailure> {
    let stdout = strip_ansi(stdout);
    let stderr = strip_ansi(stderr);
    let markers: Vec<&str> = stderr
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with("Running ") || l.starts_with("Doc-tests "))
        .collect();

    let mut blocks: Vec<Vec<&str>> = Vec::new();
    for line in stdout.lines() {
        if line.starts_with("running ") && (line.ends_with(" tests") || line.ends_with(" test")) {
            blocks.push(Vec::new());
        } else if let Some(block) = blocks.last_mut() {
            block.push(line);
        }
    }

    let mut failures = Vec::new();
    for (i, block) in blocks.iter().enumerate() {
        let krate = markers
            .get(i)
            .filter(|_| markers.len() == blocks.len())
            .and_then(|m| crate_of_marker(m, index))
            .unwrap_or_else(|| index.fallback());
        let names: Vec<&str> = block
            .iter()
            .filter_map(|l| l.strip_prefix("test ")?.strip_suffix(" ... FAILED"))
            .collect();
        let sections = stdout_sections(block);
        for name in names {
            let output = sections.get(name).cloned().unwrap_or_default();
            failures.push(TestFailure {
                name: name.to_string(),
                crate_id: krate.clone(),
                output: truncate(output, MAX_OUTPUT_BYTES),
            });
        }
    }
    failures
}

/// `---- name stdout ----` sections of one libtest block.
fn stdout_sections(block: &[&str]) -> HashMap<String, String> {
    let mut sections: HashMap<String, Vec<&str>> = HashMap::new();
    let mut current: Option<String> = None;
    for line in block {
        if let Some(name) = line
            .strip_prefix("---- ")
            .and_then(|l| l.strip_suffix(" stdout ----"))
        {
            current = Some(name.to_string());
            sections.entry(name.to_string()).or_default();
        } else if *line == "failures:" {
            current = None;
        } else if let Some(name) = &current {
            sections.entry(name.clone()).or_default().push(line);
        }
    }
    sections
        .into_iter()
        .map(|(name, lines)| (name, lines.join("\n").trim().to_string()))
        .collect()
}

/// Parses one nextest status line such as
/// `FAIL [   0.005s] (1/2) my-crate tests::broken` into
/// `(status, binary id, test name)`.
fn parse_status_line(trimmed: &str) -> Option<(&str, &str, &str)> {
    let (status, rest) = trimmed.split_once(char::is_whitespace)?;
    let rest = rest.trim_start().strip_prefix('[')?;
    let (_, mut rest) = rest.split_once(']')?;
    rest = rest.trim_start();
    // Optional `(n/m)` progress counter.
    if rest.starts_with('(') {
        rest = rest.split_once(')')?.1.trim_start();
    }
    let (binary, name) = rest.split_once(char::is_whitespace)?;
    let name = name.trim();
    (!name.is_empty()).then_some((status, binary, name))
}

/// `true` for lines that end a captured output section.
fn is_section_end(trimmed: &str) -> bool {
    trimmed.starts_with("────")
        || [
            "PASS [",
            "FAIL [",
            "SKIP [",
            "SLOW [",
            "LEAK [",
            "Starting ",
            "Summary [",
            "Canceling",
            "TRY ",
        ]
        .iter()
        .any(|p| trimmed.starts_with(p))
}

/// Parses the stderr of `cargo nextest run --no-fail-fast`.
///
/// Failed tests appear as `FAIL [ time ] (n/m) <binary-id> <name>` followed
/// by indented `stdout ───` / `stderr ───` sections, and are listed again in
/// the final summary; only the first occurrence is used.
pub(crate) fn parse_nextest(stderr: &str, index: &CrateIndex) -> Vec<TestFailure> {
    const FAIL_STATUSES: [&str; 5] = ["FAIL", "TIMEOUT", "SIGSEGV", "SIGABRT", "ABORT"];
    let stderr = strip_ansi(stderr);

    // (binary, name, stdout, stderr), in order of first appearance.
    let mut tests: Vec<(String, String, Vec<String>, Vec<String>)> = Vec::new();
    let mut current: Option<usize> = None;
    let mut ignoring = false;
    // Some(true) = stdout section, Some(false) = stderr section.
    let mut section: Option<bool> = None;

    for line in stderr.lines() {
        let trimmed = line.trim();
        if is_section_end(trimmed) {
            section = None;
            if let Some((status, binary, name)) = parse_status_line(trimmed) {
                if FAIL_STATUSES.contains(&status) {
                    match tests.iter().position(|t| t.0 == binary && t.1 == name) {
                        Some(i) => {
                            current = Some(i);
                            ignoring = true;
                        }
                        None => {
                            tests.push((binary.into(), name.into(), Vec::new(), Vec::new()));
                            current = Some(tests.len() - 1);
                            ignoring = false;
                        }
                    }
                    continue;
                }
            }
            if trimmed.starts_with("PASS [") || trimmed.starts_with("Summary [") {
                current = None;
            }
            continue;
        }
        if trimmed.starts_with("stdout ───") {
            section = Some(true);
            continue;
        }
        if trimmed.starts_with("stderr ───") {
            section = Some(false);
            continue;
        }
        if let (Some(i), Some(is_stdout), false) = (current, section, ignoring) {
            let text = line.strip_prefix("    ").unwrap_or(line).to_string();
            if is_stdout {
                tests[i].2.push(text);
            } else {
                tests[i].3.push(text);
            }
        }
    }

    tests
        .into_iter()
        .map(|(binary, name, stdout, stderr)| {
            let crate_name = binary.split("::").next().unwrap_or(&binary);
            // The panic message goes to stderr; stdout only has libtest noise.
            let stderr = stderr.join("\n").trim().to_string();
            let output = if stderr.is_empty() {
                stdout.join("\n").trim().to_string()
            } else {
                stderr
            };
            TestFailure {
                crate_id: index
                    .for_name(crate_name)
                    .unwrap_or_else(|| index.fallback()),
                name,
                output: truncate(output, MAX_OUTPUT_BYTES),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn index() -> CrateIndex {
        let krate = |n: &str| CrateId {
            name: n.into(),
            manifest_path: PathBuf::from(format!("/ws/crates/{n}/Cargo.toml")),
        };
        CrateIndex::new(Path::new("/ws"), vec![krate("base"), krate("my-app")])
    }

    #[test]
    fn cargo_test_failures_are_attributed_per_binary() {
        let stderr = "   Compiling x\n     Running unittests src/lib.rs (target/debug/deps/base-1a2b)\n     Running unittests src/lib.rs (target/debug/deps/my_app-9f)\n   Doc-tests base\n";
        let stderr = stderr
            .replace(
                "src/lib.rs (target/debug/deps/base",
                "crates/base/src/lib.rs (target/debug/deps/base",
            )
            .replace(
                "src/lib.rs (target/debug/deps/my_app",
                "crates/my-app/src/lib.rs (target/debug/deps/my_app",
            );
        let stdout = "\nrunning 2 tests\ntest ok_one ... ok\ntest bad::one ... FAILED\n\nfailures:\n\n---- bad::one stdout ----\nthread 'bad::one' panicked at src/lib.rs:5:9:\nboom\n\n\nfailures:\n    bad::one\n\ntest result: FAILED. 1 passed; 1 failed\n\nrunning 1 test\ntest app_bad ... FAILED\n\nfailures:\n\n---- app_bad stdout ----\nnope\n\nfailures:\n    app_bad\n\ntest result: FAILED.\n\nrunning 0 tests\n\ntest result: ok.\n";
        let failures = parse_cargo_test(stdout, &stderr, &index());
        assert_eq!(failures.len(), 2);
        assert_eq!(failures[0].name, "bad::one");
        assert_eq!(failures[0].crate_id.name, "base");
        assert!(failures[0].output.contains("boom"));
        assert_eq!(failures[1].name, "app_bad");
        assert_eq!(failures[1].crate_id.name, "my-app");
        assert_eq!(failures[1].output, "nope");
    }

    #[test]
    fn nextest_failures_with_output_dedup() {
        let stderr = "\
    Starting 2 tests across 1 binary
        FAIL [   0.005s] (1/2) base tests::broken
  stdout ───

    running 1 test
    test tests::broken ... FAILED

  stderr ───

    thread 'tests::broken' (1) panicked at src/lib.rs:9:9:
    assertion failed

        PASS [   0.006s] (2/2) base tests::fine
────────────
     Summary [   0.006s] 2 tests run: 1 passed, 1 failed, 0 skipped
        FAIL [   0.005s] (1/2) base tests::broken
        FAIL [   0.005s] (2/2) base::it other
error: test run failed
";
        let failures = parse_nextest(stderr, &index());
        assert_eq!(failures.len(), 2);
        assert_eq!(failures[0].name, "tests::broken");
        assert_eq!(failures[0].crate_id.name, "base");
        assert_eq!(
            failures[0].output,
            "thread 'tests::broken' (1) panicked at src/lib.rs:9:9:\nassertion failed"
        );
        assert_eq!(failures[1].name, "other");
        assert_eq!(failures[1].crate_id.name, "base");
        assert_eq!(failures[1].output, "");
    }
}
