//! `orq explain <CODE>`: a trimmed `rustc --explain`.

use anyhow::Result;

use crate::ExplainArgs;

/// Runs `orq explain` and returns the exit code.
pub fn run(args: &ExplainArgs) -> Result<u8> {
    let text = orq_runner::explain::explain(&args.code, args.budget)?;
    println!("{}", text.trim_end());
    Ok(0)
}
