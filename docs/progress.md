# Progress

## Wave 0 — Base (lead) · closed 2026-10-22

Merged into `ccr-953ffba1-nejt0x` (PR #1). CI is green on Linux and macOS.

- **T0.1 Scaffold**: workspace of 8 crates, toolchain stable, fast dev profile,
  `unwrap_used`/`expect_used` denied outside tests, dual MIT/Apache-2.0 license.
  Legacy website and todo app removed.
- **T0.2 Contracts** (`orq-core`): every type and trait from PLAN.md §4, plus
  `Config` (`orq.toml`, `ignore` globs), `CoreError`, `CacheKey`,
  `estimate_tokens`, `ImpactSet::affected`, `Step::ladder`. 13 tests.
- **T0.3 Fixtures**: `multi`, `single`, `broken-type` (E0308 @ 6:19),
  `cascade` (E0432 + 12 derived), `failing-test`, `clippy-warn` (3 warnings),
  `build-rs`.
- **T0.4 CI**: fmt, clippy `-D warnings`, and test on ubuntu/macos with
  rust-cache and nextest.

Decisions:
- Added the `toml` dependency (not in PLAN.md §3); `Config` needs it to read `orq.toml`.
- The core traits are `Send + Sync`.
- `insta` is built with the `redactions` and `filters` features.


## Wave 1 — Core (4 parallel subagents) · closed 2026-10-22

Each task ran in its own worktree and was merged into `ccr-953ffba1-nejt0x`
(PR #1). Workspace fmt, clippy `-D warnings` and tests are green (66 tests).

| Task | Crate | Public API | Tests |
| --- | --- | --- | --- |
| T1.1 | `orq-impact` | `CargoImpactAnalyzer::{new, invalidate}`, `detect_changes(root)` | 15 |
| T1.2 | `orq-runner` | `Runner::{new, verify(root, impact, up_to).await}`, `RunnerError` | 16 (7 against real fixtures, 5 snapshots) |
| T1.3 | `orq-compress` | `LlmCompressor::new(lang)`, `MIN_USEFUL_BUDGET = 16` | 12 (8 snapshots, budget property 200/500/1500) |
| T1.4 | `orq-cache` | `RedbCache::{open, open_at, with_max_entries, with_env_source}` | 10 |

End-to-end smoke test (impact → cache → runner → compress, run by the lead):

| Case | Result | Time | Tokens |
| --- | --- | --- | --- |
| `broken-type` | `FAIL check (1 error)`, E0308 at `src/lib.rs:6:19` | 232 ms | 41 |
| `multi`, `leaf` changed | `PASS` · crates: leaf | 574 ms | 15 |
| `multi`, `base` changed | `PASS` · crates: app, base, mid | 422 ms | 18 |
| `multi`, `base` repeated | `PASS` from cache | **61 ms** (target < 200 ms) | 18 |

Measured acceptance: second impact analysis < 300 ms; cache key < 100 ms
(both asserted in tests).

Fixes made by the lead during integration:
- The `clippy-warn` snapshot was flaky because cargo interleaves `lib` and
  `lib test` messages differently between runs. The test now sorts
  diagnostics before snapshotting.
- `Cargo.lock` conflicts were resolved by regenerating it with cargo.

No contract changes were requested.

Notes for wave 2:
- `Diagnostic` has no field for the primary span label (rustc's "expected
  `u64`, found `String`"), so the caret is printed without its text. Option:
  add `label: Option<String>` to `orq-core` before T2.1.
- The runner keeps `lib`/`lib test` duplicates; `orq-compress` deduplicates them.
- The file lock `target/orq/lock` (PLAN.md §7.2) is not implemented yet; it
  belongs to T2.1 (in-process mode) and T2.2 (mutex in the daemon).
- The worktree tool created the subagent worktrees from an old commit
  (850073f); each agent had to move to 8ed4fea by hand. Wave 2 briefs must
  say this explicitly.


## Wave 2 — Phase 1 integration + daemon · closed 2026-10-22 · `v0.1.0`

Before launching the agents, the lead added these contracts to `orq-core`:
- `Pipeline`: the impact → cache → runner flow, shared by the CLI and the
  daemon. A cache failure never fails a verify.
- `StepRunner` and `ImpactAnalyzer::detect_changes`.
- `orq_core::ipc`: the JSON-lines protocol, `socket_path` and a blocking client.
- `Diagnostic.label`: the runner fills it and the compressor prints it after
  the caret.
- MSRV raised to 1.89, for `File::lock`.
- A stub for `orq_daemon::serve`.

| Task | Result | Tests |
| --- | --- | --- |
| T2.1 `orq-cli` | `orq verify` (`--files/--up-to/--budget/--no-cache/--json`, exit 0/1/2), `orq explain`, `orq daemon start\|stop\|status\|run`, lock on `target/orq/lock` | 7 integration + 5 unit |
| T2.2 `orq-daemon` | Unix socket, watcher (debounce 300 ms), metadata invalidation, `cargo check` warm-up, one cargo at a time, 30 min idle shutdown | 4 |
| T2.3 integration (lead) | merge, root detection without spawning cargo, e2e, `v0.1.0` | — |

Measurements on `fixtures/multi` (debug build):

| Case | Time |
| --- | --- |
| `orq verify` in-process, repeated (cache) | ~130 ms, measured from process start; was ~190 ms before removing `cargo locate-project` |
| `orq verify` through the daemon, repeated (cache) | **13 ms** |
| `orq verify` through the daemon after a change (warm) | 0.9 s |

The CI test for the < 200 ms target passes on Linux and macOS.

End-to-end daemon run: `start` → `status` (`warming=true` after the edit) →
`verify` PASS on leaf → cached `verify` (13 ms) → error in `base` gives
`FAIL check` on app, base, leaf and mid with the E0308 label → `stop` →
`status` reports "not running".

Pending:
- Windows: the daemon is Unix-only for now, and the CLI falls back to
  in-process mode.
- `rustc -Vv` still costs ~70 ms of every in-process run. It could be memoized
  on disk with a toolchain fingerprint if needed.

## Wave 3 — MCP, setup and metrics · closed 2026-10-22 · `v0.2.0`

Before launching, the lead moved the shared logic into libraries so that the
three agents never touched the same file:
- `orq_core::root` (workspace discovery) and `orq_core::stats` (`stats.jsonl`).
- `orq_daemon::client::verify`: delegates to the daemon or runs in-process
  under the lock.
- `orq_runner::explain`.
- A stub for each of `orq mcp`, `orq setup` and `orq stats`, in its own file.

| Task | Result | Tests |
| --- | --- | --- |
| T3.1 `orq-mcp` | `orq mcp` over stdio (rmcp 3.7, `ServerHandler` written by hand), tools `verify`, `run_affected_tests`, `explain_error`, `impact`; `docs/claude-code.md` | 4 (duplex JSON-RPC) + 1 (real `orq mcp` binary) |
| T3.2 `orq setup` | detects nightly/Cranelift, mold/wild, sccache, nextest; marked block in `.cargo/config.toml` for dev only; backup; refuses if it would duplicate tables; Cranelift opt-in | 13 unit + 3 integration |
| T3.3 metrics | `orq stats` (nearest-rank median/p90, hit rate, tokens, breakdowns), `benches/compare.sh`, `docs/benchmarks.md` (multi + ripgrep) | 6 unit + 2 integration |
| T3.4 close (lead) | full README, CHANGELOG, `v0.2.0` | — |

Workspace total: 121 tests; fmt, clippy `-D warnings` and tests are green.

Bug found by the benchmark and fixed by the lead: on ripgrep the cache never
hit. A tracked symlink to a directory (`HomebrewFormula`) made the cache key
fail with `EISDIR`. Symlinks are now hashed by their target. The repeated
`orq verify --up-to check` on ripgrep went from 3.5 s (no cache) to
125–171 ms. A regression test was added.

Dogfooding: `orq verify --files crates/orq-cache/src/store.rs --up-to clippy`
on this repo gives `PASS fmt+check+clippy · crates: orq-cache, orq-cli,
orq-daemon, orq-mcp`, and `orq stats` summarizes it.

### Definition of done (PLAN.md §9)

- [x] `orq verify` only compiles the affected crates and its output stays within the budget.
- [x] A repeated verify without changes answers from the cache in < 200 ms: ~130 ms in-process, 13 ms through the daemon.
- [x] The daemon keeps state warm and precompiles after each change.
- [x] `orq mcp` works when registered in Claude Code (`claude mcp add orq -- orq mcp`); its tool descriptions tell the agent to use it instead of `cargo build`.
- [x] `orq setup` proposes and applies the fast dev profile.
- [x] `orq stats` and `benches/compare.sh` show time and tokens against the baseline: 40–56x fewer tokens.
- [x] CI is green on Linux and macOS; README and CHANGELOG are complete.

Known limitations:
- The daemon is Unix-only.
- A cold `orq verify` without the daemon is 1.4–2x slower than
  `cargo build && cargo test`, because it also runs fmt and clippy.
- `rustc -Vv` costs ~70 ms per in-process run.
- The `v0.1.0` and `v0.2.0` tags only exist locally: this session can only
  push to its branch.
