//! Compresses a [`VerifyResult`] into a token-budgeted report for an LLM.
//!
//! The pipeline is: deduplicate diagnostics (same code + file + line), order
//! errors before warnings (compile order inside each group), mark errors that
//! follow a resolution error in the same file as "possibly derived", then
//! render the most detailed text that fits the budget. When it does not fit,
//! detail is shed in this order: snippets, warnings, trailing errors
//! (`+K more errors`), and finally everything but the header.
//!
//! # Budget guarantee
//!
//! [`CompactReport::estimated_tokens`] is always `<= budget`. For budgets of at
//! least [`LlmCompressor::MIN_USEFUL_BUDGET`] tokens the header line survives
//! intact; below that the header is hard-cut to `budget * 4` characters and
//! `truncated` is set.
//!
//! Snippets are assumed to start at [`Diagnostic::line`]: the first snippet
//! line is rendered with that line number and the following ones increment it.

mod lang;

use std::collections::HashMap;
use std::path::Path;

use orq_core::{
    estimate_tokens, CompactReport, Compressor, Diagnostic, Lang, Outcome, Severity, TestFailure,
    VerifyResult,
};
use serde_json::{json, Value};

use lang::Words;

/// Codes of name-resolution errors that tend to cause cascades of later errors.
const RESOLUTION_CODES: [&str; 4] = ["E0432", "E0433", "E0412", "E0425"];

/// [`Compressor`] producing compact text and JSON for LLM consumption.
#[derive(Debug, Clone, Default)]
pub struct LlmCompressor {
    lang: Lang,
}

impl LlmCompressor {
    /// Smallest budget (in estimated tokens) for which the header line is
    /// never cut. Smaller budgets get a hard-truncated header.
    pub const MIN_USEFUL_BUDGET: usize = 16;

    /// Creates a compressor writing its text in `lang`.
    pub fn new(lang: Lang) -> Self {
        Self { lang }
    }
}

impl Compressor for LlmCompressor {
    fn compress(&self, result: &VerifyResult, budget: usize) -> CompactReport {
        let prepared = Prepared::new(result);
        let words = Words::of(self.lang);
        let candidates = candidates(result.outcome, &prepared);

        let mut last = (String::new(), Value::Null);
        for (i, params) in candidates.iter().enumerate() {
            let (text, mut json) = render(result, &prepared, params, &words);
            let tokens = estimate_tokens(&text);
            if tokens <= budget {
                json["truncated"] = Value::Bool(i > 0);
                return CompactReport {
                    text,
                    json,
                    estimated_tokens: tokens,
                    truncated: i > 0,
                };
            }
            last = (text, json);
        }

        // Even the shortest header does not fit: hard cut.
        let (text, mut json) = last;
        let text: String = text.chars().take(budget.saturating_mul(4)).collect();
        json["truncated"] = Value::Bool(true);
        CompactReport {
            estimated_tokens: estimate_tokens(&text),
            text,
            json,
            truncated: true,
        }
    }
}

/// A diagnostic plus its cascade verdict.
struct Entry<'a> {
    diag: &'a Diagnostic,
    /// 1-based display index of the resolution error this one probably derives from.
    derived_from: Option<usize>,
}

/// Deduplication key: severity, code, file, line and (only without a code) message.
type DedupKey<'a> = (
    Severity,
    Option<&'a str>,
    Option<&'a Path>,
    Option<u32>,
    &'a str,
);

/// Deduplicated and ordered view of a result.
struct Prepared<'a> {
    errors: Vec<Entry<'a>>,
    warnings: Vec<Entry<'a>>,
    tests: Vec<&'a TestFailure>,
}

