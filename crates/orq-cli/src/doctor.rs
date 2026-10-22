//! `orq doctor`: diagnose the installation (PLAN.md T5.5).

use std::path::PathBuf;

use anyhow::{bail, Result};
use clap::Args;

/// Arguments of `orq doctor`.
#[derive(Args)]
pub struct DoctorArgs {
    /// Workspace root (default: the workspace containing the current directory).
    #[arg(long)]
    pub root: Option<PathBuf>,
}

/// Runs `orq doctor` and returns the exit code.
pub fn run(args: &DoctorArgs) -> Result<u8> {
    let _ = args;
    bail!("`orq doctor` is not implemented yet")
}
