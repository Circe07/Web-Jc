//! Parsing of `cargo check/clippy --message-format=json` output and of
//! `cargo fmt --check` output into [`Diagnostic`]s.

use std::path::{Path, PathBuf};

use orq_core::{Diagnostic, Severity, Step};
use serde::Deserialize;

use crate::index::CrateIndex;
use crate::signatures;
use crate::util::strip_ansi;

/// One line of cargo's JSON stream (only the fields we need).
#[derive(Deserialize)]
struct CargoLine {
    reason: String,
    package_id: Option<String>,
    manifest_path: Option<PathBuf>,
    message: Option<RustcMessage>,
}

#[derive(Deserialize)]
struct RustcMessage {
    message: String,
    code: Option<RustcCode>,
    level: String,
    #[serde(default)]
    spans: Vec<Span>,
    #[serde(default)]
    children: Vec<RustcMessage>,
}

#[derive(Deserialize)]
struct RustcCode {
    code: String,
}

#[derive(Deserialize)]
struct Span {
    file_name: String,
    line_start: u32,
    column_start: u32,
    #[serde(default)]
    is_primary: bool,
    #[serde(default)]
    text: Vec<SpanText>,
    suggested_replacement: Option<String>,
    #[serde(default)]
    label: Option<String>,
    expansion: Option<Box<Expansion>>,
}

#[derive(Deserialize)]
struct SpanText {
    text: String,
    /// 1-based char columns of the span within `text`.
    #[serde(default)]
    highlight_start: usize,
    #[serde(default)]
    highlight_end: usize,
}

#[derive(Deserialize)]
struct Expansion {
    span: Span,
}

impl Span {
    /// Follows macro expansions out of pseudo files such as `<std macros>`.
    fn call_site(&self) -> &Span {
        let mut span = self;
        while span.file_name.starts_with('<') {
            match &span.expansion {
                Some(exp) => span = &exp.span,
                None => break,
            }
        }
        span
    }
}

fn severity(level: &str) -> Option<Severity> {
    match level {
        "error" | "error: internal compiler error" => Some(Severity::Error),
        "warning" => Some(Severity::Warning),
        "note" => Some(Severity::Note),
        "help" => Some(Severity::Help),
        _ => None,
    }
}

/// `true` for cargo/rustc summary lines that carry no information.
fn is_summary(message: &str) -> bool {
    message.starts_with("aborting due to")
        || message.starts_with("could not compile")
        || ((message.ends_with(" emitted") || message.contains(" emitted (")) && {
            message.contains("warning") || message.contains("error")
        })
}

/// The first suggestion of `msg`, with its context when rustc gives it.
///
/// A suggested replacement alone is a fragment (`: Ord`, `+ Clone`) that says
/// neither where it goes nor why, so when the help's spans sit on one source
/// line the result is that line with the edits applied, then the help text
/// (code first, so clipping a long line drops the prose, not the fix):
/// ``line 8: `fn f<T: Ord>()` (consider restricting type parameter `T` with trait `Ord`)``.
fn suggestion(msg: &RustcMessage) -> Option<String> {
    let has_edit = |s: &Span| s.suggested_replacement.is_some();
    if let Some(help) = msg.children.iter().find(|c| c.spans.iter().any(has_edit)) {
        if let Some((line, patched)) = patched_line(&help.spans) {
            return Some(format!(
                "line {line}: `{}` ({})",
                patched.trim(),
                help.message.trim_end_matches(':')
            ));
        }
    }
    msg.children
        .iter()
        .flat_map(|c| c.spans.iter())
        .chain(msg.spans.iter())
        .filter_map(|s| s.suggested_replacement.as_deref())
        .find(|r| !r.is_empty())
        .map(str::to_string)
}