impl<'a> Prepared<'a> {
    fn new(result: &'a VerifyResult) -> Self {
        let mut seen: Vec<DedupKey<'a>> = Vec::new();
        let mut errors: Vec<Entry<'a>> = Vec::new();
        let mut warnings: Vec<Entry<'a>> = Vec::new();

        for d in &result.diagnostics {
            if !matches!(d.severity, Severity::Error | Severity::Warning) {
                continue;
            }
            // Without a code, the message is what tells two diagnostics apart.
            let msg = if d.code.is_none() { &d.message[..] } else { "" };
            let key = (
                d.severity,
                d.code.as_deref(),
                d.file.as_deref(),
                d.line,
                msg,
            );
            if seen.contains(&key) {
                continue;
            }
            seen.push(key);
            let entry = Entry {
                diag: d,
                derived_from: None,
            };
            if d.severity == Severity::Error {
                errors.push(entry);
            } else {
                warnings.push(entry);
            }
        }

        // Cascade heuristic: errors after a resolution error in the same file.
        let mut first_resolution: HashMap<&Path, usize> = HashMap::new();
        for (i, entry) in errors.iter_mut().enumerate() {
            let Some(file) = entry.diag.file.as_deref() else {
                continue;
            };
            if let Some(root) = first_resolution.get(file) {
                entry.derived_from = Some(*root);
            } else if entry
                .diag
                .code
                .as_deref()
                .is_some_and(|c| RESOLUTION_CODES.contains(&c))
            {
                first_resolution.insert(file, i + 1);
            }
        }

        let mut tests: Vec<&TestFailure> = Vec::new();
        for t in &result.test_failures {
            if !tests
                .iter()
                .any(|o| o.name == t.name && o.crate_id == t.crate_id)
            {
                tests.push(t);
            }
        }

        Self {
            errors,
            warnings,
            tests,
        }
    }
}

/// How much detail one rendering attempt keeps.
#[derive(Clone, Copy)]
struct Params {
    snippet_lines: usize,
    width: usize,
    suggestions: bool,
    warnings: bool,
    max_errors: usize,
    max_tests: usize,
    test_lines: usize,
    crates_max: usize,
    /// Show the `+K more ...` lines.
    summary: bool,
    /// Show the counts in parentheses in the header.
    counts: bool,
}

/// Rendering attempts from most to least detailed.
fn candidates(outcome: Outcome, prepared: &Prepared<'_>) -> Vec<Params> {
    let full = Params {
        snippet_lines: 8,
        width: 120,
        suggestions: true,
        warnings: true,
        max_errors: usize::MAX,
        max_tests: usize::MAX,
        test_lines: 20,
        crates_max: 6,
        summary: true,
        counts: true,
    };
    let mut out = vec![full];
    if outcome.is_pass() {
        for crates_max in [2, 0] {
            out.push(Params { crates_max, ..full });
        }
        return out;
    }
    // Trim snippets.
    out.push(Params {
        snippet_lines: 3,
        width: 100,
        test_lines: 8,
        ..full
    });
    out.push(Params {
        snippet_lines: 0,
        width: 100,
        test_lines: 4,
        ..full
    });
    // Drop warnings.
    let no_warn = Params {
        snippet_lines: 0,
        width: 100,
        test_lines: 4,
        warnings: false,
        ..full
    };
    out.push(no_warn);
    // Show only the first N errors.
    for n in (1..=prepared.errors.len().max(1)).rev() {
        out.push(Params {
            max_errors: n,
            max_tests: 5,
            test_lines: 2,
            width: 80,
            ..no_warn
        });
    }
    let tiny = Params {
        max_errors: 1,
        max_tests: 1,
        test_lines: 1,
        width: 50,
        suggestions: false,
        crates_max: 2,
        ..no_warn
    };
    out.push(tiny);
    let header_only = Params {
        max_errors: 0,
        max_tests: 0,
        summary: false,
        ..tiny
    };
    out.push(header_only);
    out.push(Params {
        crates_max: 0,
        counts: false,
        ..header_only
    });
    out
}

