//! `orq stats`: summary of `target/orq/stats.jsonl` (PLAN.md T3.3).
//! Records are written by `orq verify` and the MCP server through
//! `orq_core::stats`.

use std::path::PathBuf;

use anyhow::{bail, Result};
use clap::Args;

/// Arguments of `orq stats`.
#[derive(Args)]
pub struct StatsArgs {
    /// Print JSON instead of text.
    #[arg(long)]
    pub json: bool,
    /// Workspace root (default: the workspace containing the current directory).
    #[arg(long)]
    pub root: Option<PathBuf>,
}

/// Runs `orq stats` and returns the exit code.
pub fn run(args: &StatsArgs) -> Result<u8> {
    let _ = args;
    bail!("`orq stats` is not implemented yet (PLAN.md T3.3)")
}
