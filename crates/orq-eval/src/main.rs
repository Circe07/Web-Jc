//! `orq-eval`: offline evaluation of orq's compact report.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use orq_eval::report::{CaseError, Report};
use orq_eval::run::{self, CaseTexts};
use orq_eval::EvalError;

#[derive(Parser)]
#[command(name = "orq-eval", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Measure raw cargo output and orq's report on every case.
    Run {
        /// Directory with the cases.
        #[arg(long, default_value = "eval/cases")]
        cases: PathBuf,
        /// Token budgets to compress at, comma separated.
        #[arg(long, value_delimiter = ',', default_values_t = [200usize, 500, 1500])]
        budgets: Vec<usize>,
        /// Where to write offline.json and offline.md.
        #[arg(long, default_value = "eval/results")]
        out: PathBuf,
        /// Run only these case ids (repeatable).
        #[arg(long = "case")]
        only: Vec<String>,
        /// Exit with an error if orq's mean recall at the largest budget is
        /// below this value (or a case failed to run).
        #[arg(long)]
        min_recall: Option<f64>,
    },
}

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();
    let Command::Run {
        cases,
        budgets,
        out,
        only,
        min_recall,
    } = Cli::parse().command;
    match run_all(&cases, budgets, &out, &only, min_recall) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("orq-eval: {e}");
            ExitCode::from(2)
        }
    }
}

fn run_all(
    cases_dir: &Path,
    budgets: Vec<usize>,
    out: &Path,
    only: &[String],
    min_recall: Option<f64>,
) -> Result<ExitCode, EvalError> {
    let ids = run::list_cases(cases_dir, only)?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| EvalError::Run(format!("starting the runtime: {e}")))?;
    let mut results = Vec::new();
    let mut errors = Vec::new();
    let mut texts: Vec<(String, CaseTexts)> = Vec::new();
    for id in &ids {
        eprintln!("orq-eval: running {id}");
        match runtime.block_on(run::run_case(&cases_dir.join(id), &budgets)) {
            Ok((result, case_texts)) => {
                results.push(result);
                texts.push((id.clone(), case_texts));
            }
            Err(e) => {
                eprintln!("orq-eval: case {id} failed: {e}");
                errors.push(CaseError {
                    id: id.clone(),
                    error: e.to_string(),
                });
            }
        }
    }
    let report = Report::new(budgets, results, errors);

    std::fs::create_dir_all(out).map_err(|e| EvalError::io(out, e))?;
    let json = serde_json::to_string_pretty(&report)
        .map_err(|e| EvalError::Run(format!("serializing results: {e}")))?;
    write(&out.join("offline.json"), &json)?;
    write(&out.join("offline.md"), &report.to_markdown())?;
    for (id, case_texts) in &texts {
        let dir = out.join("reports").join(id);
        std::fs::create_dir_all(&dir).map_err(|e| EvalError::io(&dir, e))?;
        for (mode, text) in &case_texts.texts {
            write(&dir.join(format!("{mode}.txt")), text)?;
        }
    }
    eprintln!("orq-eval: wrote {}", out.join("offline.md").display());

    let Some(min) = min_recall else {
        return Ok(ExitCode::SUCCESS);
    };
    if !report.errors.is_empty() {
        eprintln!("orq-eval: {} case(s) failed to run", report.errors.len());
        return Ok(ExitCode::FAILURE);
    }
    match report.orq_mean_recall_at_largest_budget() {
        Some(recall) if recall >= min => Ok(ExitCode::SUCCESS),
        Some(recall) => {
            eprintln!("orq-eval: mean recall {recall:.3} is below --min-recall {min}");
            Ok(ExitCode::FAILURE)
        }
        None => {
            eprintln!("orq-eval: no cases measured, cannot check --min-recall");
            Ok(ExitCode::FAILURE)
        }
    }
}

fn write(path: &Path, content: &str) -> Result<(), EvalError> {
    std::fs::write(path, content).map_err(|e| EvalError::io(path, e))
}
