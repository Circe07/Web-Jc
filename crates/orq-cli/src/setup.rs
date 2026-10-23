//! `orq setup`: detect fast build tools and propose a dev-only profile
//! (PLAN.md T3.2).
//!
//! Design decisions:
//!
//! * Detection never installs anything; missing tools only produce the
//!   command the user can run by hand.
//! * The default proposal is safe on stable cargo and limited to the dev
//!   profile: `debug = "line-tables-only"` and `opt-level = 1` for
//!   dependencies.
//! * Cranelift is opt-in (`--cranelift`): stable cargo rejects `[unstable]`
//!   and the `codegen-backend` profile key, so writing it by default could
//!   break `cargo check` for a user on the stable toolchain.
//! * Linker (mold/wild, each a commented alternative) and sccache are not per-profile in cargo config, so
//!   they are only emitted as commented-out hints inside the managed block;
//!   release builds are never touched.
//! * The managed block (`# >>> orq setup >>>` ... `# <<< orq setup <<<`) is
//!   replaced in place, which makes re-running idempotent.
//! * TOML forbids duplicate tables. If the user's file (outside the block)
//!   already defines a table we need to extend, we do not merge text: we
//!   refuse `--apply` and list the keys to add by hand. Keys the user already
//!   set are never overridden.

use std::fmt::Write as _;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result};
use clap::Args;

/// First line of the managed block.
pub const BLOCK_START: &str = "# >>> orq setup >>>";
/// Last line of the managed block.
pub const BLOCK_END: &str = "# <<< orq setup <<<";

/// Arguments of `orq setup`.
#[derive(Args)]
pub struct SetupArgs {
    /// Write the proposed block to `.cargo/config.toml` (backing up the old file).
    #[arg(long)]
    pub apply: bool,
    /// Also propose the Cranelift backend (requires nightly + the component;
    /// the project must then be built with nightly).
    #[arg(long)]
    pub cranelift: bool,
    /// Workspace root (default: the workspace containing the current directory).
    #[arg(long)]
    pub root: Option<PathBuf>,
}

/// Which tools were found on this machine.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Detected {
    /// A `nightly` toolchain is installed.
    pub nightly: bool,
    /// The `rustc-codegen-cranelift` component is installed on nightly.
    pub cranelift: bool,
    /// `mold` is on `PATH`.
    pub mold: bool,
    /// `wild` is on `PATH`.
    pub wild: bool,
    /// `sccache` runs.
    pub sccache: bool,
    /// `cargo nextest` runs.
    pub nextest: bool,
}

/// Source of detection results (injectable for tests).
pub trait Probe {
    /// Detects the installed tools without installing anything.
    fn detect(&self) -> Detected;
}

/// Probe that inspects the real machine.
pub struct SystemProbe;

fn succeeds(program: &str, args: &[&str]) -> bool {
    Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn output_of(program: &str, args: &[&str]) -> String {
    Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

fn on_path(name: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| {
        let candidate = dir.join(name);
        std::fs::metadata(&candidate).is_ok_and(|m| {
            m.is_file() && {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    m.permissions().mode() & 0o111 != 0
                }
                #[cfg(not(unix))]
                {
                    true
                }
            }
        })
    })
}

impl Probe for SystemProbe {
    fn detect(&self) -> Detected {
        let nightly = output_of("rustup", &["toolchain", "list"])
            .lines()
            .any(|l| l.trim_start().starts_with("nightly"));
        let cranelift = nightly
            && output_of(
                "rustup",
                &["component", "list", "--toolchain", "nightly", "--installed"],
            )
            .contains("rustc-codegen-cranelift");
        Detected {
            nightly,
            cranelift,
            mold: on_path("mold"),
            wild: on_path("wild"),
            sccache: succeeds("sccache", &["--version"]),
            nextest: succeeds("cargo", &["nextest", "--version"]),
        }
    }
}

