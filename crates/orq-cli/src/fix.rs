//! `orq fix`: apply machine-applicable fixes, then verify (PLAN.md T5.3).

use std::path::PathBuf;

use anyhow::{bail, Result};
use clap::Args;

/// Arguments of `orq fix`.
#[derive(Args)]
pub struct FixArgs {
    /// Workspace root (default: the workspace containing the current directory).
    #[arg(long)]
    pub root: Option<PathBuf>,
}

/// Runs `orq fix` and returns the exit code.
pub fn run(args: &FixArgs) -> Result<u8> {
    let _ = args;
    bail!("`orq fix` is not implemented yet")
}
