# orq plugin for Claude Code

Installs two things in one step:

- a `PostToolUse` hook (`orq hook`) that verifies every edit of a Rust file
  (`.rs`, `Cargo.toml`, `Cargo.lock`, `build.rs`) and feeds a compact error
  report back to Claude when the build breaks; it is silent when all is well;
- the `orq` MCP server (`orq mcp`): `verify`, `run_affected_tests`,
  `explain_error` and `impact`.

## Install

1. Install the `orq` binary and make sure it is on your `PATH` (the plugin
   only runs `orq`; it does not ship it):

   ```sh
   cargo install orq-cli        # or: cargo install --path crates/orq-cli
   orq doctor
   ```

2. Add the marketplace and install the plugin. From a checkout of this
   repository:

   ```sh
   claude plugin marketplace add ./integrations/claude-code
   claude plugin install orq@orq-marketplace
   ```

Restart Claude Code (or run `/reload-plugins`). Check with `/plugin` and
`/hooks`.

## Behaviour

The hook verifies only the edited file's affected crates, up to `check`
(fmt and cargo check), with a 600 token budget, so it stays fast. Use the MCP
tools for clippy and tests. Non-Rust files and files outside a cargo
workspace exit immediately without output. Internal errors never block the
agent. Set `ORQ_NO_DAEMON=1` to disable the daemon.

To change the depth, register your own hook command in `settings.json`, for
example `orq hook --up-to clippy --budget 800`.

## Validate (maintainers)

```sh
claude plugin validate ./integrations/claude-code/orq
claude plugin validate ./integrations/claude-code
```
