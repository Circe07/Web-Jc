//! Signatures of the workspace items an error talks about, so the agent does
//! not have to open the definition (one turn) before it can fix a call.
//!
//! The signature travels as a companion [`Diagnostic`] that directly follows
//! its error: severity [`Severity::Help`], code [`SIGNATURE_CODE`], the
//! one-line signature as message and the definition's `file:line`. The
//! compressor pairs them. (A dedicated field in `orq-core` would be cleaner;
//! until then the companion keeps the contract untouched.)
//!
//! Two sources, best first:
//!
//! 1. rustc's own `note: function defined here` child of the error, which
//!    names the exact definition (calls with the wrong arguments).
//! 2. A best-effort textual lookup of `fn name(` / `struct Name` in the
//!    workspace sources, for errors that only name the item in their message
//!    (`no method named x found`, `struct Foo has no field named y`...). It is
//!    used only when the lookup is unambiguous.

use std::path::{Path, PathBuf};

use orq_core::{Diagnostic, Severity};

/// `Diagnostic::code` of a signature companion.
pub const SIGNATURE_CODE: &str = "orq::signature";

/// Most errors that get a lookup per run (each costs a source scan).
const MAX_LOOKUPS: usize = 4;
/// Longest signature kept, in chars.
const MAX_SIGNATURE_CHARS: usize = 160;
/// Source lines read for one signature.
const MAX_SIGNATURE_LINES: usize = 8;
/// Bounds of the workspace scan.
const MAX_FILES: usize = 4000;
const MAX_FILE_BYTES: u64 = 512 * 1024;

/// What a lookup searches for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Wanted {
    /// `fn name`, preferring the ones inside an `impl` of `owner`.
    Function { name: String, owner: Option<String> },
    /// `struct Name`.
    Struct(String),
}

/// A signature companion for the definition at `file:line`.
pub(crate) fn companion(
    error: &Diagnostic,
    signature: String,
    file: PathBuf,
    line: u32,
) -> Diagnostic {
    Diagnostic {
        severity: Severity::Help,
        code: Some(SIGNATURE_CODE.to_string()),
        message: signature,
        file: Some(file),
        line: Some(line),
        column: None,
        snippet: None,
        label: None,
        suggestion: None,
        crate_id: error.crate_id.clone(),
        step: error.step,
    }
}

/// `true` for a signature companion.
pub(crate) fn is_companion(d: &Diagnostic) -> bool {
    d.severity == Severity::Help && d.code.as_deref() == Some(SIGNATURE_CODE)
}

/// What an error message asks for, if it names a workspace item.
pub(crate) fn wanted_from(code: Option<&str>, message: &str) -> Option<Wanted> {
    let ticks = backticked(message);
    match code? {
        "E0599" => {
            let name = if message.starts_with("no method named")
                || message.starts_with("no function or associated item named")
            {
                ticks.first()?.clone()
            } else {
                return None;
            };
            // "... found for struct `Foo` in the current scope"
            let owner = message
                .split(" found for ")
                .nth(1)
                .map(backticked)
                .and_then(|t| t.into_iter().next())
                .map(|t| type_name(&t))
                .filter(|t| !t.is_empty());
            Some(Wanted::Function { name, owner })
        }
        "E0425" => {
            let rest = message.strip_prefix("cannot find function")?;
            let name = backticked(rest).into_iter().next()?;
            Some(Wanted::Function { name, owner: None })
        }
        "E0560" | "E0559" => {
            // "struct `Foo` has no field named `x`"
            let name = type_name(ticks.first()?);
            (!name.is_empty()).then_some(Wanted::Struct(name))
        }
        "E0063" => {
            // "missing field `x` in initializer of `Foo`"
            let after = message.split("in initializer of").nth(1)?;
            let name = type_name(&backticked(after).into_iter().next()?);
            (!name.is_empty()).then_some(Wanted::Struct(name))
        }
        "E0609" => {
            // "no field `x` on type `Foo`"
            let after = message.split(" on type ").nth(1)?;
            let name = type_name(&backticked(after).into_iter().next()?);
            (!name.is_empty()).then_some(Wanted::Struct(name))
        }
        _ => None,
    }
}

