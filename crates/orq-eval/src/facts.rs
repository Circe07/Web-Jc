//! Case format (`case.toml`) and fact matching against report text.

use std::path::Path;

use serde::Deserialize;

use crate::EvalError;

/// One fact a useful report must contain. Every field is optional; all the
/// fields that are set must be found for the fact to count as found.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Fact {
    /// Error or lint code, e.g. `E0308`.
    pub code: Option<String>,
    /// File of the primary span, e.g. `src/lib.rs`.
    pub file: Option<String>,
    /// 1-based line of the primary span (matched as `file:line`).
    pub line: Option<u32>,
    /// Substring of the primary span label.
    pub label: Option<String>,
    /// Name of a failing test.
    pub test: Option<String>,
}

/// A parsed `case.toml`.
#[derive(Debug, Clone, Deserialize)]
pub struct CaseSpec {
    /// Same as the directory name.
    pub id: String,
    /// Free-text category, used to group results.
    #[serde(default)]
    pub category: String,
    /// `easy`, `medium` or `hard`.
    #[serde(default)]
    pub difficulty: String,
    /// Last step that must pass after the fix: `check`, `clippy` or `test`.
    pub up_to: String,
    /// Step where the broken project fails: `fmt`, `check`, `clippy`, `test`.
    pub expected_fail_step: String,
    /// Facts a useful report must contain.
    #[serde(default)]
    pub facts: Vec<Fact>,
}

impl CaseSpec {
    /// Reads and parses a `case.toml`.
    pub fn load(path: &Path) -> Result<Self, EvalError> {
        let text = std::fs::read_to_string(path).map_err(|e| EvalError::io(path, e))?;
        toml::from_str(&text).map_err(|e| EvalError::Case {
            path: path.to_path_buf(),
            message: e.to_string(),
        })
    }
}

impl Fact {
    /// Human-readable description, e.g. `E0308 src/lib.rs:6 "expected `u64`"`.
    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
        if let Some(code) = &self.code {
            parts.push(code.clone());
        }
        match (&self.file, self.line) {
            (Some(f), Some(l)) => parts.push(format!("{f}:{l}")),
            (Some(f), None) => parts.push(f.clone()),
            (None, Some(l)) => parts.push(format!("line {l}")),
            (None, None) => {}
        }
        if let Some(label) = &self.label {
            parts.push(format!("\"{label}\""));
        }
        if let Some(test) = &self.test {
            parts.push(format!("test {test}"));
        }
        parts.join(" ")
    }

    /// The parts of this fact that `text` does not contain (empty = found).
    pub fn missing_parts(&self, text: &str) -> Vec<String> {
        let mut missing = Vec::new();
        if let Some(code) = &self.code {
            if !text.contains(code.as_str()) {
                missing.push(format!("code {code}"));
            }
        }
        match (&self.file, self.line) {
            (Some(file), Some(line)) => {
                if !contains_location(text, file, line) {
                    missing.push(format!("location {file}:{line}"));
                }
            }
            (Some(file), None) if !text.contains(file.as_str()) => {
                missing.push(format!("file {file}"));
            }
            _ => {}
        }
        if let Some(label) = &self.label {
            if !text.contains(label.as_str()) {
                missing.push(format!("label \"{label}\""));
            }
        }
        if let Some(test) = &self.test {
            if !text.contains(test.as_str()) {
                missing.push(format!("test {test}"));
            }
        }
        missing
    }

    /// `true` if every field that is set is found in `text`.
    pub fn found_in(&self, text: &str) -> bool {
        self.missing_parts(text).is_empty()
    }
}

/// `true` if `text` has `file:line` not followed by another digit (so `:6`
/// does not match `:60`).
pub fn contains_location(text: &str, file: &str, line: u32) -> bool {
    let needle = format!("{file}:{line}");
    let mut from = 0;
    while let Some(pos) = text[from..].find(&needle) {
        let end = from + pos + needle.len();
        if !text[end..].starts_with(|c: char| c.is_ascii_digit()) {
            return true;
        }
        from = end;
    }
    false
}

/// Result of matching a fact list against one text.
#[derive(Debug, Clone, PartialEq)]
pub struct Recall {
    /// Facts found.
    pub found: usize,
    /// Facts in the case.
    pub total: usize,
    /// Descriptions of the facts not found, with the missing parts.
    pub missing: Vec<String>,
}

impl Recall {
    /// Fraction of facts found; `1.0` when the case has no facts.
    pub fn ratio(&self) -> f64 {
        if self.total == 0 {
            1.0
        } else {
            self.found as f64 / self.total as f64
        }
    }
}

/// Matches `facts` against `text`.
pub fn recall(facts: &[Fact], text: &str) -> Recall {
    let mut found = 0;
    let mut missing = Vec::new();
    for fact in facts {
        let parts = fact.missing_parts(text);
        if parts.is_empty() {
            found += 1;
        } else {
            missing.push(format!(
                "{} (missing {})",
                fact.describe(),
                parts.join(", ")
            ));
        }
    }
    Recall {
        found,
        total: facts.len(),
        missing,
    }
}

