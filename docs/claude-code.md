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

Optional: run `orq daemon start` in the workspace. The server then delegates
to the warm daemon automatically; otherwise it verifies in-process (set
`ORQ_NO_DAEMON=1` to forbid the daemon).

## Tools

| Tool | Arguments | What it does |
| --- | --- | --- |
| `verify` | `files?: string[]`, `up_to?: "check" \| "clippy" \| "test"`, `budget?: number` | Verifies the crates affected by the changes (fmt, check, clippy, tests; stops at the first failing step). Returns a compact text report plus the same data as structured JSON. A failing build is a normal result, not a tool error. |
| `run_affected_tests` | `files?: string[]`, `budget?: number` | Shortcut for `verify` with `up_to = "test"`. |
| `explain_error` | `code: string` | Short explanation and typical fix for a rustc error code such as `E0308`. |
| `impact` | `files?: string[]` | Returns `{changed, dependents, full_rebuild}` (crate names) without building anything. Useful to plan. |

`files` are relative to the workspace root. When omitted, orq detects the
changes with `git`. `budget` defaults to the `budget` in `orq.toml`. Every
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

## Troubleshooting

- Nothing on stdout but JSON-RPC: logs go to stderr. Use `RUST_LOG=debug` and
  `claude --debug` to see them.
- "not inside a cargo workspace": pass `--root`.
