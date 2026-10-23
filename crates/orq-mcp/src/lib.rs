//! orq-mcp: MCP server over stdio exposing `verify`, `run_affected_tests`,
//! `explain_error` and `impact` (PLAN.md T3.1).
//!
//! The server is hand-written on top of `rmcp`'s `ServerHandler` (no proc
//! macros, no `schemars`): the input schemas are plain JSON.

mod tools;

use std::path::PathBuf;
use std::sync::Arc;

use orq_core::{root, Config, CoreError};
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, ListToolsResult, PaginatedRequestParams,
    ServerCapabilities, ServerConfig,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, ServerHandler, ServiceExt};
use tokio::io::{AsyncRead, AsyncWrite};

/// Instructions sent to the client in the `initialize` response.
const INSTRUCTIONS: &str = "orq verifies Rust workspaces incrementally. After every edit call \
`verify` instead of running `cargo build`, `cargo check`, `cargo clippy` or `cargo test`: it only \
checks the affected crates, is cached, and returns a compact report. Use `impact` to plan, \
`run_affected_tests` before finishing and `explain_error` for unfamiliar rustc error codes.";

/// MCP handler bound to one workspace.
#[derive(Clone)]
struct OrqServer {
    state: Arc<tools::State>,
}

impl ServerHandler for OrqServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions(INSTRUCTIONS)
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult::with_all_items(tools::definitions()))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        match tools::call(&self.state, &request.name, request.arguments).await {
            Some(result) => Ok(result.into()),
            None => Err(ErrorData::invalid_params(
                format!("unknown tool `{}`", request.name),
                None,
            )),
        }
    }
}

fn process_error(message: impl Into<String>) -> CoreError {
    CoreError::Process {
        command: "orq mcp".into(),
        message: message.into(),
    }
}

/// Serves MCP over an arbitrary reader/writer pair until the reader closes.
///
/// Used by [`serve_stdio`] and by tests (e.g. over `tokio::io::duplex`).
/// `root` is the workspace to serve (default: the workspace containing the
/// current directory).
///
/// Never starts a daemon (see [`serve_stdio`]).
pub async fn serve_io<R, W>(root: Option<PathBuf>, reader: R, writer: W) -> Result<(), CoreError>
where
    R: AsyncRead + Send + Unpin + 'static,
    W: AsyncWrite + Send + Unpin + 'static,
{
    serve(root, reader, writer, false).await
}

/// Whether a daemon may be started for this process.
fn autostart_allowed(config: &Config) -> bool {
    cfg!(unix)
        && config.autostart_daemon
        && !std::env::var_os("ORQ_NO_DAEMON").is_some_and(|v| v == "1")
}

/// Starts the daemon in the background if none answers. Never blocks the
/// caller and never fails the server.
fn autostart_daemon(root: PathBuf) {
    // Fire and forget: the blocking task runs to completion on its own.
    drop(tokio::task::spawn_blocking(move || {
        if orq_daemon::client::daemon_status(&root).is_some() {
            return;
        }
        if let Err(e) = orq_daemon::client::spawn_daemon(&root) {
            tracing::warn!("could not autostart the daemon: {e}");
        }
    }));
}

async fn serve<R, W>(
    root: Option<PathBuf>,
    reader: R,
    writer: W,
    autostart: bool,
) -> Result<(), CoreError>
where
    R: AsyncRead + Send + Unpin + 'static,
    W: AsyncWrite + Send + Unpin + 'static,
{
    let root = root::resolve(root.as_deref())?;
    let config = Config::load(&root)?;
    if autostart && autostart_allowed(&config) {
        autostart_daemon(root.clone());
    }
    let server = OrqServer {
        state: Arc::new(tools::State { root, config }),
    };
    let running = server
        .serve((reader, writer))
        .await
        .map_err(|e| process_error(format!("MCP initialization failed: {e}")))?;
    running
        .waiting()
        .await
        .map_err(|e| process_error(format!("MCP server stopped: {e}")))?;
    Ok(())
}

/// Serves MCP over stdin/stdout until stdin closes.
///
/// `root` is the workspace to serve (default: the workspace containing the
/// current directory). Nothing but JSON-RPC may be written to stdout.
///
/// Unless `autostart_daemon = false` in `orq.toml` or `ORQ_NO_DAEMON=1`, a
/// background daemon is started on unix when none is running. This spawns
/// the current executable, so it must be the `orq` binary.
pub async fn serve_stdio(root: Option<PathBuf>) -> Result<(), CoreError> {
    let (reader, writer) = rmcp::transport::stdio();
    serve(root, reader, writer, true).await
}
