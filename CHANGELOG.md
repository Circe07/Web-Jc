# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added
- Evaluation of the compression quality (`eval/`, `docs/eval.md`):
  - a corpus of 22 real failure cases with fixes and expected facts;
  - `orq-eval`, offline metrics (tokens and fact recall per budget),
    run in CI with `--min-recall 0.9`;
  - an agent harness (`eval/agent/`, manual `eval.yml` workflow) that
    measures turns, tokens and success rate of a model fixing each case
    with orq's report versus raw cargo output. It works with Claude or with
    Gemini's free tier (`--provider gemini`, `GEMINI_API_KEY`).
- `deny_warnings` in `orq.toml`: clippy runs with `-D warnings`.
- Six harder evaluation cases (28 in total), aimed at multi-turn fixes.
- Agent harness: `--transcripts DIR` saves each conversation, and the report
  shows time per run split into model and checker.

### Added (wave 8)
- `orq verify --fix` (also MCP `fix`, opt-in in the hook): apply
  machine-applicable fixes first, then report which files changed.
- Lints of the crates that compile are reported next to the errors of the
  ones that do not; the signature of the item an error names is shown.
- The daemon verifies each change in the background and answers a verify
  for the same tree from that result.
- The Test step runs only the test targets a change can affect.
- `orq doctor` warns when rust-analyzer shares `target/` and lists fast
  build tools; `orq setup` hints wild and sccache.
- `eval/projects/tasktrack` test project with dev and fix tasks, and
  `benches/edit_loop.py`.

### Performance
- The runner calls the toolchain's binaries directly instead of the rustup
  proxies, and finds cargo-nextest without spawning it.

### Changed
- Suggestions now show the source line with the edit applied, its line
  number and rustc's help text, instead of a bare fragment such as
  `: std::cmp::Ord`.

### Fixed
- A failed `assert_eq!` showed `left`/`right` clipped before the
  difference; long pairs now get more room and a window around it.
- `orq setup` gave the wild linker mold's flag.
- Agent harness: a relative `--orq` path works; time no longer counts the
  waits for the Gemini rate limit, and each mode is timed with its own
  checker; orq mode denies clippy warnings like the judge, so a warning no
  longer reads as `PASS` to the agent while the run fails; raw mode runs
  tests with `RUST_BACKTRACE=0`, as orq does.
- A pass with warnings used to hide them. The `PASS` line now carries the
  warning count, and the warnings are listed below it within the budget.
- `orq.toml` is part of the cache key, so changing it no longer replays an
  old result.

## [0.4.0] - 2026-10-23

### Added
- Claude Code integration:
  - `orq hook` for `PostToolUse` on Edit/Write/MultiEdit: verifies the edited
    file and feeds failures back to Claude as `{"decision":"block"}`. Pass
    and non-Rust files are silent.
  - A plugin in `integrations/claude-code/` (hook + MCP server + local
    marketplace).
- `orq fix` and the MCP tool `apply_fix`: apply rustc/clippy machine-applicable
  suggestions to the affected crates, then verify again.
- `orq doctor`: checks the toolchain, nextest, git, config, target dir,
  cache, socket privacy, daemon and MCP registration.
- `orq stats` shows an estimate of tokens and time saved.
- Nightly real-world CI (`realworld.yml`, `benches/realworld.sh`) on
  ripgrep, serde, clap and tokio; results in `docs/realworld.md`.

### Changed
- `targets = "auto"` (default, in `[build]`): check and clippy build lib, bins
  and tests, and benches/examples only when one of them changed. This avoids
  failing on benches that need nightly (ripgrep's globset).
- Tests that failed last time run first and fail fast.
- The daemon warm-up now runs the same step as a real verify (clippy, not
  check), so its artifacts are reused, and it prebuilds the tests. On
  ripgrep, a verify right after an edit with a warm daemon went from 28.3 s
  to 7.2 s.
- Errors inside a dependency (registry or git checkout) are folded into one
  entry per dependency.

### Fixed
- Workspace members named like a registry dependency (tokio, serde) could not
  be verified: `-p <name>` is ambiguous. Packages are now selected with
  `path+file://<dir>#<name>` specs, using canonical paths.
- A cargo/tool failure is no longer cached as a verification result.

## [0.3.0] - 2026-10-22

### Added
- Feature and target selection: `orq verify --features/--all-features/--no-default-features/--target`,
  the same arguments on the MCP `verify` and `run_affected_tests` tools, and
  `[build]` in `orq.toml`. They are passed to check, clippy and test, and are
  part of the cache key.
- `orq mcp` starts a daemon for the workspace automatically
  (`autostart_daemon`, default on; `ORQ_NO_DAEMON=1` disables it).
- Release workflow: binaries for Linux and macOS (x86_64/aarch64) with
  checksums on every `v*` tag, a manual crates.io publish job, and a Homebrew
  formula template. All crates can now be packaged, and CI checks that.

### Changed
- One compile instead of two: when the ladder reaches clippy, a single
  `cargo clippy` run covers both the check and clippy steps. `cargo fmt` runs
  in parallel with it.
- The output of `rustc -Vv` is saved in the cache database, keyed by a
  toolchain fingerprint, and file hashes are reused when size, mtime, ctime and
  inode are unchanged (with a 2 s racy-write window). A cached `orq verify`
  went from ~130 ms to ~55 ms, and a cache key on a warm db from ~100 ms to
  ~2–6 ms.

### Security
- The daemon socket lives in a private `0700` directory, the socket is `0600`,
  peer uids are verified, and requests are capped at 1 MiB / 30 s. The client
  won't connect through an insecure directory.

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