/// One `key = value` setting inside a TOML table.
struct Setting {
    table: &'static [&'static str],
    key: &'static str,
    value: &'static str,
    comment: &'static str,
}

fn settings(cranelift: bool) -> Vec<Setting> {
    let mut v = Vec::new();
    if cranelift {
        v.push(Setting {
            table: &["unstable"],
            key: "codegen-backend",
            value: "true",
            comment: "Requires building with nightly cargo.",
        });
    }
    v.push(Setting {
        table: &["profile", "dev"],
        key: "debug",
        value: "\"line-tables-only\"",
        comment: "Smaller debug info: faster links, backtraces still work.",
    });
    if cranelift {
        v.push(Setting {
            table: &["profile", "dev"],
            key: "codegen-backend",
            value: "\"cranelift\"",
            comment: "Requires nightly + rustc-codegen-cranelift-preview.",
        });
    }
    v.push(Setting {
        table: &["profile", "dev", "package", "*"],
        key: "opt-level",
        value: "1",
        comment: "Lightly optimise dependencies only (your crates stay at 0).",
    });
    v
}

fn header(table: &[&str]) -> String {
    let parts: Vec<String> = table
        .iter()
        .map(|p| {
            if p.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            {
                (*p).to_string()
            } else {
                format!("\"{p}\"")
            }
        })
        .collect();
    format!("[{}]", parts.join("."))
}

/// A proposed config edit.
#[derive(Debug, PartialEq, Eq)]
pub enum Proposal {
    /// The new full text of `.cargo/config.toml`.
    Ready {
        /// Final file contents.
        text: String,
        /// Human notes (keys skipped because the user already set them).
        notes: Vec<String>,
    },
    /// Applying would need edits inside tables the user owns.
    Refused {
        /// Lines (`[table]  key = value`) to add by hand.
        manual: Vec<String>,
    },
}

/// Removes the managed block, returning the remaining text.
fn strip_block(text: &str) -> Result<String> {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.iter().position(|l| l.trim() == BLOCK_START);
    let end = lines.iter().position(|l| l.trim() == BLOCK_END);
    match (start, end) {
        (None, None) => Ok(text.to_string()),
        (Some(s), Some(e)) if s < e => {
            let mut rest: Vec<&str> = lines[..s].to_vec();
            rest.extend_from_slice(&lines[e + 1..]);
            let mut out = rest.join("\n");
            if !out.is_empty() {
                out.push('\n');
            }
            Ok(out)
        }
        _ => anyhow::bail!("unbalanced `{BLOCK_START}` / `{BLOCK_END}` markers in config.toml"),
    }
}

fn lookup<'a>(root: &'a toml::Table, path: &[&str]) -> Option<&'a toml::Table> {
    let mut cur = root;
    for p in path {
        cur = cur.get(*p)?.as_table()?;
    }
    Some(cur)
}

