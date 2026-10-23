//! `orq doctor`: diagnose the installation (PLAN.md T5.5).
//!
//! Every check is a plain function of a [`Probe`] (the outside world: running
//! programs, asking the daemon), so tests inject fakes. The checks never
//! create anything in the workspace or in the socket directory.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::Result;
use clap::Args;
use orq_core::ipc::{self, DaemonStatus};
use orq_core::{root, Config, NextestMode, CONFIG_FILE_NAME};
use serde_json::{json, Value};

/// Arguments of `orq doctor`.
#[derive(Args)]
pub struct DoctorArgs {
    /// Workspace root (default: the workspace containing the current directory).
    #[arg(long)]
    pub root: Option<PathBuf>,
    /// Print JSON instead of text.
    #[arg(long)]
    pub json: bool,
}

/// Severity of one check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// Everything fine.
    Ok,
    /// Works, but something is suboptimal.
    Warn,
    /// Broken: `orq verify` will not work properly.
    Fail,
    /// Informational line (hints); never affects the exit code.
    Info,
}

impl Level {
    fn label(self) -> &'static str {
        match self {
            Level::Ok => "ok",
            Level::Warn => "warn",
            Level::Fail => "fail",
            Level::Info => "info",
        }
    }
}

/// One line of the report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    /// Short name (`toolchain`, `cache`...).
    pub name: String,
    /// Result.
    pub level: Level,
    /// Human-readable detail.
    pub detail: String,
}

impl Check {
    fn new(name: &str, level: Level, detail: impl Into<String>) -> Self {
        Self {
            name: name.to_string(),
            level,
            detail: detail.into(),
        }
    }
}

/// The outside world, injectable for tests.
pub trait Probe {
    /// Runs `program args...` and returns its trimmed stdout, or `None` if it
    /// is missing or exits non-zero.
    fn output(&self, program: &str, args: &[&str], dir: Option<&Path>) -> Option<String>;
    /// Status of the daemon serving `root`, if one answers.
    fn daemon_status(&self, root: &Path) -> Option<DaemonStatus>;
}

/// Probe backed by real processes and the real daemon socket.
pub struct RealProbe;

impl Probe for RealProbe {
    fn output(&self, program: &str, args: &[&str], dir: Option<&Path>) -> Option<String> {
        let mut cmd = Command::new(program);
        cmd.args(args).stdin(Stdio::null()).stderr(Stdio::null());
        if let Some(dir) = dir {
            cmd.current_dir(dir);
        }
        let out = cmd.output().ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    fn daemon_status(&self, root: &Path) -> Option<DaemonStatus> {
        orq_daemon::client::daemon_status(root)
    }
}

/// Runs every check against `root` (`Err` = the root could not be resolved).
pub fn collect(root: Result<PathBuf, String>, probe: &dyn Probe) -> (Vec<Check>, Option<Value>) {
    let mut checks = toolchain_checks(probe);
    checks.push(git_binary_check(probe));
    let root = match root {
        Ok(root) => {
            checks.push(Check::new(
                "workspace",
                Level::Ok,
                format!("root {}", root.display()),
            ));
            root
        }
        Err(e) => {
            checks.push(Check::new(
                "workspace",
                Level::Fail,
                format!("no Cargo workspace found ({e}); pass --root <dir>"),
            ));
            return (checks, None);
        }
    };
    checks.push(git_repo_check(&root, probe));
    let (config_check, config) = config_check(&root);
    checks.push(config_check);
    if let Some(config) = &config {
        checks.push(nextest_check(config, probe));
    } else {
        checks.push(nextest_check(&Config::default(), probe));
    }
    checks.push(target_dir_check(&root));
    let daemon = probe.daemon_status(&root);
    checks.push(cache_check(&root, daemon.is_some()));
    checks.extend(daemon_checks(&root, daemon.as_ref()));
    checks.push(mcp_check(&root));
    let effective = config.and_then(|c| serde_json::to_value(c).ok());
    (checks, effective)
}

fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or("")
}

