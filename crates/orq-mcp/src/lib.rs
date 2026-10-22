//! orq-mcp: MCP server over stdio exposing `verify`, `run_affected_tests`,
//! `explain_error` and `impact` (PLAN.md T3.1).

use std::path::PathBuf;

use orq_core::CoreError;

/// Serves MCP over stdin/stdout until stdin closes.
///
/// `root` is the workspace to serve (default: the workspace containing the
/// current directory). Nothing but JSON-RPC may be written to stdout.
pub async fn serve_stdio(root: Option<PathBuf>) -> Result<(), CoreError> {
    let _ = root;
    Err(CoreError::Process {
        command: "orq mcp".into(),
        message: "not implemented yet (PLAN.md T3.1)".into(),
    })
}
