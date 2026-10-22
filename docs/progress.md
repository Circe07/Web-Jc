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

Pending: wave 2 (T2.1 `orq-cli verify`, T2.2 `orq-daemon`, T2.3 integration and `v0.1.0`).
