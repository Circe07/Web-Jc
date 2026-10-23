# Using orq from Claude Code

`orq mcp` is an MCP server over stdio. Once registered, Claude Code can verify
your Rust workspace with a handful of tokens instead of running `cargo build`
and `cargo test` and reading their raw output.

## Install

```sh
cargo install --path crates/orq-cli   # puts `orq` on your PATH
```

## Register the server

```sh
# Current project only (default scope, stored per user):
claude mcp add orq -- orq mcp

# Shared with your team through .mcp.json in the repository:
claude mcp add --scope project orq -- orq mcp

# Pin the workspace explicitly (useful when Claude Code starts elsewhere):
claude mcp add orq -- orq mcp --root /path/to/workspace
```

Without `--root`, orq serves the Cargo workspace that contains the directory
Claude Code was started in. Check it with `claude mcp list`.

The daemon starts automatically: when `orq mcp` starts and no daemon is
running for the workspace, it launches one in the background (unix only; it
never delays or fails the server). The server then delegates to the warm
daemon; otherwise it verifies in-process. To disable the autostart set
`autostart_daemon = false` in `orq.toml` or export `ORQ_NO_DAEMON=1` (which
also forbids using a daemon at all). You can still run `orq daemon start` and
`orq daemon stop` by hand.

## Tools

| Tool | Arguments | What it does |
| --- | --- | --- |
| `verify` | `files?: string[]`, `up_to?: "check" \| "clippy" \| "test"`, `budget?: number`, `features?: string[]`, `all_features?: bool`, `no_default_features?: bool`, `target?: string` | Verifies the crates affected by the changes (fmt, check, clippy, tests; stops at the first failing step). Returns a compact text report plus the same data as structured JSON. A failing build is a normal result, not a tool error. |
| `run_affected_tests` | `files?: string[]`, `budget?: number`, plus the same build arguments as `verify` | Shortcut for `verify` with `up_to = "test"`. |
| `explain_error` | `code: string` | Short explanation and typical fix for a rustc error code such as `E0308`. |
| `impact` | `files?: string[]` | Returns `{changed, dependents, full_rebuild}` (crate names) without building anything. Useful to plan. |

`files` are relative to the workspace root. When omitted, orq detects the
changes with `git`. `budget` defaults to the `budget` in `orq.toml`. The build arguments override the `[build]` section of `orq.toml` (same as `orq verify --features a,b --all-features --no-default-features --target <triple>`). Every
`verify` is recorded in `<target>/orq/stats.jsonl` with `source = "mcp"`
(see `orq stats`).

## Suggested CLAUDE.md snippet

```markdown
## Building and testing

This project uses the `orq` MCP server. Do NOT run `cargo build`,
`cargo check`, `cargo clippy` or `cargo test` yourself.

- After every edit call the `verify` tool (use `up_to: "check"` while iterating).
- Before finishing call `run_affected_tests`.
- If a report shows an error code you do not know, call `explain_error`.
- Call `impact` to see which crates a change touches before a big refactor.
```

## Example

`verify` on a workspace with a type error:

```text
FAIL check (1 error) · crates: broken-type · 0.2s
[1] E0308 mismatched types — src/lib.rs:6:19
    6 |     let id: u64 = req.id.clone();
      |                   ^ expected `u64`, found `String`
```

The structured content of the same call is the JSON form of the report
(`outcome`, `steps`, `crates`, `cache_hit`, `truncated`, ...), the same as
`orq verify --json`.

`impact` with `{"files": ["crates/base/src/lib.rs"]}`:

```text
impact: changed [base], dependents [mid, leaf, app]
```

## Automatic verification with the hook

The MCP tool only helps if the agent remembers to call it. `orq hook` is a
[`PostToolUse` hook](https://code.claude.com/docs/en/hooks) that runs after
every successful `Edit`, `Write` or `MultiEdit`, verifies the edited file and
tells Claude when the build broke.

Add it to `.claude/settings.json` (shared) or `.claude/settings.local.json`:

```json
{
  "hooks": {
    "PostToolUse": [
      {
        "matcher": "Edit|Write|MultiEdit",
        "hooks": [{ "type": "command", "command": "orq hook", "timeout": 120 }]
      }
    ]
  }
}
```

(`timeout` is in seconds.) Or install the [plugin](#plugin), which registers
the same hook and the MCP server together.

What the hook does:

- Reads the hook JSON from stdin and takes the edited path from
  `tool_input.file_path` (always absolute); `cwd` resolves relative paths.
- Only `.rs`, `Cargo.toml`, `Cargo.lock` and `build.rs` matter. Any other
  file, a file outside a cargo workspace, a path ignored by `orq.toml`
  (`ignore`) or malformed input exits 0 with no output, immediately.
- Otherwise it runs the same pipeline as `orq verify --files <file>` (daemon
  when running, unless `ORQ_NO_DAEMON=1`) and records the run in
  `orq stats` with `source = "hook"`.
- Pass: exit 0, no output, nothing added to the context.
- Fail: exit 0 and a JSON decision on stdout,
  `{"decision":"block","reason":"orq verify failed after your edit:\n<compact report>"}`.
  For `PostToolUse` the docs say `decision: "block"` "adds the `reason` next
  to the tool result"; the tool already ran, so the file stays as written and
  Claude simply sees the error and fixes it. This is preferred to exit code 2
  plus stderr because it is the structured form and keeps stderr for logs.
- Internal errors (daemon, cargo, bad JSON) are logged to stderr and exit 0:
  the hook never blocks the agent.

Options: `orq hook --up-to <fmt|check|clippy|test>` (default `check`) and
`--budget <tokens>` (default 600). The defaults keep the loop fast, since the
hook runs after every edit and its failures land in the agent's context. Let
the agent run clippy and tests through the MCP tools (or raise `--up-to` if
you prefer a stricter hook).

## Plugin

`integrations/claude-code/` in this repository is a Claude Code marketplace
with one plugin, `orq`, that registers the hook above and the MCP server
(`orq mcp`) in one step. The plugin does not ship the binary: install `orq`
first.

```sh
cargo install --path crates/orq-cli      # orq on PATH; check with `orq doctor`
claude plugin marketplace add ./integrations/claude-code
claude plugin install orq@orq-marketplace
```

Layout (per the [plugin reference](https://code.claude.com/docs/en/plugins-reference)):

```text
integrations/claude-code/
├── .claude-plugin/marketplace.json   # marketplace "orq-marketplace"
└── orq/
    ├── .claude-plugin/plugin.json
    ├── hooks/hooks.json              # PostToolUse -> `orq hook`
    ├── .mcp.json                     # `orq mcp`
    └── README.md
```

Both manifests pass `claude plugin validate`.

## Troubleshooting

- Nothing on stdout but JSON-RPC: logs go to stderr. Use `RUST_LOG=debug` and
  `claude --debug` to see them.
- "not inside a cargo workspace": pass `--root`.