/// The texts between backticks, in order.
fn backticked(message: &str) -> Vec<String> {
    message
        .split('`')
        .skip(1)
        .step_by(2)
        .map(str::to_string)
        .collect()
}

/// The last path segment of a type, without references or generics:
/// `&mut crate::model::Task<T>` -> `Task`.
fn type_name(ty: &str) -> String {
    let base = ty.split('<').next().unwrap_or(ty);
    let last = base.rsplit("::").next().unwrap_or(base);
    last.trim_start_matches(['&', ' '])
        .trim_start_matches("mut ")
        .trim()
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect()
}

/// Adds signature companions after the errors that name a workspace item and
/// do not have one yet.
pub(crate) fn annotate(root: &Path, diagnostics: Vec<Diagnostic>) -> Vec<Diagnostic> {
    let wanted_at: Vec<Option<Wanted>> = diagnostics
        .iter()
        .enumerate()
        .map(|(i, d)| {
            let has_companion = diagnostics.get(i + 1).is_some_and(is_companion);
            if d.severity == Severity::Error && !has_companion {
                wanted_from(d.code.as_deref(), &d.message)
            } else {
                None
            }
        })
        .collect();
    if wanted_at.iter().all(Option::is_none) {
        return diagnostics;
    }

    let sources = Sources::load(root);
    let mut lookups = 0;
    let mut out = Vec::with_capacity(diagnostics.len() + 2);
    for (d, wanted) in diagnostics.into_iter().zip(wanted_at) {
        let found = match wanted {
            Some(w) if lookups < MAX_LOOKUPS => {
                lookups += 1;
                sources.find(&w)
            }
            _ => None,
        };
        let companion = found.map(|(file, line, sig)| companion(&d, sig, file, line));
        out.push(d);
        out.extend(companion);
    }
    out
}

/// The `.rs` files of the workspace, read once.
struct Sources {
    files: Vec<(PathBuf, Vec<String>)>,
}

impl Sources {
    fn load(root: &Path) -> Self {
        let mut files = Vec::new();
        collect(root, root, &mut files);
        Self { files }
    }

    /// `(file, 1-based line, signature)` of the unique definition, if any.
    fn find(&self, wanted: &Wanted) -> Option<(PathBuf, u32, String)> {
        let mut hits: Vec<(&PathBuf, usize, &[String])> = Vec::new();
        for (path, lines) in &self.files {
            for (i, line) in lines.iter().enumerate() {
                if defines(line, wanted) {
                    hits.push((path, i, lines));
                }
            }
        }
        // A method of another type would mislead: with a known owner only
        // the methods of its `impl` blocks qualify.
        if let Wanted::Function {
            owner: Some(owner), ..
        } = wanted
        {
            hits.retain(|(_, i, lines)| inside_impl_of(lines, *i, owner));
        }
        let [(path, i, lines)] = hits.as_slice() else {
            return None;
        };
        let signature = signature_from(lines, *i)?;
        Some((
            (*path).clone(),
            u32::try_from(*i + 1).unwrap_or(u32::MAX),
            signature,
        ))
    }
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<(PathBuf, Vec<String>)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        if out.len() >= MAX_FILES {
            return;
        }
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            if name != "target" && !name.starts_with('.') {
                collect(root, &path, out);
            }
        } else if kind.is_file() && name.ends_with(".rs") {
            if entry.metadata().is_ok_and(|m| m.len() > MAX_FILE_BYTES) {
                continue;
            }
            if let Ok(text) = std::fs::read_to_string(&path) {
                let rel = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
                out.push((rel, text.lines().map(str::to_string).collect()));
            }
        }
    }
}

