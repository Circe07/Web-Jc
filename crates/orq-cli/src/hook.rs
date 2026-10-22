//! `orq hook`: Claude Code hook entry point (PLAN.md T5.4).

use std::path::PathBuf;

use anyhow::{bail, Result};
use clap::Args;

/// Arguments of `orq hook`.
#[derive(Args)]
pub struct HookArgs {
    /// Workspace root (default: the workspace containing the current directory).
    #[arg(long)]
    pub root: Option<PathBuf>,
}

/// Runs `orq hook` and returns the exit code.
pub fn run(args: &HookArgs) -> Result<u8> {
    let _ = args;
    bail!("`orq hook` is not implemented yet")
}