fn toolchain_checks(probe: &dyn Probe) -> Vec<Check> {
    let mut out = Vec::new();
    for (name, program) in [("rustc", "rustc"), ("cargo", "cargo")] {
        out.push(match probe.output(program, &["-V"], None) {
            Some(v) => Check::new(name, Level::Ok, first_line(&v)),
            None => Check::new(
                name,
                Level::Fail,
                format!("`{program} -V` failed; install Rust from https://rustup.rs"),
            ),
        });
    }
    for (name, args, hint) in [
        (
            "rustfmt",
            ["fmt", "--version"],
            "rustup component add rustfmt",
        ),
        (
            "clippy",
            ["clippy", "--version"],
            "rustup component add clippy",
        ),
    ] {
        out.push(match probe.output("cargo", &args, None) {
            Some(v) => Check::new(name, Level::Ok, first_line(&v)),
            None => Check::new(
                name,
                Level::Fail,
                format!("not installed (`orq verify` runs it); run `{hint}`"),
            ),
        });
    }
    out
}

fn nextest_check(config: &Config, probe: &dyn Probe) -> Check {
    match probe.output("cargo", &["nextest", "--version"], None) {
        Some(v) => Check::new("nextest", Level::Ok, first_line(&v)),
        None if config.nextest == NextestMode::Always => Check::new(
            "nextest",
            Level::Fail,
            "orq.toml sets nextest = \"always\" but cargo-nextest is missing; run `cargo install cargo-nextest --locked`",
        ),
        None => Check::new(
            "nextest",
            Level::Warn,
            "cargo-nextest not installed; tests run with `cargo test` (optional: `cargo install cargo-nextest --locked`)",
        ),
    }
}

fn git_binary_check(probe: &dyn Probe) -> Check {
    match probe.output("git", &["--version"], None) {
        Some(v) => Check::new("git", Level::Ok, first_line(&v)),
        None => Check::new(
            "git",
            Level::Fail,
            "git not found on PATH; orq needs it to detect changed files",
        ),
    }
}

fn git_repo_check(root: &Path, probe: &dyn Probe) -> Check {
    match probe.output("git", &["rev-parse", "--is-inside-work-tree"], Some(root)) {
        Some(v) if v == "true" => {
            Check::new("git repo", Level::Ok, "root is inside a git work tree")
        }
        _ => Check::new(
            "git repo",
            Level::Fail,
            "root is not a git repository; `orq verify` needs `git diff` (run `git init`)",
        ),
    }
}

fn config_check(root: &Path) -> (Check, Option<Config>) {
    let path = root.join(CONFIG_FILE_NAME);
    let exists = path.is_file();
    match Config::load(root) {
        Ok(config) => {
            let shown = serde_json::to_string(&config).unwrap_or_default();
            let origin = if exists {
                CONFIG_FILE_NAME
            } else {
                "no orq.toml, defaults"
            };
            (
                Check::new("config", Level::Ok, format!("{origin}: {shown}")),
                Some(config),
            )
        }
        Err(e) => (
            Check::new(
                "config",
                Level::Fail,
                format!("{CONFIG_FILE_NAME} does not parse: {e}"),
            ),
            None,
        ),
    }
}

fn target_dir_check(root: &Path) -> Check {
    let target = root::target_dir(root);
    // Probe in the nearest existing ancestor: create nothing in a missing dir.
    let Some(existing) = target.ancestors().find(|p| p.is_dir()) else {
        return Check::new(
            "target dir",
            Level::Fail,
            format!("{} has no existing parent", target.display()),
        );
    };
    let probe = existing.join(format!(".orq-doctor-{}", std::process::id()));
    let writable = std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(&probe);
    if !writable {
        return Check::new(
            "target dir",
            Level::Fail,
            format!(
                "{} is not writable (checked {})",
                target.display(),
                existing.display()
            ),
        );
    }
    let state = if target.is_dir() {
        "writable"
    } else {
        "writable (will be created)"
    };
    Check::new(
        "target dir",
        Level::Ok,
        format!("{} {state}", target.display()),
    )
}