/// Builds the new config text from the `existing` file contents.
pub fn propose(existing: &str, found: &Detected, cranelift: bool) -> Result<Proposal> {
    let base = strip_block(existing)?;
    let user: toml::Table = base
        .parse()
        .context("existing .cargo/config.toml is not valid TOML")?;

    let all = settings(cranelift);
    let mut tables: Vec<(&'static [&'static str], Vec<&Setting>)> = Vec::new();
    let mut notes = Vec::new();
    let mut manual = Vec::new();
    for s in &all {
        if let Some(t) = lookup(&user, s.table) {
            if t.contains_key(s.key) {
                notes.push(format!(
                    "{} {} is already set in your file; left untouched",
                    header(s.table),
                    s.key
                ));
                continue;
            }
            // A table that only holds sub-tables can still get its own
            // `[table]` header; anything else would be a duplicate.
            let occupied = t.is_empty() || t.values().any(|v| !v.is_table());
            if occupied {
                manual.push(format!("{}  {} = {}", header(s.table), s.key, s.value));
                continue;
            }
        }
        match tables.iter_mut().find(|(t, _)| *t == s.table) {
            Some((_, v)) => v.push(s),
            None => tables.push((s.table, vec![s])),
        }
    }
    if !manual.is_empty() {
        return Ok(Proposal::Refused { manual });
    }

    let mut block = String::new();
    let _ = writeln!(block, "{BLOCK_START}");
    let _ = writeln!(
        block,
        "# Managed by `orq setup`; re-running replaces only this block."
    );
    for (table, items) in &tables {
        let _ = writeln!(block, "{}", header(table));
        for s in items {
            let _ = writeln!(block, "# {}", s.comment);
            let _ = writeln!(block, "{} = {}", s.key, s.value);
        }
    }
    let _ = writeln!(block, "#");
    let _ = writeln!(
        block,
        "# Not enabled: linker and compiler wrapper are not per-profile in cargo"
    );
    let _ = writeln!(
        block,
        "# config and would also affect release builds. Uncomment deliberately."
    );
    // Each linker is its own alternative: enable one at most. mold is a
    // `-fuse-ld` name for clang; wild is selected by path (`--ld-path`).
    let linker_flags = [
        (found.mold, "mold", "-fuse-ld=mold"),
        (found.wild, "wild", "--ld-path=wild"),
    ];
    for (present, name, flag) in linker_flags {
        if present {
            let _ = writeln!(block, "# Linker alternative: {name}");
            let _ = writeln!(block, "# [target.x86_64-unknown-linux-gnu]");
            let _ = writeln!(block, "# linker = \"clang\"");
            let _ = writeln!(block, "# rustflags = [\"-C\", \"link-arg={flag}\"]");
        }
    }
    if found.sccache {
        let _ = writeln!(
            block,
            "# Compiler cache (or per shell: RUSTC_WRAPPER=sccache):"
        );
        let _ = writeln!(block, "# [build]");
        let _ = writeln!(block, "# rustc-wrapper = \"sccache\"");
    }
    let _ = writeln!(block, "{BLOCK_END}");

    let mut text = base.trim_end().to_string();
    if !text.is_empty() {
        text.push_str("\n\n");
    }
    text.push_str(&block);
    text.parse::<toml::Table>()
        .context("generated config.toml would not be valid TOML")?;
    Ok(Proposal::Ready { text, notes })
}

/// Simple line diff (LCS) with `-`/`+`/` ` prefixes; unchanged lines far from
/// a change are dropped.
pub fn diff(old: &str, new: &str) -> String {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    let mut lcs = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            lcs[i][j] = if a[i] == b[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let mut out: Vec<(char, &str)> = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        if a[i] == b[j] {
            out.push((' ', a[i]));
            i += 1;
            j += 1;
        } else if lcs[i + 1][j] >= lcs[i][j + 1] {
            out.push(('-', a[i]));
            i += 1;
        } else {
            out.push(('+', b[j]));
            j += 1;
        }
    }
    out.extend(a[i..].iter().map(|l| ('-', *l)));
    out.extend(b[j..].iter().map(|l| ('+', *l)));
    let keep: Vec<bool> = (0..out.len())
        .map(|k| {
            let lo = k.saturating_sub(2);
            let hi = (k + 3).min(out.len());
            out[lo..hi].iter().any(|(c, _)| *c != ' ')
        })
        .collect();
    let mut s = String::new();
    for (k, (c, l)) in out.iter().enumerate() {
        if keep[k] {
            let _ = writeln!(s, "{c}{l}");
        }
    }
    s
}

/// Copies `path` to the first free `<path>.orq-backup[.N]` and returns it.
pub fn backup(path: &Path) -> Result<PathBuf> {
    let base = path.as_os_str().to_string_lossy().into_owned();
    let mut target = PathBuf::from(format!("{base}.orq-backup"));
    let mut n = 1;
    while target.exists() {
        target = PathBuf::from(format!("{base}.orq-backup.{n}"));
        n += 1;
    }
    std::fs::copy(path, &target).with_context(|| format!("backing up {}", path.display()))?;
    Ok(target)
}