/// Removes visibility and qualifiers in front of `fn` / `struct`.
fn strip_qualifiers(line: &str) -> &str {
    let mut rest = line.trim_start();
    loop {
        let before = rest;
        if let Some(r) = rest.strip_prefix("pub") {
            // `pub`, `pub(crate)`, `pub(in path)`.
            let r = r.trim_start();
            rest = match r.strip_prefix('(') {
                Some(inner) => inner.split_once(')').map_or(r, |(_, after)| after),
                None => r,
            };
        }
        for q in ["const ", "async ", "unsafe ", "default "] {
            if let Some(r) = rest.trim_start().strip_prefix(q) {
                rest = r;
            }
        }
        if let Some(r) = rest.trim_start().strip_prefix("extern ") {
            // `extern "C" fn`
            rest = r.trim_start();
            if let Some(r) = rest.strip_prefix('"') {
                rest = r.split_once('"').map_or(rest, |(_, after)| after);
            }
        }
        rest = rest.trim_start();
        if rest == before {
            return rest;
        }
    }
}

/// `true` if `line` starts the definition of `wanted`.
fn defines(line: &str, wanted: &Wanted) -> bool {
    let rest = strip_qualifiers(line);
    let (keyword, name) = match wanted {
        Wanted::Function { name, .. } => ("fn ", name),
        Wanted::Struct(name) => ("struct ", name),
    };
    let Some(after) = rest.strip_prefix(keyword) else {
        return false;
    };
    let after = after.trim_start();
    after.strip_prefix(name.as_str()).is_some_and(|tail| {
        !tail
            .chars()
            .next()
            .is_some_and(|c| c.is_alphanumeric() || c == '_')
    })
}

/// `true` if the item at line `at` sits in an `impl` block whose header
/// mentions `owner`.
fn inside_impl_of(lines: &[String], at: usize, owner: &str) -> bool {
    let indent = |l: &str| l.len() - l.trim_start().len();
    let own = indent(&lines[at]);
    for line in lines[..at].iter().rev() {
        if line.trim().is_empty() || indent(line) >= own {
            continue;
        }
        let header = line.trim_start();
        if header.starts_with("impl") {
            return header
                .split(|c: char| !(c.is_alphanumeric() || c == '_'))
                .any(|word| word == owner);
        }
        // A less indented, non-impl line (another item, `}`): not inside one.
        if own > 0 && indent(line) == 0 && !header.starts_with("impl") {
            return false;
        }
    }
    false
}

/// The one-line signature that starts at `lines[at]`.
///
/// Functions end at the opening brace, `;` or `where`; structs include their
/// fields when the body is short.
pub(crate) fn signature_from(lines: &[String], at: usize) -> Option<String> {
    let is_struct = strip_qualifiers(lines.get(at)?).starts_with("struct ");
    let mut text = String::new();
    for line in lines.iter().skip(at).take(MAX_SIGNATURE_LINES) {
        let piece = line.trim();
        if piece.starts_with("//") {
            continue;
        }
        if !text.is_empty() {
            text.push(' ');
        }
        text.push_str(piece);
        let done = if is_struct {
            text.contains('}') || text.ends_with(';')
        } else {
            text.contains('{') || text.ends_with(';') || text.contains(" where")
        };
        if done {
            break;
        }
    }
    let cut = if is_struct {
        // `struct A { x: u32 }` keeps its fields; a body that does not close
        // within the lines read (or tuple/unit structs) keeps just the header.
        match (text.find('{'), text.contains('}')) {
            (Some(open), false) => text[..open].trim().to_string(),
            _ => text.trim_end_matches(';').trim().to_string(),
        }
    } else {
        let end = [text.find('{'), text.find(" where"), text.find(';')]
            .into_iter()
            .flatten()
            .min()
            .unwrap_or(text.len());
        text[..end].trim().to_string()
    };
    let cut = cut.trim_end_matches(',').trim().to_string();
    let cut = cut
        .replace("( ", "(")
        .replace(", )", ")")
        .replace(",)", ")")
        .replace(", }", " }")
        .replace(",}", " }");
    if cut.is_empty() {
        return None;
    }
    Some(clip(&cut, MAX_SIGNATURE_CHARS))
}

fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(src: &str) -> Vec<String> {
        src.lines().map(str::to_string).collect()
    }

    #[test]
    fn signature_of_a_function() {
        let l = lines("    pub fn add(&mut self, a: u32, b: u32) -> u32 {\n        a + b\n    }");
        assert_eq!(
            signature_from(&l, 0).as_deref(),
            Some("pub fn add(&mut self, a: u32, b: u32) -> u32")
        );
    }

    #[test]
    fn signature_of_a_multiline_function() {
        let l = lines(
            "pub fn new(\n    name: &str,\n    due: Option<u32>,\n) -> Task\nwhere\n    T: Clone,\n{",
        );
        assert_eq!(
            signature_from(&l, 0).as_deref(),
            Some("pub fn new(name: &str, due: Option<u32>) -> Task")
        );
    }

    #[test]
    fn signature_of_a_trait_method_declaration() {
        let l = lines("    fn area(&self) -> f64;");
        assert_eq!(
            signature_from(&l, 0).as_deref(),
            Some("fn area(&self) -> f64")
        );
    }

    #[test]
    fn signature_of_structs() {
        let l = lines("pub struct Task {\n    pub id: u32,\n    pub title: String,\n}\n");
        assert_eq!(
            signature_from(&l, 0).as_deref(),
            Some("pub struct Task { pub id: u32, pub title: String }")
        );
        let l = lines("pub struct Id(u32);");
        assert_eq!(signature_from(&l, 0).as_deref(), Some("pub struct Id(u32)"));
    }

    #[test]
    fn long_signatures_are_clipped() {
        let l = lines(&format!("fn f({}) {{}}", "a: u32, ".repeat(60)));
        let s = signature_from(&l, 0).unwrap();
        assert_eq!(s.chars().count(), MAX_SIGNATURE_CHARS);
        assert!(s.ends_with('…'));
    }

    #[test]
    fn defines_matches_whole_names_and_qualifiers() {
        let f = |n: &str| Wanted::Function {
            name: n.into(),
            owner: None,
        };
        assert!(defines("pub fn add(a: u32)", &f("add")));
        assert!(defines("    pub(crate) async fn add<T>(a: T)", &f("add")));
        assert!(defines("pub const unsafe fn add()", &f("add")));
        assert!(defines("extern \"C\" fn add()", &f("add")));
        assert!(!defines("pub fn adder()", &f("add")));
        assert!(!defines("// fn add()", &f("add")));
        assert!(!defines("let fn_add = 1;", &f("add")));
        let s = Wanted::Struct("Task".into());
        assert!(defines("pub struct Task {", &s));
        assert!(!defines("pub struct TaskList {", &s));
        assert!(!defines("pub enum Task {", &s));
    }

    #[test]
    fn messages_map_to_wanted_items() {
        let f = |name: &str, owner: Option<&str>| {
            Some(Wanted::Function {
                name: name.into(),
                owner: owner.map(str::to_string),
            })
        };
        assert_eq!(
            wanted_from(
                Some("E0599"),
                "no method named `done` found for struct `Task` in the current scope"
            ),
            f("done", Some("Task"))
        );
        assert_eq!(
            wanted_from(
                Some("E0599"),
                "no function or associated item named `create` found for struct `crate::model::Task<T>` in the current scope"
            ),
            f("create", Some("Task"))
        );
        assert_eq!(
            wanted_from(Some("E0425"), "cannot find function `helper` in this scope"),
            f("helper", None)
        );
        assert_eq!(
            wanted_from(Some("E0425"), "cannot find value `x` in this scope"),
            None
        );
        assert_eq!(
            wanted_from(Some("E0560"), "struct `Task` has no field named `titel`"),
            Some(Wanted::Struct("Task".into()))
        );
        assert_eq!(
            wanted_from(
                Some("E0063"),
                "missing fields `a` and `b` in initializer of `Task`"
            ),
            Some(Wanted::Struct("Task".into()))
        );
        assert_eq!(
            wanted_from(Some("E0609"), "no field `nme` on type `&Task`"),
            Some(Wanted::Struct("Task".into()))
        );
        assert_eq!(wanted_from(Some("E0308"), "mismatched types"), None);
        assert_eq!(wanted_from(None, "no method named `x` found"), None);
    }

    fn tree(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (name, text) in files {
            let path = dir.path().join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        dir
    }

    fn error(code: &str, message: &str) -> Diagnostic {
        Diagnostic {
            severity: Severity::Error,
            code: Some(code.into()),
            message: message.into(),
            file: Some(PathBuf::from("src/main.rs")),
            line: Some(3),
            column: Some(1),
            snippet: None,
            label: None,
            suggestion: None,
            crate_id: None,
            step: orq_core::Step::Check,
        }
    }

    #[test]
    fn annotate_adds_a_unique_function_signature() {
        let dir = tree(&[
            (
                "src/lib.rs",
                "pub struct Task;\nimpl Task {\n    pub fn finish(&mut self, at: u64) -> bool {\n        true\n    }\n}\n",
            ),
            ("target/debug/junk.rs", "fn finish() {}\n"),
        ]);
        let diags = vec![error(
            "E0599",
            "no method named `finish` found for struct `Task` in the current scope",
        )];
        let out = annotate(dir.path(), diags);
        assert_eq!(out.len(), 2);
        assert!(is_companion(&out[1]));
        assert_eq!(out[1].message, "pub fn finish(&mut self, at: u64) -> bool");
        assert_eq!(out[1].file.as_deref(), Some(Path::new("src/lib.rs")));
        assert_eq!(out[1].line, Some(3));
    }

    #[test]
    fn annotate_uses_the_impl_owner_to_break_ties() {
        let dir = tree(&[(
            "src/lib.rs",
            "pub struct A;\npub struct B;\nimpl A {\n    pub fn run(&self) {}\n}\nimpl B {\n    pub fn run(&self, n: u32) {}\n}\n",
        )]);
        let diags = vec![error(
            "E0599",
            "no method named `run` found for struct `B` in the current scope",
        )];
        let out = annotate(dir.path(), diags);
        assert_eq!(out[1].message, "pub fn run(&self, n: u32)");
        assert_eq!(out[1].line, Some(7));
    }

    #[test]
    fn annotate_skips_ambiguous_and_missing_items() {
        let dir = tree(&[(
            "src/lib.rs",
            "pub fn run() {}\npub mod m {\n    pub fn run() {}\n}\n",
        )]);
        let ambiguous = vec![error("E0425", "cannot find function `run` in this scope")];
        assert_eq!(annotate(dir.path(), ambiguous).len(), 1);
        let missing = vec![error("E0425", "cannot find function `nope` in this scope")];
        assert_eq!(annotate(dir.path(), missing).len(), 1);
    }

    #[test]
    fn annotate_does_not_show_a_method_of_another_type() {
        let dir = tree(&[(
            "src/lib.rs",
            "pub struct A;\npub struct B;\nimpl A {\n    pub fn run(&self) {}\n}\n",
        )]);
        let diags = vec![error(
            "E0599",
            "no method named `run` found for struct `B` in the current scope",
        )];
        assert_eq!(annotate(dir.path(), diags).len(), 1);
    }

    #[test]
    fn annotate_finds_structs_for_field_errors() {
        let dir = tree(&[(
            "src/model.rs",
            "pub struct Task {\n    pub id: u32,\n    pub title: String,\n}\n",
        )]);
        let diags = vec![error("E0560", "struct `Task` has no field named `titel`")];
        let out = annotate(dir.path(), diags);
        assert_eq!(
            out[1].message,
            "pub struct Task { pub id: u32, pub title: String }"
        );
    }

    #[test]
    fn annotate_keeps_existing_companions_and_other_errors() {
        let dir = tree(&[("src/lib.rs", "pub fn run() {}\n")]);
        let first = error("E0425", "cannot find function `run` in this scope");
        let existing = companion(&first, "fn run()".into(), PathBuf::from("x.rs"), 1);
        let out = annotate(
            dir.path(),
            vec![first, existing, error("E0308", "mismatched types")],
        );
        assert_eq!(out.len(), 3);
    }
}