fn cache_check(root: &Path, daemon_running: bool) -> Check {
    let db = root::target_dir(root).join("orq").join("cache.redb");
    if !db.exists() {
        return Check::new(
            "cache",
            Level::Ok,
            format!(
                "{} not created yet (first `orq verify` creates it)",
                db.display()
            ),
        );
    }
    match orq_cache::RedbCache::open_at(&db).and_then(|c| c.len()) {
        Ok(n) => Check::new(
            "cache",
            Level::Ok,
            format!("{} opens, {n} entries", db.display()),
        ),
        // redb holds an exclusive lock: a running daemon explains the failure.
        Err(_) if daemon_running => Check::new(
            "cache",
            Level::Ok,
            format!("{} is held open by the running daemon", db.display()),
        ),
        Err(e) => Check::new(
            "cache",
            Level::Fail,
            format!(
                "{} does not open: {e}; delete it, orq recreates it",
                db.display()
            ),
        ),
    }
}

fn daemon_checks(root: &Path, status: Option<&DaemonStatus>) -> Vec<Check> {
    let socket = ipc::socket_path(root);
    let mut out = vec![Check::new(
        "socket",
        Level::Ok,
        socket.display().to_string(),
    )];
    out.push(socket_dir_check(&socket));
    out.push(match status {
        Some(s) => Check::new(
            "daemon",
            Level::Ok,
            format!(
                "running, pid {}, up {} s, {} requests{}",
                s.pid,
                s.uptime_ms / 1000,
                s.requests,
                if s.warming { ", warming up" } else { "" }
            ),
        ),
        None => Check::new(
            "daemon",
            Level::Info,
            "not running (optional; started on demand by `orq mcp`, or `orq daemon start`)",
        ),
    });
    out
}

/// Read-only version of `ipc::prepare_socket_dir`: same rules, creates nothing.
#[cfg(unix)]
fn socket_dir_check(socket: &Path) -> Check {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let Some(dir) = socket.parent() else {
        return Check::new("socket dir", Level::Ok, "no parent directory");
    };
    let in_runtime_dir = std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|d| !d.is_empty())
        .is_some_and(|d| dir == Path::new(&d));
    if in_runtime_dir {
        return Check::new(
            "socket dir",
            Level::Ok,
            format!("{} (XDG_RUNTIME_DIR, private by definition)", dir.display()),
        );
    }
    let bad = |why: &str| {
        Check::new(
            "socket dir",
            Level::Fail,
            format!(
                "{}: {why}; the daemon will refuse to start here",
                dir.display()
            ),
        )
    };
    let meta = match std::fs::symlink_metadata(dir) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Check::new(
                "socket dir",
                Level::Ok,
                format!("{} will be created with mode 0700", dir.display()),
            )
        }
        Err(e) => return bad(&e.to_string()),
    };
    if !meta.file_type().is_dir() {
        return bad("not a directory (symlink?)");
    }
    if meta.permissions().mode() & 0o077 != 0 {
        return bad("accessible by other users (expected mode 0700)");
    }
    // Owner check without libc: a file we create is owned by our uid.
    let owner_ok = tempfile_uid().is_none_or(|uid| uid == meta.uid());
    if !owner_ok {
        return bad("owned by another user");
    }
    Check::new(
        "socket dir",
        Level::Ok,
        format!("{} is private (mode 0700, ours)", dir.display()),
    )
}

/// uid of a file created in the temp dir (and removed again).
#[cfg(unix)]
fn tempfile_uid() -> Option<u32> {
    use std::os::unix::fs::MetadataExt;
    let probe = std::env::temp_dir().join(format!(".orq-doctor-uid-{}", std::process::id()));
    std::fs::write(&probe, b"").ok()?;
    let uid = std::fs::metadata(&probe).map(|m| m.uid()).ok();
    let _ = std::fs::remove_file(&probe);
    uid
}

