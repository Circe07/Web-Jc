//! Capturing raw cargo output the way an agent would see it.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::EvalError;

/// Runs `program args` in `dir` with stdout and stderr merged into one
/// stream, as a terminal shows them. Returns whether it succeeded and the
/// combined text.
pub fn run_combined(dir: &Path, program: &str, args: &[&str]) -> Result<(bool, String), EvalError> {
    let run_err = |e: std::io::Error| EvalError::Run(format!("{program} {}: {e}", args.join(" ")));
    let (mut reader, writer) = std::io::pipe().map_err(run_err)?;
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(dir)
        .env("CARGO_TERM_COLOR", "never")
        .stdin(Stdio::null())
        .stdout(writer.try_clone().map_err(run_err)?)
        .stderr(writer);
    let mut child = command.spawn().map_err(run_err)?;
    // The command holds the write ends; drop it or the read below never ends.
    drop(command);
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes).map_err(run_err)?;
    let status = child.wait().map_err(run_err)?;
    Ok((
        status.success(),
        String::from_utf8_lossy(&bytes).into_owned(),
    ))
}

/// The cargo command an agent would run to reproduce a failure at `step`
/// (`fmt`, `check`, `clippy` or `test`).
pub fn step_command(step: &str) -> Result<Vec<&'static str>, EvalError> {
    Ok(match step {
        "fmt" => vec!["fmt", "--check"],
        "check" => vec!["check", "--all-targets"],
        "clippy" => vec!["clippy", "--all-targets", "--", "-D", "warnings"],
        "test" => vec!["test"],
        other => return Err(EvalError::Run(format!("unknown step `{other}`"))),
    })
}

/// Raw output of the command for `step`.
pub fn raw_step(dir: &Path, step: &str) -> Result<String, EvalError> {
    let args = step_command(step)?;
    Ok(run_combined(dir, "cargo", &args)?.1)
}

/// Raw output of the full ladder `cargo build && cargo test`.
pub fn raw_ladder(dir: &Path) -> Result<String, EvalError> {
    let (ok, mut text) = run_combined(dir, "cargo", &["build"])?;
    if ok {
        text.push_str(&run_combined(dir, "cargo", &["test"])?.1);
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_commands() {
        assert_eq!(step_command("check").unwrap(), ["check", "--all-targets"]);
        assert_eq!(step_command("test").unwrap(), ["test"]);
        assert!(step_command("build").is_err());
    }

    #[test]
    fn combines_both_streams() {
        let dir = tempfile::tempdir().unwrap();
        let (ok, text) =
            run_combined(dir.path(), "sh", &["-c", "echo out; echo err >&2; exit 3"]).unwrap();
        assert!(!ok);
        assert!(text.contains("out") && text.contains("err"));
    }
}
