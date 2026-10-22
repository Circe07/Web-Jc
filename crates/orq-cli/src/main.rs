//! `orq` binary. Commands are added in wave 2 (PLAN.md T2.1).

use clap::Parser;

/// Rust build orchestrator for AI agents.
#[derive(Parser)]
#[command(name = "orq", version, about)]
struct Cli {}

fn main() -> anyhow::Result<()> {
    let _cli = Cli::parse();
    Ok(())
}
