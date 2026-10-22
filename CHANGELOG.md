# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/).

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
