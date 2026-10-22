# orq

Rust build orchestrator for AI agents. `orq` sits between an agent and Cargo and
answers "I changed this — is it OK?" as fast as possible and in as few tokens as
possible: it only verifies the affected crates, runs a fmt → check → clippy →
test ladder that stops at the first failure, caches results, and compresses
diagnostics to a token budget.

Status: early development — see [`PLAN.md`](PLAN.md) and
[`docs/progress.md`](docs/progress.md).

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
