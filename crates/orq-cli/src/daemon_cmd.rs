//! `orq daemon start|stop|status|run`.

use std::path::Path;

use anyhow::{bail, Context, Result};
use orq_core::ipc::{self, DaemonRequest, DaemonResponse, DaemonStatus};

use orq_core::root;

use crate::DaemonCommand;

/// Dispatches a daemon subcommand and returns the exit code.
pub fn run(action: DaemonCommand) -> Result<u8> {
    match action {
        DaemonCommand::Start { root } => start(&root::resolve(root.as_deref())?),
        DaemonCommand::Stop { root } => stop(&root::resolve(root.as_deref())?),
        DaemonCommand::Status { root } => status(&root::resolve(root.as_deref())?),
        DaemonCommand::Run { root } => foreground(&root::resolve(root.as_deref())?),
    }
}

fn query_status(root: &Path) -> Result<Option<DaemonStatus>> {
    match ipc::request(root, &DaemonRequest::Status)? {
        Some(DaemonResponse::Status(status)) => Ok(Some(status)),
        Some(other) => bail!("unexpected daemon response: {other:?}"),
        None => Ok(None),
    }
}

fn print_status(status: &DaemonStatus) {
    println!(
        "running pid={} root={} uptime_ms={} requests={} warming={}",
        status.pid,
        status.root.display(),
        status.uptime_ms,
        status.requests,
        status.warming
    );
}

fn start(root: &Path) -> Result<u8> {
    if let Some(status) = query_status(root)? {
        print_status(&status);
        return Ok(0);
    }
    orq_daemon::client::spawn_daemon(root)?;
    match query_status(root)? {
        Some(status) => {
            print_status(&status);
            Ok(0)
        }
        None => bail!("daemon did not answer within 5 s"),
    }
}

fn stop(root: &Path) -> Result<u8> {
    match ipc::request(root, &DaemonRequest::Stop)? {
        Some(_) => {
            println!("stopping");
            Ok(0)
        }
        None => {
            println!("not running");
            Ok(1)
        }
    }
}

fn status(root: &Path) -> Result<u8> {
    match query_status(root)? {
        Some(status) => {
            print_status(&status);
            Ok(0)
        }
        None => {
            println!("not running");
            Ok(1)
        }
    }
}

fn foreground(root: &Path) -> Result<u8> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("building tokio runtime")?;
    runtime.block_on(orq_daemon::serve(
        root,
        orq_daemon::DaemonOptions::default(),
    ))?;
    Ok(0)
}
