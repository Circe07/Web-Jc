use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
use cli::{run, Command};
use store::Store;

/// A small task tracker.
#[derive(Parser)]
#[command(version)]
struct Args {
    /// Store file.
    #[arg(long, default_value = "tasks.json")]
    file: PathBuf,
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Add a task: `tasktrack add Buy milk +shopping !high due:2026-11-01`.
    Add { words: Vec<String> },
    /// List tasks matching a query.
    List {
        query: Vec<String>,
        #[arg(long, default_value = "id")]
        sort: String,
    },
    /// Mark a task as in progress.
    Start { id: u32 },
    /// Mark a task as done.
    Done { id: u32 },
    /// Remove a task.
    Rm { id: u32 },
    /// Remove every finished task.
    Purge,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let command = match args.command {
        Cmd::Add { words } => Command::Add {
            line: words.join(" "),
        },
        Cmd::List { query, sort } => Command::List {
            query: query.join(" "),
            sort,
        },
        Cmd::Start { id } => Command::Start { id },
        Cmd::Done { id } => Command::Done { id },
        Cmd::Rm { id } => Command::Remove { id },
        Cmd::Purge => Command::Purge,
    };
    let mut store = Store::load(&args.file)?;
    let today = chrono::Local::now().date_naive();
    println!("{}", run(&mut store, &command, today)?);
    store.save(&args.file)?;
    Ok(())
}