#[cfg(not(unix))]
fn socket_dir_check(_socket: &Path) -> Check {
    Check::new("socket dir", Level::Ok, "no Unix sockets on this platform")
}

fn mcp_check(root: &Path) -> Check {
    let registered = [
        "/.mcp.json",
        "/.claude/settings.json",
        "/.claude/settings.local.json",
    ]
    .iter()
    .any(|f| {
        std::fs::read_to_string(format!("{}{f}", root.display())).is_ok_and(|t| t.contains("orq"))
    });
    if registered {
        Check::new(
            "mcp",
            Level::Ok,
            "orq is referenced in the project's MCP/Claude settings",
        )
    } else {
        Check::new(
            "mcp",
            Level::Info,
            format!(
                "not registered in this project; run `claude mcp add orq -- orq mcp --root {}` (or add it to .mcp.json)",
                root.display()
            ),
        )
    }
}

/// `true` when no check failed.
pub fn all_ok(checks: &[Check]) -> bool {
    checks.iter().all(|c| c.level != Level::Fail)
}

/// Text form: one `[level] name: detail` line per check.
pub fn render(checks: &[Check]) -> String {
    let mut out = String::new();
    for c in checks {
        out.push_str(&format!(
            "[{:<4}] {:<10} {}\n",
            c.level.label(),
            c.name,
            c.detail
        ));
    }
    let fails = checks.iter().filter(|c| c.level == Level::Fail).count();
    let warns = checks.iter().filter(|c| c.level == Level::Warn).count();
    out.push_str(&format!("{fails} fail, {warns} warn\n"));
    out
}

/// JSON form printed by `orq doctor --json`.
pub fn to_json(checks: &[Check], config: Option<Value>) -> Value {
    json!({
        "ok": all_ok(checks),
        "checks": checks.iter().map(|c| json!({
            "name": c.name,
            "level": c.level.label(),
            "detail": c.detail,
        })).collect::<Vec<_>>(),
        "config": config,
    })
}

