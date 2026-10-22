# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/).

## [0.2.0] - 2026-10-22

### Added
- `orq mcp`: MCP server over stdio (`claude mcp add orq -- orq mcp`) with the
  tools `verify`, `run_affected_tests`, `explain_error` and `impact`.
  `verify` returns the compact text plus structured JSON.
- `orq setup [--apply] [--cranelift]`: detects nightly + Cranelift, mold/wild,
  sccache and cargo-nextest. It proposes a dev-profile block for
  `.cargo/config.toml` as a diff and only writes it with `--apply`, backing up
  the previous file first. It never installs anything.
- `orq stats [--json] [--last N]`: median and p90 duration, cache hit rate,
  tokens, and hit/miss and daemon/in-process breakdowns from
  `target/orq/stats.jsonl`. `orq verify` and the MCP server append one record
  per run.
- `benches/compare.sh` and `docs/benchmarks.md`: `cargo build && cargo test`
  compared with `orq verify`.
- `docs/claude-code.md`: setup guide for Claude Code.
- Diagnostics now carry the primary span label
  (`^ expected `u64`, found `String``).

### Changed
- The workspace root is found without spawning cargo, which saves about 70 ms
  per run.

### Fixed
- A tracked symlink to a directory no longer disables the result cache.

## [0.1.0] - 2026-10-22

### Added
- `orq verify [--files …] [--up-to check|clippy|test] [--budget N] [--no-cache] [--json]`:
  only affected crates are verified with a fmt → check → clippy → test ladder
  that stops at the first error; results are cached and compressed to a token
  budget. Exit code 0 pass, 1 fail, 2 internal error.
- `orq explain <CODE>`: `rustc --explain` trimmed to a budget.
- `orq daemon start|stop|status`: warm daemon over a Unix socket that watches
  the workspace, precompiles (`cargo check`) after each change and answers
  `verify` requests; it stops after 30 minutes idle. Unix only for now.
- Per-project configuration in `orq.toml` (`budget`, `up_to`, `lang`,
  `nextest`, `ignore`).
