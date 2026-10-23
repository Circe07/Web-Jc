# orq

Rust build orchestrator for AI agents. `orq` sits between an agent and Cargo.
It answers "I changed this — is it OK?" as fast as possible and in as few
tokens as possible:

- **Impact analysis**: only the crates containing changed files and their
  reverse dependents are verified.
- **Verification ladder**: fmt → check → clippy → tests (nextest when
  available). It stops at the first step with errors. Warnings don't stop it
  (unless `deny_warnings = true`), but they are listed even on a pass.
- **Result cache**: a BLAKE3 key over the relevant sources, toolchain,
  features/target and environment. An unchanged state answers from the cache
  in ~55 ms (~13 ms through the daemon).
- **LLM compression**: diagnostics are deduplicated, the first real error is
  shown first, cascades are marked "possibly derived", and the output always
  fits a token budget.
- **Daemon**: keeps metadata warm and precompiles after each change.
- **MCP server**: Claude Code (or any MCP client) calls `verify` instead of
  `cargo build/test`.

```text
FAIL check (1 error) · crates: broken-type · 0.2s
[1] E0308 mismatched types — src/lib.rs:6:19
    6 |     let id: u64 = req.id.clone();
      |                   ^ expected `u64`, found `String`
```

```text
PASS fmt+check+clippy+test · crates: app, base, mid · 0.4s (cache: no)
```

## Installation

Requires a stable Rust toolchain (MSRV 1.89) and `git`.
`cargo-nextest` is optional.

```sh
cargo install orq-cli                    # from crates.io (once published); installs `orq`
# or from source:
git clone https://github.com/circe07/orq && cd orq
cargo install --path crates/orq-cli
```

Prebuilt binaries for Linux and macOS (x86_64/aarch64) are attached to each
GitHub release, and there is a Homebrew formula template in `packaging/homebrew/`
(see [`docs/release.md`](docs/release.md)).

## Usage

```sh
orq verify                         # verify what `git diff` + untracked files changed
orq verify --files src/lib.rs      # explicit files (relative to the current dir)
orq verify --up-to check           # stop after `cargo check`
orq verify --budget 500 --json     # structured output, smaller budget
orq verify --no-cache              # ignore the result cache
orq verify --features serde --target wasm32-unknown-unknown
orq verify --all-features          # also --no-default-features

orq explain E0308                  # short `rustc --explain`

orq daemon start                   # warm state + precompile on change; verify delegates to it
orq daemon status
orq daemon stop                    # (it also stops after 30 min idle)

orq setup                          # detect fast tools, show proposed .cargo/config.toml diff
orq setup --apply                  # write it (backs up the old file)

orq stats                          # median/p90 time, cache hit rate, tokens, savings (estimate)
orq fix                            # apply rustc/clippy machine-applicable fixes, then verify
orq doctor                         # check toolchain, nextest, cache, socket, daemon, MCP setup
```

Exit codes: `0` pass, `1` verification failed, `2` internal error.

## Use with Claude Code

The quickest way is the plugin, which installs both the MCP server and a
hook that verifies every Rust edit automatically. Failures are fed back to
Claude; a pass is silent.

```sh
# from a clone of this repository:
claude plugin marketplace add ./integrations/claude-code
claude plugin install orq@orq-marketplace
```

To install only the MCP server:

```sh
claude mcp add orq -- orq mcp
```

`orq mcp` starts a daemon for the workspace automatically, so repeated
verifications stay warm. Set `autostart_daemon = false` in `orq.toml` or
`ORQ_NO_DAEMON=1` to turn that off. The server exposes four tools:
- `verify`: arguments `files?`, `up_to?`, `budget?`.
- `run_affected_tests`
- `explain_error`
- `impact`: helps plan which crates a change touches.
- `apply_fix`: applies the compiler's machine-applicable suggestions, then
  verifies again.

See [`docs/claude-code.md`](docs/claude-code.md) for per-project setup and a
suggested `CLAUDE.md` snippet.

## Configuration

Optional `orq.toml` at the workspace root. Every key is optional:

```toml
budget = 1500          # tokens per response
up_to = "test"         # default last step: fmt | check | clippy | test
lang = "en"            # report language: "en" | "es"
nextest = "auto"       # "auto" | "always" | "never"
ignore = ["docs/**"]   # changes to these paths are ignored (* and ** globs)
autostart_daemon = true
deny_warnings = false  # clippy -- -D warnings: warnings fail the ladder

[build]                # passed to check/clippy/test (not fmt)
features = ["serde"]
all_features = false
no_default_features = false
target = "x86_64-unknown-linux-gnu"
```

Environment variables:
- `ORQ_LOG=debug` turns on logs. They go to stderr, never stdout.
- `ORQ_NO_DAEMON=1` forces in-process mode.
- `CARGO_TARGET_DIR` is respected. orq keeps its own state in
  `<target>/orq/`: the result cache, `stats.jsonl` and the lock file.

## Benchmarks

Full tables and the exact commands are in
[`docs/benchmarks.md`](docs/benchmarks.md). You can reproduce them with
`benches/compare.sh <project> [N] [orq-binary]`. The numbers below are a
summary.

| Project | Mode | Time | Tokens of output |
| --- | --- | ---: | ---: |
| `fixtures/multi` | `cargo build && cargo test` | 407 ms | 586 |
| `fixtures/multi` | `orq verify` (full ladder) | 827 ms | 16 |
| `fixtures/multi` | `orq verify`, unchanged (cache) | 143 ms (55 ms since v0.3.0) | 16 |
| ripgrep | `cargo build && cargo test` | 4.4 s | 4724 |
| ripgrep | `orq verify` (full ladder) | 6.6 s | 83 |
| ripgrep | `orq verify --up-to check`, unchanged (cache) | ~0.15 s | 22 |

The agent sees 40–56x fewer tokens. A single cold edit takes longer than the
baseline because orq also runs fmt and clippy. The time saving comes from the
cache and the daemon: a cached `verify` through the daemon answers in ~13 ms.

## Layout

| Crate | Purpose |
| --- | --- |
| `orq-core` | Shared types, traits, `Pipeline`, IPC protocol, config, stats |
| `orq-impact` | Workspace graph and affected crates |
| `orq-runner` | Runs cargo, parses JSON diagnostics, verification ladder, `explain` |
| `orq-compress` | Diagnostic compression to a token budget |
| `orq-cache` | Cache key and redb storage (LRU, 5 000 entries) |
| `orq-daemon` | Watcher, warm state, Unix-socket server, client used by CLI/MCP |
| `orq-mcp` | MCP server over stdio |
| `orq-cli` | The `orq` binary |

`fixtures/` holds the standalone test workspaces, which are not part of this
workspace. [`PLAN.md`](PLAN.md) is the design, and
[`docs/progress.md`](docs/progress.md) is the build log.

**Security.** Only the user who started the daemon can reach it. The socket
lives in a private directory: `$XDG_RUNTIME_DIR`, or a `0700` directory
under the temp dir that must be owned by you and must not be a symlink. The
socket file itself is `0600`. The daemon checks each connection's peer uid and
closes anything that doesn't match its own without reading it; if it can't read
the credentials, it refuses the connection. Requests are limited to 1 MiB and
30 s, so a local process can't wedge the daemon. The client also refuses to
connect to a socket in a directory that fails these checks.

Limitations: the daemon is Unix-only for now. On Windows, `verify` always runs
in-process.

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at
your option.