fn install_hints(found: &Detected, out: &mut dyn Write) -> Result<()> {
    let mut missing: Vec<&str> = Vec::new();
    if !found.nightly {
        missing.push("rustup toolchain install nightly   # nightly toolchain (for Cranelift)");
    }
    if !found.cranelift {
        missing.push(
            "rustup component add rustc-codegen-cranelift-preview --toolchain nightly   # Cranelift",
        );
    }
    if !found.mold && !found.wild {
        missing.push(
            "cargo install wild-linker --locked   # fast linker (or `mold` from your distro)",
        );
    }
    if !found.sccache {
        missing.push("cargo install sccache --locked   # compiler cache");
    }
    if !found.nextest {
        missing.push("cargo install cargo-nextest --locked   # faster test runner");
    }
    if !missing.is_empty() {
        writeln!(
            out,
            "\nMissing tools (orq never installs them; run by hand):"
        )?;
        for m in missing {
            writeln!(out, "  {m}")?;
        }
    }
    Ok(())
}

fn yn(b: bool) -> &'static str {
    if b {
        "found"
    } else {
        "missing"
    }
}

/// Runs `orq setup` with injected probe and writer; returns the exit code.
pub fn run_with(args: &SetupArgs, probe: &dyn Probe, out: &mut dyn Write) -> Result<u8> {
    let root = orq_core::root::resolve(args.root.as_deref())?;
    let found = probe.detect();
    writeln!(out, "Detected tools:")?;
    writeln!(out, "  nightly toolchain  {}", yn(found.nightly))?;
    writeln!(out, "  cranelift          {}", yn(found.cranelift))?;
    writeln!(out, "  mold               {}", yn(found.mold))?;
    writeln!(out, "  wild               {}", yn(found.wild))?;
    writeln!(out, "  sccache            {}", yn(found.sccache))?;
    writeln!(out, "  cargo-nextest      {}", yn(found.nextest))?;

    if args.cranelift && !(found.nightly && found.cranelift) {
        writeln!(
            out,
            "\nrefused: --cranelift needs the nightly toolchain and the cranelift component."
        )?;
        install_hints(&found, out)?;
        return Ok(2);
    }

    let path = root.join(".cargo").join("config.toml");
    let existing = match std::fs::read_to_string(&path) {
        Ok(t) => Some(t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    let old = existing.clone().unwrap_or_default();
    let (text, notes) = match propose(&old, &found, args.cranelift)? {
        Proposal::Ready { text, notes } => (text, notes),
        Proposal::Refused { manual } => {
            writeln!(
                out,
                "\nrefused: {} already defines tables orq would extend, and TOML forbids duplicate tables.",
                path.display()
            )?;
            writeln!(out, "Add these by hand:")?;
            for m in manual {
                writeln!(out, "  {m}")?;
            }
            return Ok(2);
        }
    };
    for n in &notes {
        writeln!(out, "note: {n}")?;
    }

    if text == old {
        writeln!(out, "\n{} is already up to date.", path.display())?;
        install_hints(&found, out)?;
        return Ok(0);
    }

    writeln!(out, "\n--- {0}\n+++ {0} (proposed)", path.display())?;
    write!(out, "{}", diff(&old, &text))?;

    if args.apply {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        if existing.is_some() {
            let b = backup(&path)?;
            writeln!(out, "\nBackup: {}", b.display())?;
        }
        std::fs::write(&path, &text).with_context(|| format!("writing {}", path.display()))?;
        writeln!(out, "Wrote {}", path.display())?;
    } else {
        writeln!(
            out,
            "\nDry run: nothing written. Re-run with --apply to write it."
        )?;
    }
    install_hints(&found, out)?;
    Ok(0)
}

/// Runs `orq setup` and returns the exit code.
pub fn run(args: &SetupArgs) -> Result<u8> {
    let mut stdout = std::io::stdout();
    run_with(args, &SystemProbe, &mut stdout)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn none() -> Detected {
        Detected::default()
    }

    fn ready(p: Proposal) -> String {
        match p {
            Proposal::Ready { text, .. } => text,
            Proposal::Refused { manual } => panic!("refused: {manual:?}"),
        }
    }

    #[test]
    fn default_proposal_is_dev_only_and_valid() {
        let t = ready(propose("", &none(), false).unwrap());
        assert!(t.contains("[profile.dev]"));
        assert!(t.contains("debug = \"line-tables-only\""));
        assert!(t.contains("[profile.dev.package.\"*\"]"));
        assert!(!t.contains("cranelift\""));
        assert!(!t.contains("[unstable]"));
        assert!(!t.contains("profile.release"));
        t.parse::<toml::Table>().unwrap();
    }

    #[test]
    fn linker_and_sccache_are_only_comments() {
        let found = Detected {
            mold: true,
            sccache: true,
            ..none()
        };
        let t = ready(propose("", &found, false).unwrap());
        assert!(t.contains("# rustflags = [\"-C\", \"link-arg=-fuse-ld=mold\"]"));
        assert!(t.contains("# rustc-wrapper = \"sccache\""));
        let doc: toml::Table = t.parse().unwrap();
        assert!(doc.get("target").is_none() && doc.get("build").is_none());
    }

    #[test]
    fn wild_hint_uses_ld_path_and_both_linkers_are_alternatives() {
        let found = Detected {
            wild: true,
            sccache: true,
            ..none()
        };
        let t = ready(propose("", &found, false).unwrap());
        assert!(t.contains("# rustflags = [\"-C\", \"link-arg=--ld-path=wild\"]"));
        assert!(!t.contains("fuse-ld=mold"));
        assert!(t.contains("RUSTC_WRAPPER=sccache"));
        let both = Detected {
            mold: true,
            wild: true,
            ..none()
        };
        let t = ready(propose("", &both, false).unwrap());
        assert!(t.contains("fuse-ld=mold") && t.contains("--ld-path=wild"));
        // still only comments: nothing outside the dev profile is enabled
        let doc: toml::Table = t.parse().unwrap();
        assert!(doc.get("target").is_none() && doc.get("build").is_none());
    }

    #[test]
    fn cranelift_is_opt_in() {
        let found = Detected {
            nightly: true,
            cranelift: true,
            ..none()
        };
        assert!(!ready(propose("", &found, false).unwrap()).contains("[unstable]"));
        let t = ready(propose("", &found, true).unwrap());
        let doc: toml::Table = t.parse().unwrap();
        assert_eq!(doc["unstable"]["codegen-backend"].as_bool(), Some(true));
        assert_eq!(
            doc["profile"]["dev"]["codegen-backend"].as_str(),
            Some("cranelift")
        );
    }

    #[test]
    fn existing_dev_table_is_refused() {
        let existing = "[profile.dev]\nopt-level = 0\n";
        match propose(existing, &none(), false).unwrap() {
            Proposal::Refused { manual } => {
                assert_eq!(manual.len(), 1);
                assert!(manual[0].contains("debug"));
            }
            other => panic!("expected refusal, got {other:?}"),
        }
    }

    #[test]
    fn user_keys_are_never_overridden() {
        let existing = "[profile.dev]\ndebug = false\n";
        let t = ready(propose(existing, &none(), false).unwrap());
        assert_eq!(t.matches("[profile.dev]").count(), 1);
        let doc: toml::Table = t.parse().unwrap();
        assert_eq!(doc["profile"]["dev"]["debug"].as_bool(), Some(false));
        assert_eq!(
            doc["profile"]["dev"]["package"]["*"]["opt-level"].as_integer(),
            Some(1)
        );
    }

    #[test]
    fn unrelated_user_content_is_kept() {
        let existing = "[net]\nretry = 3\n";
        let t = ready(propose(existing, &none(), false).unwrap());
        assert!(t.starts_with("[net]\nretry = 3\n"));
        t.parse::<toml::Table>().unwrap();
    }

    #[test]
    fn proposal_is_idempotent() {
        let first = ready(propose("[net]\nretry = 3\n", &none(), false).unwrap());
        let second = ready(propose(&first, &none(), false).unwrap());
        assert_eq!(first, second);
        assert_eq!(first.matches(BLOCK_START).count(), 1);
    }

    #[test]
    fn unbalanced_markers_error() {
        assert!(propose(&format!("{BLOCK_START}\n"), &none(), false).is_err());
    }

    #[test]
    fn invalid_existing_toml_errors() {
        assert!(propose("[[[", &none(), false).is_err());
    }

    #[test]
    fn diff_marks_additions() {
        let d = diff("a\nb\n", "a\nb\nc\n");
        assert!(d.contains("+c"));
        assert!(!d.contains("-b"));
    }

    #[test]
    fn backup_does_not_overwrite_previous_backups() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("config.toml");
        std::fs::write(&p, "one").unwrap();
        let b1 = backup(&p).unwrap();
        std::fs::write(&p, "two").unwrap();
        let b2 = backup(&p).unwrap();
        assert_ne!(b1, b2);
        assert_eq!(std::fs::read_to_string(b1).unwrap(), "one");
        assert_eq!(std::fs::read_to_string(b2).unwrap(), "two");
    }

    struct Fixed(Detected);
    impl Probe for Fixed {
        fn detect(&self) -> Detected {
            self.0
        }
    }

    fn args(root: &Path, apply: bool, cranelift: bool) -> SetupArgs {
        SetupArgs {
            apply,
            cranelift,
            root: Some(root.to_path_buf()),
        }
    }

    #[test]
    fn run_with_applies_backs_up_and_is_noop_second_time() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname=\"x\"\n").unwrap();
        std::fs::create_dir(dir.path().join(".cargo")).unwrap();
        let cfg = dir.path().join(".cargo/config.toml");
        std::fs::write(&cfg, "[net]\nretry = 3\n").unwrap();
        let mut out = Vec::new();
        assert_eq!(
            run_with(&args(dir.path(), true, false), &Fixed(none()), &mut out).unwrap(),
            0
        );
        assert!(dir.path().join(".cargo/config.toml.orq-backup").exists());
        let after = std::fs::read_to_string(&cfg).unwrap();
        assert!(after.contains(BLOCK_START));
        let mut out = Vec::new();
        run_with(&args(dir.path(), true, false), &Fixed(none()), &mut out).unwrap();
        assert!(String::from_utf8(out).unwrap().contains("up to date"));
        assert_eq!(std::fs::read_to_string(&cfg).unwrap(), after);
        assert!(!dir.path().join(".cargo/config.toml.orq-backup.1").exists());
    }

    #[test]
    fn run_with_refuses_cranelift_without_nightly_and_dev_conflicts() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname=\"x\"\n").unwrap();
        let mut out = Vec::new();
        assert_eq!(
            run_with(&args(dir.path(), true, true), &Fixed(none()), &mut out).unwrap(),
            2
        );
        assert!(!dir.path().join(".cargo/config.toml").exists());

        std::fs::create_dir(dir.path().join(".cargo")).unwrap();
        std::fs::write(dir.path().join(".cargo/config.toml"), "[profile.dev]\n").unwrap();
        let mut out = Vec::new();
        assert_eq!(
            run_with(&args(dir.path(), true, false), &Fixed(none()), &mut out).unwrap(),
            2
        );
        assert!(String::from_utf8(out)
            .unwrap()
            .contains("Add these by hand"));
    }
}
