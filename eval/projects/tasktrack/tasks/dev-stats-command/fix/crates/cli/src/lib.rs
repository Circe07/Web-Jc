//! Command implementations of the `tasktrack` CLI, kept separate from
//! argument parsing so they can be tested without spawning a process.

pub mod render;
mod stats;

use anyhow::{bail, Context, Result};
use chrono::NaiveDate;
use model::{parse_quick_add, Status, TaskId};
use query::{sort_tasks, Query, SortKey};
use store::Store;

/// One CLI command, already parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Add { line: String },
    List { query: String, sort: String },
    Start { id: u32 },
    Done { id: u32 },
    Remove { id: u32 },
    Purge,
    Stats,
}

/// Runs a command against the store and returns the text to print.
pub fn run(store: &mut Store, command: &Command, today: NaiveDate) -> Result<String> {
    match command {
        Command::Add { line } => {
            let input = parse_quick_add(line).context("cannot parse the task")?;
            let id = store.add(input, today);
            Ok(format!("added {id}"))
        }
        Command::List { query, sort } => {
            let query = Query::parse(query)?;
            let key: SortKey = sort.parse()?;
            let mut found = query.filter(store.tasks(), today);
            sort_tasks(&mut found, key);
            Ok(render::table(&found, today))
        }
        Command::Start { id } => {
            set_status(store, *id, Status::InProgress)?;
            Ok(format!("started {}", TaskId(*id)))
        }
        Command::Done { id } => {
            set_status(store, *id, Status::Done)?;
            Ok(format!("done {}", TaskId(*id)))
        }
        Command::Remove { id } => {
            let task = store.remove(TaskId(*id))?;
            Ok(format!("removed {} {}", task.id, task.title))
        }
        Command::Purge => {
            let n = store.purge_done();
            Ok(format!("purged {n} finished task(s)"))
        }
        Command::Stats => Ok(stats::summary(store.tasks(), today)),
    }
}

fn set_status(store: &mut Store, id: u32, status: Status) -> Result<()> {
    let task = store.get(TaskId(id))?;
    if task.status == status {
        bail!("{} is already {status}", task.id);
    }
    store.set_status(TaskId(id), status)?;
    Ok(())
}