/// The source line of `spans` with every suggested replacement applied, when
/// they all edit the same single line; `None` otherwise.
fn patched_line(spans: &[Span]) -> Option<(u32, String)> {
    let mut edits: Vec<(&Span, &SpanText, &str)> = Vec::new();
    for s in spans {
        let Some(replacement) = s.suggested_replacement.as_deref() else {
            continue;
        };
        let [text] = s.text.as_slice() else {
            return None;
        };
        if replacement.contains('\n') || text.highlight_start == 0 {
            return None;
        }
        edits.push((s, text, replacement));
    }
    let &(first, source, _) = edits.first()?;
    if edits
        .iter()
        .any(|(s, t, _)| s.line_start != first.line_start || t.text != source.text)
    {
        return None;
    }
    let mut chars: Vec<char> = source.text.chars().collect();
    // Right to left, so earlier columns stay valid.
    edits.sort_by_key(|(_, t, _)| std::cmp::Reverse(t.highlight_start));
    for (_, t, replacement) in edits {
        let (start, end) = (t.highlight_start - 1, t.highlight_end.saturating_sub(1));
        if start > end || end > chars.len() {
            return None;
        }
        chars.splice(start..end, replacement.chars());
    }
    Some((first.line_start, chars.into_iter().collect()))
}