/// The first error or lint code that appears in `text`: `E` plus four digits,
/// or `clippy::<lint>`.
pub fn first_code(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut best: Option<(usize, String)> = None;
    let mut i = 0;
    while i + 5 <= bytes.len() {
        let boundary = i == 0 || !bytes[i - 1].is_ascii_alphanumeric();
        if boundary
            && bytes[i] == b'E'
            && bytes[i + 1..i + 5].iter().all(u8::is_ascii_digit)
            && !bytes.get(i + 5).is_some_and(u8::is_ascii_alphanumeric)
        {
            best = Some((i, text[i..i + 5].to_string()));
            break;
        }
        i += 1;
    }
    if let Some(pos) = text.find("clippy::") {
        if best.as_ref().is_none_or(|(p, _)| pos < *p) {
            let rest = &text[pos + "clippy::".len()..];
            let len = rest
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .unwrap_or(rest.len());
            best = Some((pos, format!("clippy::{}", &rest[..len])));
        }
    }
    best.map(|(_, code)| code)
}

/// Whether the first error shown in `text` is the first fact with a code.
/// `None` when the case has no fact with a code.
pub fn first_error_correct(facts: &[Fact], text: &str) -> Option<bool> {
    let expected = facts.iter().find_map(|f| f.code.as_deref())?;
    Some(first_code(text).as_deref() == Some(expected))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fact(code: &str, file: &str, line: u32, label: &str) -> Fact {
        Fact {
            code: Some(code.into()),
            file: Some(file.into()),
            line: Some(line),
            label: Some(label.into()),
            test: None,
        }
    }

    #[test]
    fn location_respects_digit_boundary() {
        assert!(contains_location("--> src/lib.rs:6:9", "src/lib.rs", 6));
        assert!(!contains_location("--> src/lib.rs:60:9", "src/lib.rs", 6));
        assert!(contains_location(
            "a src/lib.rs:60 b src/lib.rs:6",
            "src/lib.rs",
            6
        ));
        assert!(!contains_location("src/main.rs:6", "src/lib.rs", 6));
    }

    #[test]
    fn fact_needs_every_field() {
        let f = fact("E0308", "src/lib.rs", 6, "expected `u64`");
        let full = "error[E0308]: mismatched\n --> src/lib.rs:6:5\n expected `u64`, found";
        assert!(f.found_in(full));
        let no_label = "error[E0308]: mismatched\n --> src/lib.rs:6:5";
        assert_eq!(f.missing_parts(no_label), ["label \"expected `u64`\""]);
        let wrong_line = "E0308 src/lib.rs:7 expected `u64`";
        assert_eq!(f.missing_parts(wrong_line), ["location src/lib.rs:6"]);
    }

    #[test]
    fn test_fact_and_empty_fact() {
        let f = Fact {
            test: Some("tests::adds".into()),
            ..Fact::default()
        };
        assert!(f.found_in("FAILED tests::adds"));
        assert!(!f.found_in("FAILED tests::subs"));
        assert!(Fact::default().found_in(""));
    }

    #[test]
    fn recall_counts_and_describes_missing() {
        let facts = vec![
            fact("E0308", "src/lib.rs", 6, "x"),
            Fact {
                test: Some("t::a".into()),
                ..Fact::default()
            },
        ];
        let r = recall(&facts, "E0308 src/lib.rs:6 x");
        assert_eq!((r.found, r.total), (1, 2));
        assert!((r.ratio() - 0.5).abs() < 1e-9);
        assert_eq!(r.missing, ["test t::a (missing test t::a)"]);
        assert!((recall(&[], "").ratio() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn first_code_finds_earliest() {
        assert_eq!(
            first_code("error[E0599]: x\nerror[E0308]").as_deref(),
            Some("E0599")
        );
        assert_eq!(
            first_code("see clippy::needless_return, E0308").as_deref(),
            Some("clippy::needless_return")
        );
        assert_eq!(first_code("XE0308 E03088 nothing"), None);
        assert_eq!(first_code("no codes"), None);
    }

    #[test]
    fn first_error_correct_uses_first_coded_fact() {
        let facts = vec![
            Fact {
                test: Some("t".into()),
                ..Fact::default()
            },
            fact("E0308", "a.rs", 1, "l"),
        ];
        assert_eq!(first_error_correct(&facts, "E0308 then E0599"), Some(true));
        assert_eq!(first_error_correct(&facts, "E0599 then E0308"), Some(false));
        assert_eq!(first_error_correct(&facts, "nothing"), Some(false));
        let none = vec![Fact::default()];
        assert_eq!(first_error_correct(&none, "E0308"), None);
    }

    #[test]
    fn case_spec_parses_seed_format() {
        let toml = r#"
id = "x"
up_to = "check"
expected_fail_step = "check"
[[facts]]
code = "E0308"
line = 6
[[facts]]
test = "tests::parses"
"#;
        let spec: CaseSpec = toml::from_str(toml).unwrap();
        assert_eq!(spec.facts.len(), 2);
        assert_eq!(spec.facts[0].line, Some(6));
        assert_eq!(spec.category, "");
    }
}