/// Renders text and JSON for one set of parameters.
fn render(
    result: &VerifyResult,
    prepared: &Prepared<'_>,
    p: &Params,
    w: &Words,
) -> (String, Value) {
    let crates = crate_names(result);
    let crates_shown = crate_list(&crates, result.impact.full_rebuild, p.crates_max, w);
    let secs = format!("{:.1}s", result.total_ms as f64 / 1000.0);

    match result.outcome {
        Outcome::Pass => {
            let steps: Vec<&str> = result.steps.iter().map(|s| s.step.as_str()).collect();
            let steps_txt = if steps.is_empty() {
                "-".to_string()
            } else {
                steps.join("+")
            };
            let mut parts = vec![format!("PASS {steps_txt}")];
            if let Some(c) = &crates_shown {
                parts.push(c.clone());
            }
            parts.push(format!("{secs} ({})", w.cache(result.cache_hit)));
            let json = json!({
                "outcome": "pass",
                "steps": steps,
                "crates": crates,
                "full_rebuild": result.impact.full_rebuild,
                "duration_ms": result.total_ms,
                "cache_hit": result.cache_hit,
            });
            (parts.join(" · "), json)
        }
        Outcome::Fail { failed_step } => {
            let shown_errors = prepared.errors.len().min(p.max_errors);
            let hidden_errors = prepared.errors.len() - shown_errors;
            let shown_tests = prepared.tests.len().min(p.max_tests);
            let hidden_tests = prepared.tests.len() - shown_tests;
            let hidden_warnings = if p.warnings {
                0
            } else {
                prepared.warnings.len()
            };

            // Header.
            let mut counts: Vec<String> = Vec::new();
            if !prepared.errors.is_empty() {
                counts.push(w.errors(prepared.errors.len()));
            }
            if !prepared.tests.is_empty() {
                counts.push(w.failed_tests(prepared.tests.len()));
            }
            if !prepared.warnings.is_empty() {
                counts.push(w.warnings(prepared.warnings.len(), hidden_warnings > 0));
            }
            let mut head = format!("FAIL {failed_step}");
            if p.counts && !counts.is_empty() {
                head.push_str(&format!(" ({})", counts.join(", ")));
            }
            let mut parts = vec![head];
            if let Some(c) = &crates_shown {
                parts.push(c.clone());
            }
            parts.push(secs);
            let mut lines = vec![parts.join(" · ")];

            // Errors.
            let mut errors_json = Vec::new();
            for (i, e) in prepared.errors.iter().take(shown_errors).enumerate() {
                let (item_lines, item_json) = render_error(i + 1, e, p, w);
                lines.extend(item_lines);
                errors_json.push(item_json);
            }
            if p.summary && hidden_errors > 0 {
                lines.push(w.more_errors(hidden_errors));
            }

            // Failed tests.
            let mut tests_json = Vec::new();
            for (i, t) in prepared.tests.iter().take(shown_tests).enumerate() {
                let out_lines = trimmed_output(&t.output, p.test_lines, p.width);
                lines.push(format!("[T{}] {}", i + 1, t.name));
                for l in &out_lines {
                    lines.push(format!("    {l}"));
                }
                tests_json.push(json!({
                    "index": i + 1,
                    "name": t.name,
                    "crate": t.crate_id.name,
                    "output": out_lines,
                }));
            }
            if p.summary && hidden_tests > 0 {
                lines.push(w.more_tests(hidden_tests));
            }

            // Warnings (one line each).
            let mut warnings_json = Vec::new();
            if p.warnings {
                for (i, e) in prepared.warnings.iter().enumerate() {
                    lines.push(format!("[W{}] {}", i + 1, headline(e, p.width, w)));
                    warnings_json.push(item_json(json!(format!("W{}", i + 1)), e, p.width));
                }
            }

            let json = json!({
                "outcome": "fail",
                "failed_step": failed_step.as_str(),
                "crates": crates,
                "full_rebuild": result.impact.full_rebuild,
                "duration_ms": result.total_ms,
                "cache_hit": result.cache_hit,
                "steps": result.steps.iter().map(|s| s.step.as_str()).collect::<Vec<_>>(),
                "total": {
                    "errors": prepared.errors.len(),
                    "warnings": prepared.warnings.len(),
                    "test_failures": prepared.tests.len(),
                },
                "errors": errors_json,
                "test_failures": tests_json,
                "warnings": warnings_json,
                "hidden": {
                    "errors": hidden_errors,
                    "warnings": hidden_warnings,
                    "test_failures": hidden_tests,
                },
            });
            (lines.join("\n"), json)
        }
    }
}

/// Names of the affected crates (empty when a full rebuild covers everything).
fn crate_names(result: &VerifyResult) -> Vec<String> {
    if result.impact.full_rebuild {
        return Vec::new();
    }
    result
        .impact
        .affected()
        .into_iter()
        .map(|c| c.name)
        .collect()
}