fn to_diagnostic(
    line: &CargoLine,
    msg: &RustcMessage,
    step: Step,
    index: &CrateIndex,
) -> Option<Diagnostic> {
    let severity = severity(&msg.level)?;
    if is_summary(&msg.message) {
        return None;
    }
    let primary_span = msg.spans.iter().find(|s| s.is_primary);
    let primary = primary_span.map(Span::call_site);
    let label = primary_span
        .and_then(|s| s.label.as_deref())
        .or_else(|| primary.and_then(|s| s.label.as_deref()))
        .filter(|l| !l.trim().is_empty())
        .map(str::to_string);
    let snippet = primary
        .map(|s| {
            s.text
                .iter()
                .map(|t| t.text.as_str())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .filter(|s| !s.trim().is_empty());
    Some(Diagnostic {
        severity,
        code: msg.code.as_ref().map(|c| c.code.clone()),
        message: msg.message.clone(),
        file: primary.map(|s| index.relative(Path::new(&s.file_name))),
        line: primary.map(|s| s.line_start),
        column: primary.map(|s| s.column_start),
        snippet,
        label,
        suggestion: suggestion(msg),
        crate_id: index.resolve(line.manifest_path.as_deref(), line.package_id.as_deref()),
        step,
    })
}

/// The signature of the item rustc says is `defined here` (a `note` child of
/// the error), as the companion that follows the error. Only workspace
/// sources qualify: std and registry paths are absolute.
fn definition_companion(
    msg: &RustcMessage,
    error: &Diagnostic,
    index: &CrateIndex,
) -> Option<Diagnostic> {
    let child = msg
        .children
        .iter()
        .find(|c| c.level == "note" && c.message.ends_with(" defined here"))?;
    let span = child
        .spans
        .iter()
        .find(|s| s.is_primary)
        .or_else(|| child.spans.first())?;
    if span.file_name.starts_with('<') {
        return None;
    }
    let file = index.relative(Path::new(&span.file_name));
    if !file.is_relative() || file.starts_with("..") {
        return None;
    }
    let source = std::fs::read_to_string(index.root().join(&file)).ok()?;
    let lines: Vec<String> = source.lines().map(str::to_string).collect();
    let at = usize::try_from(span.line_start).ok()?.checked_sub(1)?;
    let signature = signatures::signature_from(&lines, at)?;
    Some(signatures::companion(
        error,
        signature,
        file,
        span.line_start,
    ))
}

/// Parses the stdout of a cargo command run with `--message-format=json`.
///
/// Lines that are not compiler messages (artifacts, build-script output,
/// anything not JSON) are ignored.
pub(crate) fn parse_compiler_messages(
    stdout: &str,
    step: Step,
    index: &CrateIndex,
) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for raw in stdout.lines() {
        if !raw.starts_with('{') {
            continue;
        }
        let line: CargoLine = match serde_json::from_str(raw) {
            Ok(line) => line,
            Err(e) => {
                tracing::debug!("skipping unparsable cargo line: {e}");
                continue;
            }
        };
        if line.reason != "compiler-message" {
            continue;
        }
        if let Some(msg) = &line.message {
            let diagnostic = to_diagnostic(&line, msg, step, index);
            let companion = diagnostic
                .as_ref()
                .filter(|d| d.severity == Severity::Error)
                .and_then(|d| definition_companion(msg, d, index));
            out.extend(diagnostic);
            out.extend(companion);
        }
    }
    out
}

/// Parses the output of a single `cargo clippy --message-format=json` run,
/// assigning each diagnostic to [`Step::Clippy`] if its code starts with
/// `clippy::` and to [`Step::Check`] otherwise.
pub(crate) fn parse_clippy_messages(stdout: &str, index: &CrateIndex) -> Vec<Diagnostic> {
    let mut diagnostics = parse_compiler_messages(stdout, Step::Check, index);
    for d in &mut diagnostics {
        if d.code.as_deref().is_some_and(|c| c.starts_with("clippy::")) {
            d.step = Step::Clippy;
        }
    }
    diagnostics
}

/// Parses `cargo fmt -- --check` output (`Diff in <file>:<line>:` hunks) into
/// warnings.
pub(crate) fn parse_fmt_diffs(output: &str, index: &CrateIndex) -> Vec<Diagnostic> {
    struct Hunk {
        file: PathBuf,
        line: u32,
        body: Vec<String>,
    }
    let mut hunks: Vec<Hunk> = Vec::new();
    for raw in strip_ansi(output).lines() {
        if let Some(rest) = raw.strip_prefix("Diff in ") {
            if let Some((file, line)) = parse_diff_header(rest) {
                hunks.push(Hunk {
                    file: PathBuf::from(file),
                    line,
                    body: Vec::new(),
                });
                continue;
            }
        }
        if let Some(h) = hunks.last_mut() {
            h.body.push(raw.to_string());
        }
    }
    hunks
        .into_iter()
        .map(|h| {
            let snippet = h.body.join("\n").trim_end().to_string();
            Diagnostic {
                severity: Severity::Warning,
                code: None,
                message: "file is not formatted according to rustfmt".to_string(),
                file: Some(index.relative(&h.file)),
                line: Some(h.line),
                column: None,
                snippet: (!snippet.is_empty()).then_some(snippet),
                label: None,
                suggestion: None,
                crate_id: index.for_path(&h.file),
                step: Step::Fmt,
            }
        })
        .collect()
}

/// Parses `<file>:<line>:` (current rustfmt) or `<file> at line <line>:`.
fn parse_diff_header(rest: &str) -> Option<(&str, u32)> {
    let rest = rest.trim_end().strip_suffix(':')?;
    if let Some((file, line)) = rest.rsplit_once(" at line ") {
        return Some((file, line.trim().parse().ok()?));
    }
    let (file, line) = rest.rsplit_once(':')?;
    Some((file, line.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use orq_core::CrateId;

    fn index() -> CrateIndex {
        CrateIndex::new(
            Path::new("/ws"),
            vec![CrateId {
                name: "demo".into(),
                manifest_path: PathBuf::from("/ws/Cargo.toml"),
            }],
        )
    }

    #[test]
    fn parses_error_with_suggestion_and_skips_summary() {
        let msg = r#"{"reason":"compiler-message","package_id":"path+file:///ws#demo@0.1.0","manifest_path":"/ws/Cargo.toml","target":{"name":"demo"},"message":{"rendered":"x","$message_type":"diagnostic","children":[{"children":[],"code":null,"level":"help","message":"try this","rendered":null,"spans":[{"file_name":"src/lib.rs","line_start":3,"column_start":1,"is_primary":true,"text":[],"suggested_replacement":"fix()","expansion":null}]}],"level":"error","message":"mismatched types","spans":[{"file_name":"src/lib.rs","line_start":3,"column_start":5,"is_primary":true,"text":[{"text":"    let x = 1;"}],"suggested_replacement":null,"expansion":null}],"code":{"code":"E0308","explanation":null}}}
{"reason":"compiler-message","package_id":"path+file:///ws#demo@0.1.0","manifest_path":"/ws/Cargo.toml","message":{"rendered":"","children":[],"level":"error","message":"aborting due to 1 previous error","spans":[],"code":null}}
{"reason":"compiler-artifact","package_id":"x"}
not json at all
"#;
        let diags = parse_compiler_messages(msg, Step::Check, &index());
        assert_eq!(diags.len(), 1);
        let d = &diags[0];
        assert_eq!(d.code.as_deref(), Some("E0308"));
        assert_eq!(d.file.as_deref(), Some(Path::new("src/lib.rs")));
        assert_eq!((d.line, d.column), (Some(3), Some(5)));
        assert_eq!(d.snippet.as_deref(), Some("    let x = 1;"));
        assert_eq!(d.suggestion.as_deref(), Some("fix()"));
        assert_eq!(d.crate_id.as_ref().map(|c| c.name.as_str()), Some("demo"));
    }

    #[test]
    fn suggestion_shows_help_and_patched_line() {
        let msg = r#"{"reason":"compiler-message","package_id":"path+file:///ws#demo@0.1.0","manifest_path":"/ws/Cargo.toml","target":{"name":"demo"},"message":{"rendered":"x","$message_type":"diagnostic","children":[{"children":[],"code":null,"level":"help","message":"consider restricting type parameter `T` with trait `Ord`","rendered":null,"spans":[{"file_name":"src/lib.rs","line_start":8,"column_start":14,"is_primary":true,"text":[{"text":"pub fn mode<T>(values: &[T]) -> Option<T> {","highlight_start":14,"highlight_end":14}],"suggested_replacement":": std::cmp::Ord","expansion":null}]}],"level":"error","message":"the trait bound `T: Ord` is not satisfied","spans":[{"file_name":"src/lib.rs","line_start":18,"column_start":10,"is_primary":true,"text":[{"text":"        .min()","highlight_start":10,"highlight_end":13}],"suggested_replacement":null,"expansion":null}],"code":{"code":"E0277","explanation":null}}}"#;
        let diags = parse_compiler_messages(msg, Step::Check, &index());
        assert_eq!(
            diags[0].suggestion.as_deref(),
            Some(
                "line 8: `pub fn mode<T: std::cmp::Ord>(values: &[T]) -> Option<T> {` \
                 (consider restricting type parameter `T` with trait `Ord`)"
            )
        );
    }

    #[test]
    fn summary_detection() {
        assert!(is_summary("aborting due to 2 previous errors"));
        assert!(is_summary("3 warnings emitted"));
        assert!(is_summary("1 warning emitted"));
        assert!(!is_summary("unused variable: `x`"));
    }

    #[test]
    fn parses_fmt_diff_headers() {
        let out = "Diff in /ws/src/lib.rs:2:\n-fn  a(){}\n+fn a() {}\n \nDiff in /ws/src/b.rs at line 9:\n-x\n+y\n";
        let diags = parse_fmt_diffs(out, &index());
        assert_eq!(diags.len(), 2);
        assert_eq!(diags[0].file.as_deref(), Some(Path::new("src/lib.rs")));
        assert_eq!(diags[0].line, Some(2));
        assert_eq!(diags[0].severity, Severity::Warning);
        assert_eq!(diags[0].snippet.as_deref(), Some("-fn  a(){}\n+fn a() {}"));
        assert_eq!(diags[1].line, Some(9));
        assert_eq!(
            diags[0].crate_id.as_ref().map(|c| c.name.as_str()),
            Some("demo")
        );
    }
}