/// Runs `orq doctor` and returns the exit code (0 if no check failed).
pub fn run(args: &DoctorArgs) -> Result<u8> {
    let root = root::resolve(args.root.as_deref()).map_err(|e| e.to_string());
    let (checks, config) = collect(root, &RealProbe);
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&to_json(&checks, config))?
        );
    } else {
        print!("{}", render(&checks));
    }
    Ok(u8::from(!all_ok(&checks)))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake {
        missing: Vec<&'static str>,
        daemon: Option<DaemonStatus>,
        in_git: bool,
    }

    impl Probe for Fake {
        fn output(&self, program: &str, args: &[&str], _dir: Option<&Path>) -> Option<String> {
            let key = args.first().copied().unwrap_or("");
            if self.missing.contains(&program) || self.missing.contains(&key) {
                return None;
            }
            if program == "git" && key == "rev-parse" {
                return self.in_git.then(|| "true".to_string());
            }
            Some(format!("{program} fake 1.0"))
        }
        fn daemon_status(&self, _root: &Path) -> Option<DaemonStatus> {
            self.daemon.clone()
        }
    }

    fn fake() -> Fake {
        Fake {
            missing: vec![],
            daemon: None,
            in_git: true,
        }
    }

    fn find<'a>(checks: &'a [Check], name: &str) -> &'a Check {
        checks.iter().find(|c| c.name == name).unwrap()
    }

    fn workspace() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[workspace]\nmembers = []\n").unwrap();
        dir
    }

    #[test]
    fn healthy_environment_has_no_failures() {
        let dir = workspace();
        let (checks, config) = collect(Ok(dir.path().to_path_buf()), &fake());
        assert!(all_ok(&checks), "{}", render(&checks));
        assert!(config.is_some());
        assert_eq!(find(&checks, "rustfmt").level, Level::Ok);
        assert_eq!(find(&checks, "daemon").level, Level::Info);
    }

    #[test]
    fn missing_clippy_and_git_fail() {
        let dir = workspace();
        let probe = Fake {
            missing: vec!["clippy", "git"],
            ..fake()
        };
        let (checks, _) = collect(Ok(dir.path().to_path_buf()), &probe);
        assert!(!all_ok(&checks));
        assert_eq!(find(&checks, "clippy").level, Level::Fail);
        assert_eq!(find(&checks, "git").level, Level::Fail);
        assert_eq!(find(&checks, "rustfmt").level, Level::Ok);
    }

    #[test]
    fn missing_nextest_warns_unless_always() {
        let dir = workspace();
        let probe = Fake {
            missing: vec!["nextest"],
            ..fake()
        };
        let (checks, _) = collect(Ok(dir.path().to_path_buf()), &probe);
        assert_eq!(find(&checks, "nextest").level, Level::Warn);
        assert!(all_ok(&checks));
        std::fs::write(dir.path().join("orq.toml"), "nextest = \"always\"\n").unwrap();
        let (checks, _) = collect(Ok(dir.path().to_path_buf()), &probe);
        assert_eq!(find(&checks, "nextest").level, Level::Fail);
    }

    #[test]
    fn not_a_git_repo_and_bad_config_fail() {
        let dir = workspace();
        std::fs::write(dir.path().join("orq.toml"), "budget = \"many\"\n").unwrap();
        let probe = Fake {
            in_git: false,
            ..fake()
        };
        let (checks, config) = collect(Ok(dir.path().to_path_buf()), &probe);
        assert_eq!(find(&checks, "git repo").level, Level::Fail);
        assert_eq!(find(&checks, "config").level, Level::Fail);
        assert!(config.is_none());
    }

    #[test]
    fn unresolved_root_fails_early() {
        let (checks, config) = collect(Err("none".into()), &fake());
        assert_eq!(find(&checks, "workspace").level, Level::Fail);
        assert!(config.is_none());
        assert!(checks.iter().all(|c| c.name != "cache"));
    }

    #[test]
    fn running_daemon_is_reported() {
        let dir = workspace();
        let status = DaemonStatus {
            root: dir.path().to_path_buf(),
            pid: 4242,
            uptime_ms: 5000,
            requests: 3,
            warming: false,
        };
        let probe = Fake {
            daemon: Some(status),
            ..fake()
        };
        let (checks, _) = collect(Ok(dir.path().to_path_buf()), &probe);
        let d = find(&checks, "daemon");
        assert_eq!(d.level, Level::Ok);
        assert!(d.detail.contains("pid 4242"));
    }

    #[test]
    fn cache_counts_entries_and_checks_do_not_create_files() {
        let dir = workspace();
        let (checks, _) = collect(Ok(dir.path().to_path_buf()), &fake());
        assert!(find(&checks, "cache").detail.contains("not created yet"));
        assert!(!dir.path().join("target").exists());
        let db = dir.path().join("target/orq/cache.redb");
        drop(orq_cache::RedbCache::open_at(&db).unwrap());
        let (checks, _) = collect(Ok(dir.path().to_path_buf()), &fake());
        assert!(find(&checks, "cache").detail.contains("0 entries"));
    }

    #[test]
    fn json_and_text_render() {
        let checks = vec![
            Check::new("a", Level::Ok, "fine"),
            Check::new("b", Level::Fail, "broken"),
        ];
        let v = to_json(&checks, None);
        assert_eq!(v["ok"], false);
        assert_eq!(v["checks"][1]["level"], "fail");
        let text = render(&checks);
        assert!(text.contains("[ok  ] a"));
        assert!(text.contains("1 fail, 0 warn"));
    }
}
