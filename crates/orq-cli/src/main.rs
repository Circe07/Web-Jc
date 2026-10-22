//! `orq` binary: verify, explain, daemon management, MCP server, setup and
//! stats commands.
//!
//! Stdout carries only the command output (report, explanation, status);
//! logs go to stderr through `tracing` filtered by `ORQ_LOG` (default `warn`).

mod daemon_cmd;
mod explain;
mod mcp;
mod setup;
mod stats;
mod verify;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};

/// Rust build orchestrator for AI agents.
#[derive(Parser)]
#[command(name = "orq", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Verify the workspace: impact, cache, fmt/check/clippy/test ladder.
    Verify(VerifyArgs),
    /// Explain a rustc error code in a few tokens.
    Explain(ExplainArgs),
    /// Manage the background daemon.
    Daemon {
        #[command(subcommand)]
        action: DaemonCommand,
    },
    /// Run the MCP server on stdio (for Claude Code: `claude mcp add orq -- orq mcp`).
    Mcp(mcp::McpArgs),
    /// Detect fast build tools and propose (or `--apply`) a dev profile.
    Setup(setup::SetupArgs),
    /// Summarize recorded verifications (`target/orq/stats.jsonl`).
    Stats(stats::StatsArgs),
}

/// Arguments of `orq verify`.
#[derive(Args)]
pub struct VerifyArgs {
    /// Changed files (relative to the current directory); default: detect via git.
    #[arg(long, value_delimiter = ',')]
    pub files: Vec<PathBuf>,
    /// Last ladder step to run (fmt, check, clippy, test).
    #[arg(long)]
    pub up_to: Option<orq_core::Step>,
    /// Token budget of the report.
    #[arg(long)]
    pub budget: Option<usize>,
    /// Ignore the result cache.
    #[arg(long)]
    pub no_cache: bool,
    /// Print the JSON report instead of text.
    #[arg(long)]
    pub json: bool,
    /// Workspace root (default: the workspace containing the current directory).
    #[arg(long)]
    pub root: Option<PathBuf>,
    /// Never delegate to a daemon (also `ORQ_NO_DAEMON=1`).
    #[arg(long, hide = true)]
    pub no_daemon: bool,
}

/// Arguments of `orq explain`.
#[derive(Args)]
pub struct ExplainArgs {
    /// Error code, e.g. `E0308`.
    pub code: String,
    /// Token budget of the explanation.
    #[arg(long, default_value_t = 400)]
    pub budget: usize,
}

/// Subcommands of `orq daemon`.
#[derive(Subcommand)]
pub enum DaemonCommand {
    /// Start the daemon in the background.
    Start {
        /// Workspace root.
        #[arg(long)]
        root: Option<PathBuf>,
    },
    /// Ask the daemon to stop.
    Stop {
        /// Workspace root.
        #[arg(long)]
        root: Option<PathBuf>,
    },
    /// Show daemon status (exit 1 if not running).
    Status {
        /// Workspace root.
        #[arg(long)]
        root: Option<PathBuf>,
    },
    /// Run the daemon in the foreground.
    #[command(hide = true)]
    Run {
        /// Workspace root.
        #[arg(long)]
        root: Option<PathBuf>,
    },
}

fn main() -> ExitCode {
    let filter = tracing_subscriber::EnvFilter::try_from_env("ORQ_LOG")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    let result = match cli.command {
        Command::Verify(args) => verify::run(&args),
        Command::Explain(args) => explain::run(&args),
        Command::Daemon { action } => daemon_cmd::run(action),
        Command::Mcp(args) => mcp::run(&args),
        Command::Setup(args) => setup::run(&args),
        Command::Stats(args) => stats::run(&args),
    };
    match result {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            eprintln!("orq: error: {e:#}");
            ExitCode::from(2)
        }
    }
}
