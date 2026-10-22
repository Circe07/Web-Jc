//! Parsing of `cargo check/clippy --message-format=json` output and of
//! `cargo fmt --check` output into [`Diagnostic`]s.

use std::path::{Path, PathBuf};

use orq_core::{Diagnostic, Severity, Step};
use serde::Deserialize;

use crate::index::CrateIndex;
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

fn suggestion(msg: &RustcMessage) -> Option<String> {
    msg.children
        .iter()
        .flat_map(|c| c.spans.iter())
        .chain(msg.spans.iter())
        .filter_map(|s| s.suggested_replacement.as_deref())
        .find(|r| !r.is_empty())
        .map(str::to_string)
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
            out.extend(to_diagnostic(&line, msg, step, index));
        }
    }
    out
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
