//! `orq mcp`: MCP server on stdio (implemented in `orq-mcp`, PLAN.md T3.1).

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Args;

/// Arguments of `orq mcp`.
#[derive(Args)]
pub struct McpArgs {
    /// Workspace root (default: the workspace containing the current directory).
    #[arg(long)]
    pub root: Option<PathBuf>,
}

/// Runs the MCP server until stdin closes. Stdout is the JSON-RPC channel:
/// nothing else may be printed there.
pub fn run(args: &McpArgs) -> Result<u8> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("building tokio runtime")?;
    runtime.block_on(orq_mcp::serve_stdio(args.root.clone()))?;
    Ok(0)
}
