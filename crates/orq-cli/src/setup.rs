//! `orq setup`: detect fast build tools and propose a dev profile
//! (PLAN.md T3.2).

use std::path::PathBuf;

use anyhow::{bail, Result};
use clap::Args;

/// Arguments of `orq setup`.
#[derive(Args)]
pub struct SetupArgs {
    /// Write the proposed block to `.cargo/config.toml` (backing up the old file).
    #[arg(long)]
    pub apply: bool,
    /// Workspace root (default: the workspace containing the current directory).
    #[arg(long)]
    pub root: Option<PathBuf>,
}

/// Runs `orq setup` and returns the exit code.
pub fn run(args: &SetupArgs) -> Result<u8> {
    let _ = args;
    bail!("`orq setup` is not implemented yet (PLAN.md T3.2)")
}
