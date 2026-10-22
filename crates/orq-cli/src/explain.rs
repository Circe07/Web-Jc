//! `orq explain <CODE>`: a trimmed `rustc --explain`.

use std::process::Command;

use anyhow::{bail, Context, Result};
use orq_core::estimate_tokens;

use crate::ExplainArgs;

/// Runs `orq explain` and returns the exit code.
pub fn run(args: &ExplainArgs) -> Result<u8> {
    let output = Command::new("rustc")
        .args(["--explain", &args.code])
        .output()
        .context("failed to run `rustc --explain`")?;
    if !output.status.success() {
        bail!(
            "rustc does not know `{}`: {}",
            args.code,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let text = summarize(&String::from_utf8_lossy(&output.stdout), args.budget);
    println!("{}", text.trim_end());
    Ok(0)
}

enum Block {
    Text(String),
    Code(String),
}

fn parse(markdown: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut current = String::new();
    let mut in_code = false;
    for line in markdown.lines() {
        if line.trim_start().starts_with("```") {
            flush(&mut blocks, &mut current, in_code);
            in_code = !in_code;
        } else if !in_code && line.trim().is_empty() {
            flush(&mut blocks, &mut current, false);
        } else {
            current.push_str(line);
            current.push('\n');
        }
    }
    flush(&mut blocks, &mut current, in_code);
    blocks
}

fn flush(blocks: &mut Vec<Block>, current: &mut String, was_code: bool) {
    let text = std::mem::take(current);
    if text.trim().is_empty() {
        return;
    }
    blocks.push(if was_code {
        Block::Code(text)
    } else {
        Block::Text(text)
    });
}

/// Keeps the leading paragraphs plus the first erroneous and corrected code
/// examples, dropping the rest, and trims the result to `budget` tokens.
fn summarize(markdown: &str, budget: usize) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut codes = 0;
    for block in parse(markdown) {
        match block {
            Block::Text(t) => {
                if codes >= 2 {
                    break;
                }
                parts.push(t.trim_end().to_string());
            }
            Block::Code(c) => {
                codes += 1;
                if codes > 2 {
                    break;
                }
                parts.push(c.trim_end().to_string());
            }
        }
    }
    let mut out = parts.join("\n\n");
    while estimate_tokens(&out) > budget && parts.len() > 1 {
        parts.pop();
        out = parts.join("\n\n");
    }
    truncate_to_budget(out, budget)
}

fn truncate_to_budget(text: String, budget: usize) -> String {
    if estimate_tokens(&text) <= budget {
        return text;
    }
    const MARK: &str = "...";
    let keep = (budget * 4).saturating_sub(MARK.len());
    let mut out: String = text.chars().take(keep).collect();
    out.push_str(MARK);
    out.chars().take(budget * 4).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "Intro text.\n\nErroneous code:\n\n```\nlet a = 1;\n```\n\nMiddle.\n\n```\nlet b = 2;\n```\n\nExtra paragraph.\n\n```\nlet c = 3;\n```\n";

    #[test]
    fn keeps_two_examples() {
        let out = summarize(SAMPLE, 1000);
        assert!(out.contains("let a") && out.contains("let b"));
        assert!(!out.contains("let c") && !out.contains("Extra"));
        assert!(!out.contains("```"));
    }

    #[test]
    fn respects_budget() {
        for budget in [1, 5, 10, 20] {
            assert!(estimate_tokens(&summarize(SAMPLE, budget)) <= budget);
        }
    }
}