/// `crates: a, b, +3` (or `crates: workspace`); `None` when there is nothing to say.
fn crate_list(names: &[String], full_rebuild: bool, max: usize, w: &Words) -> Option<String> {
    if max == 0 {
        return None;
    }
    if full_rebuild {
        return Some(format!("{} {}", w.crates, w.workspace));
    }
    if names.is_empty() {
        return None;
    }
    let mut shown: Vec<String> = names.iter().take(max).cloned().collect();
    if names.len() > max {
        shown.push(format!("+{}", names.len() - max));
    }
    Some(format!("{} {}", w.crates, shown.join(", ")))
}

/// Shortens `s` to at most `max` chars, ending in `…` when cut.
fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Collapses whitespace runs (and newlines) into single spaces.
fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `CODE message — file:line:col (possibly derived from [k])`.
fn headline(e: &Entry<'_>, width: usize, w: &Words) -> String {
    let d = e.diag;
    let mut s = String::new();
    if let Some(code) = &d.code {
        s.push_str(code);
        s.push(' ');
    }
    s.push_str(&clip(&one_line(&d.message), width.max(40) * 2));
    if let Some(loc) = location(d) {
        s.push_str(" — ");
        s.push_str(&loc);
    }
    if let Some(k) = e.derived_from {
        s.push_str(&format!(" ({} [{k}])", w.derived));
    }
    s
}

fn location(d: &Diagnostic) -> Option<String> {
    let file = d.file.as_ref()?;
    let mut s = file.display().to_string();
    if let Some(line) = d.line {
        s.push_str(&format!(":{line}"));
        if let Some(col) = d.column {
            s.push_str(&format!(":{col}"));
        }
    }
    Some(s)
}

fn item_json(index: Value, e: &Entry<'_>, width: usize) -> Value {
    let d = e.diag;
    json!({
        "index": index,
        "code": d.code,
        "message": clip(&one_line(&d.message), width.max(40) * 2),
        "file": d.file.as_ref().map(|f| f.display().to_string()),
        "line": d.line,
        "column": d.column,
        "derived_from": e.derived_from,
    })
}

/// Renders one error: headline, snippet with gutter, suggestion.
fn render_error(index: usize, e: &Entry<'_>, p: &Params, w: &Words) -> (Vec<String>, Value) {
    let d = e.diag;
    let mut lines = vec![format!("[{index}] {}", headline(e, p.width, w))];
    let mut item = item_json(json!(index), e, p.width);

    let snippet: Vec<String> = match (&d.snippet, p.snippet_lines) {
        (Some(s), n) if n > 0 => {
            let all: Vec<&str> = s.lines().map(str::trim_end).collect();
            let start = all.iter().position(|l| !l.is_empty()).unwrap_or(0);
            let end = all.iter().rposition(|l| !l.is_empty()).map_or(0, |i| i + 1);
            all.get(start..end)
                .unwrap_or(&[])
                .iter()
                .take(n)
                .map(|l| clip(l, p.width))
                .collect()
        }
        _ => Vec::new(),
    };
    if !snippet.is_empty() {
        let last_no = d.line.map(|l| l.saturating_add(snippet.len() as u32 - 1));
        let gutter = last_no.map_or(0, |n| n.to_string().len());
        for (i, l) in snippet.iter().enumerate() {
            let no = d
                .line
                .map_or_else(String::new, |n| n.saturating_add(i as u32).to_string());
            lines.push(format!("    {no:>gutter$} | {l}"));
            // Caret under the first line, where the primary span starts.
            if i == 0 {
                if let Some(col) = d.column {
                    let col = (col as usize).saturating_sub(1);
                    if col < p.width {
                        lines.push(format!("    {:>gutter$} | {}^", "", " ".repeat(col)));
                    }
                }
            }
        }
        item["snippet"] = json!(snippet);
    }
    if p.suggestions {
        if let Some(s) = &d.suggestion {
            let s = clip(&one_line(s), p.width);
            lines.push(format!("    {} {s}", w.suggestion));
            item["suggestion"] = json!(s);
        }
    }
    (lines, item)
}

/// First `max_lines` non-empty lines of a test's output, each clipped.
fn trimmed_output(output: &str, max_lines: usize, width: usize) -> Vec<String> {
    output
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .take(max_lines)
        .map(|l| clip(l, width))
        .collect()
}
