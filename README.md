# orq

Rust build orchestrator for AI agents. `orq` sits between an agent and Cargo and
answers "I changed this — is it OK?" as fast as possible and in as few tokens as
possible: it only verifies the affected crates, runs a fmt → check → clippy →
test ladder that stops at the first failure, caches results, and compresses
diagnostics to a token budget.

Status: `v0.1.0` (CLI + daemon). MCP server, `orq setup` and `orq stats`
are coming next — see [`PLAN.md`](PLAN.md) and
[`docs/progress.md`](docs/progress.md).

## Usage

```sh
cargo install --path crates/orq-cli   # installs the `orq` binary

orq verify                      # verify what `git diff` says changed
orq verify --files src/lib.rs --up-to check
orq verify --json               # structured output
orq explain E0308

orq daemon start                # keep state warm; `verify` delegates to it
orq daemon status
orq daemon stop
```

Example output:

```text
FAIL check (1 error) · crates: broken-type · 0.2s
[1] E0308 mismatched types — src/lib.rs:6:19
    6 |     let id: u64 = req.id.clone();
      |                   ^ expected `u64`, found `String`
```

Optional `orq.toml` at the workspace root:

```toml
budget = 1500          # tokens per response
up_to = "test"         # default last step
lang = "en"            # "en" | "es"
nextest = "auto"       # "auto" | "always" | "never"
ignore = ["docs/**"]
```

Set `ORQ_LOG=debug` for logs (always on stderr).

## Layout

| Crate | Purpose |
| --- | --- |
| `orq-core` | Shared types and traits (contracts) |
| `orq-impact` | Workspace graph and affected crates |
| `orq-runner` | Runs cargo, parses JSON, verification ladder |
| `orq-compress` | Diagnostic compression to a token budget |
| `orq-cache` | Cache key and result storage |
| `orq-daemon` | Watcher, warm state, IPC |
| `orq-mcp` | MCP server |
| `orq-cli` | The `orq` binary |

`fixtures/` holds standalone test workspaces (not part of this workspace).

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
