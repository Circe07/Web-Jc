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

## Wave 4 — Performance, security, real projects and distribution · closed 2026-10-22 · `v0.3.0`

Before launching, the lead added these contracts to `orq-core`:
- `BuildOptions`, in `orq.toml` `[build]` and on `VerifyRequest.build`, and
  already part of the cache key.
- `Config.autostart_daemon`.
- `ipc::prepare_socket_dir`: a private 0700 directory, checked by both the
  daemon and the client.

While doing this the lead found and fixed a race in the owner check: the
probe file was shared by parallel tests in the same process.

| Task | Result | New tests |
| --- | --- | --- |
| T4.1 daemon security | socket 0600, peer-uid check that refuses when credentials are unknown, requests capped at 1 MiB / 30 s | 5 |
| T4.2 runner | one `cargo clippy` for check and clippy, fmt in parallel, `BuildOptions` passed to cargo | 3 |
| T4.3 cache | `rustc -Vv` memo keyed by toolchain fingerprint, file index (size, mtime, ctime, inode; 2 s racy window), key format v2 | 8 |
| T4.4 CLI/MCP | `--features/--all-features/--no-default-features/--target` and MCP arguments; daemon autostart from `orq mcp`; `client::spawn_daemon` | 7 |
| T4.5 distribution | `release.yml` (4 targets, checksums, notes from CHANGELOG), manual crates.io publish job, Homebrew formula, `docs/release.md`, CI `package` job | — |

Measurements (release build):

| Case | Before | After |
| --- | --- | --- |
| Cached `orq verify` without the daemon (`multi`) | ~130 ms | **54–58 ms** |
| Cache key, fresh process with a warm db (ripgrep `full_rebuild`) | 103–170 ms | 3–7 ms |
| Cold `orq verify` on `multi` | 1.1 s | 0.8 s (baseline `cargo build && cargo test`: 0.7 s) |
| Cold `orq verify` on ripgrep | 30.2 s | 26.6 s (baseline: 19.4 s) |

End-to-end check by the lead: `orq verify --features x` on a crate with
`#[cfg(feature = "x")] compile_error!` reports `FAIL check` with "x on";
without the flag it passes.

Fixed during integration:
- The cache index tests failed on macOS. `fs::copy` preserves the mtime
  there, so the copied fixture looked settled and got indexed. The test helper
  now writes the files instead of copying them; production code was correct.

Open items:
- A cold run on large projects is still ~37% slower than the baseline, because
  of `--all-targets` plus fmt and tests. Possible next steps: drop
  `all_targets` for the clippy step, or make it configurable.
- `--all-targets` breaks on crates whose benches need nightly (ripgrep's
  globset fails with E0554). This needs a config option, for example
  `all_targets = false` or excluding benches.
- Not done yet: finer test selection, measuring compression quality with real
  tasks, and named pipes on Windows.
- Publishing to crates.io and creating releases need a manual step (a tag
  pushed by the owner and a `CARGO_REGISTRY_TOKEN`).

## Wave 5 — Cold speed, Claude Code integration and robustness · closed 2026-10-23 · `v0.4.0`

Before launching, the lead added these contracts:
- `ImpactSet.changed_files` and `BuildOptions.targets` (`auto` | `all`). Both
  are part of the cache key, together with any bench/example files that
  changed.
- Stubs for `Runner::prebuild_tests` and `orq_runner::fix`, and for the
  `orq fix`, `orq hook` and `orq doctor` subcommands.

The wave was paused overnight at the user's request. In-progress work was
saved as `wip` commits, and work resumed at 04:00 (Madrid) through a
scheduled task. On resuming the disk was full: the lead removed the
worktrees of the waves that were already merged.

| Task | Result |
| --- | --- |
| T5.1 runner | `targets = auto` with per-package flags from `cargo metadata`; tests that failed last time run first (`target/orq/last_failures.json`); `prebuild_tests` |
| T5.2 daemon | warm-up at `min(up_to, clippy)`, then a test prebuild that is dropped when a new change arrives; stats with `source = daemon-warmup` |
| T5.3 fix | `apply_fixes` (`cargo clippy --fix` on the affected crates, diff by snapshot), `orq fix`, MCP `apply_fix` |
| T5.4 hook/plugin | `orq hook` (PostToolUse, `decision: block`); plugin with hook + `.mcp.json` + local marketplace |
| T5.5 robustness | `benches/realworld.sh` + nightly `realworld.yml` (ripgrep, serde, clap, tokio), `orq doctor`, estimated savings in `orq stats` |

Bugs found by the real-world run, fixed by the lead:
1. **`-p <name>` was ambiguous** when a member shares its name with a registry
   package (tokio, serde). Every incremental verify on those workspaces
   failed. The runner now uses `path+file://<dir>#<name>` specs. The
   regression test reproduces the ambiguity with `itoa`, and is skipped when
   offline.
   - Follow-up on macOS: specs must use the canonical path (`/var` is a
     symlink to `/private/var`). Reproduced locally with a symlinked `TMPDIR`.
2. **Tool failures were cached:** the "ambiguous" failure was replayed from
   the cache even after the fix. Results whose error has no file and no code
   are no longer stored.
3. **Errors inside dependencies flooded the report** (34 E0433 in
   `trybuild`). They are now folded into one entry per dependency.

Other fixes during integration:
- `apply_fixes` used `--all-targets`; it now uses the runner's target
  selection.
- A flaky `ETXTBSY` in the cargo wrapper used by the runner tests.

Measurements on ripgrep (release build, edit in `crates/matcher`, fresh
target dir):

| Case | v0.3.0 | v0.4.0 |
| --- | --- | --- |
| Cold `orq verify` (baseline `cargo build && cargo test`: 34.1 s) | 40.3 s | 39.3 s |
| Verify right after an edit, warm daemon | **28.3 s** | **7.2 s** |
| Non-Rust edit through the hook | — | 7 ms (debug build) |

Open items:
- A cold run without the daemon is still ~15% slower than the baseline,
  because it also runs fmt and clippy.
- `StatsRecord.raw_tokens` (proposed by T5.5) would let us measure savings
  instead of estimating them. It needs a contract change.
- Measuring compression quality with real agent tasks (wave 6), and named
  pipes on Windows.

## Wave 6 — Measuring compression quality · closed 2026-10-23

Goal: measure whether orq's compact report keeps what an agent needs to fix
the code, with real failures instead of hand-picked examples.

| Task | Result |
| --- | --- |
| T6.1 corpus | 22 cases in `eval/cases/` (borrow checker, lifetimes, type mismatch, imports, traits, macros, features, workspace API change, clippy, failing tests, panics). Each one has `project/`, `fix/` and the facts the report must keep. `eval/validate.sh` checks that each case fails at the expected step and passes once fixed. |
| T6.2 offline metrics | `orq-eval` crate: runs every case and records tokens and fact recall for raw cargo output (`raw-step`, `raw-ladder`) and for orq at budgets 200/500/1500. CI job `eval-offline` with `--min-recall 0.9`. |
| T6.3 agent harness | `eval/agent/agent_eval.py`: a model fixes each case using either orq's report or raw cargo output, and the harness records turns, tokens and success. Mock mode for CI; manual `eval.yml` workflow with the `ANTHROPIC_API_KEY` secret. |

Offline results (22 cases):

| Mode | Mean tokens | vs raw-step | Mean recall | Fail step right |
| --- | ---: | ---: | ---: | ---: |
| raw-step (failing cargo command) | 556 | 100% | 1.000 | — |
| raw-ladder (`cargo build && cargo test`) | 480 | 86% | 0.955 | — |
| orq@200 | 95 | 17% | 1.000 | 100% |
| orq@1500 | 100 | 18% | 1.000 | 100% |

Bug found by the evaluation, fixed by the lead: **a pass hid the clippy
warnings.** In `clippy-needless-range-loop`, orq said `PASS` and the agent
never saw the lint (recall 0.955, fail step 95%). Changes:
- The `PASS` line now carries the warning count, and the warnings are listed
  below it within the budget.
- New `deny_warnings` option in `orq.toml` (`clippy -- -D warnings`), which
  the case uses.
- `orq.toml` is now part of the cache key.
- `orq-eval` loads each case's `orq.toml`, as real use does.

Open items:
- The agent run against a real model is still pending. At the user's request,
  the harness also supports Gemini's free tier: `--provider gemini`, the
  `GEMINI_API_KEY` secret and the default model `gemini-flash-lite-latest`.
  It respects the per-minute limit and stops cleanly when the daily quota is
  used up. The run is started by hand from the `eval` workflow.
- `StatsRecord.raw_tokens`, and named pipes on Windows (carried over).

## Wave 7 — First run against a real model, and the fixes it led to · closed 2026-10-23

The agent harness ran for the first time against a real model (Gemini
`gemini-flash-lite-latest`, free tier). Every problem it surfaced was fixed
and measured again.

| Change | Where |
| --- | --- |
| 6 harder cases (corpus 22 → 28): `staged-errors-ladder`, `thread-shared-counter`, `generic-bounds-cascade`, `lifetime-struct-iterator`, `workspace-three-crates-api`, `many-failing-tests-two-bugs`. They aim at multi-turn fixes and long cargo output. All pass `eval/validate.sh`. | `eval/cases/` |
| Suggestions in context: the line with the edits applied, its number and rustc's help text, instead of a bare fragment such as `: std::cmp::Ord`. | `orq-runner` (`messages.rs`) |
| Time breakdown per run (model, cargo, orq, deliberate waits). Each mode is timed with its own checker; rate-limit waits are left out. | `eval/agent/agent_eval.py` |
| `--transcripts DIR` saves every conversation. | `eval/agent/agent_eval.py` |
| orq mode denies warnings like the judge does. | `eval/agent/agent_eval.py` |
| A relative `--orq` path is resolved up front. | `eval/agent/agent_eval.py` |

Bugs found by the real run, fixed by the lead:
1. **A relative `--orq` path failed every orq run** (`No such file or
   directory`), because each run works in a temp dir.
2. **Suggestions without context.** rustc's suggested replacement is often an
   insertion (`: Ord`, `+ Clone`). orq printed it alone, with no line and no
   reason.
3. **Unfair timing.** Wall time included the waits for Gemini's per-minute
   limit, and orq mode was charged for cargo (the harness's judge) and orq
   on every turn.
4. **orq and the judge disagreed on clippy warnings.** The judge runs
   `clippy -- -D warnings`; orq only does with `deny_warnings = true`, which
   one case set. In `staged-errors-ladder` orq said `PASS` with a warning
   listed, the judge failed the run, and the agent spent a second turn. The
   transcripts showed it.
5. **Raw mode paid for stack traces.** The container sets
   `RUST_BACKTRACE=1`; orq forces it off for its test runs, the harness did
   not, so every failing test in raw mode came with a full backtrace. The
   harness now sets `RUST_BACKTRACE=0`.
6. **`workspace-three-crates-api` tested space counting.** Both modes
   reached the right failing test and then lost 4 turns on the exact column
   padding it pinned, not on the API change. It now expects `name: amount`
   lines. The new cases were also run through rustfmt (orq flagged the
   ledger crate as unformatted, noise in a crate the agent must not touch).

Agent results, 9 cases × 3 repeats per mode, before fixes 4–6:

| Mode | Fixed | Mean turns | Mean tokens | Feedback tokens/turn | Mean time | Model | Checker |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| raw | 24/27 | 1.33 | 3981 | 735 | 9.2 s | 8.0 s | 1.2 s |
| orq | 24/27 | 1.48 | 3581 | 241 | 10.6 s | 9.5 s | 1.2 s |

- orq sends about 3× fewer feedback tokens and 10% fewer tokens in total.
- One check costs the same: 0.47 s with orq, 0.51 s with cargo.
- orq was slower overall because of extra turns (40 vs 36), part of them from
  bug 4, and because Gemini's latency varied a lot (1.3–16 s per call).
- In `generic-bounds-cascade` one extra orq turn came from the model
  deleting an import on its own; with the same report, the other runs fixed
  it in one turn.

After fixes 4–6 (same 9 cases × 3 repeats):

| Mode | Fixed | Mean turns | Mean tokens | Feedback tokens/turn | Mean time | Model | Checker |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| raw | 27/27 | 1.00 | 2269 | 624 | 14.9 s | 13.9 s | 1.0 s |
| orq | 27/27 | 1.11 | 2043 | 234 | 16.8 s | 16.0 s | 0.8 s |

- Both modes fix every case, `workspace-three-crates-api` included, and
  orq sends 2.7× fewer feedback tokens.
- orq's checker is faster (0.8 s vs 1.0 s per run). The time gap is the
  model: Gemini was slow and erratic during this run (10–50 s per run), and
  orq took 3 extra turns.
- The extra turns, from the transcripts: `lifetime-struct-iterator` (1) is
  the case's trap (E0597 after a naive `lookup<'a>`), a model slip.
  `staged-errors-ladder` (2) is the clippy lint that only shows once the
  code compiles; neither mode shows it on turn 1, but in raw mode the model
  rewrote the loop up front in all 3 runs. Too few runs to tell whether the
  longer raw output causes that.

Open items:
- Run with more repeats and a stronger model, so API latency noise and model
  slips weigh less.
- orq lists `file is not formatted` warnings for the agent's own files, which
  the judge ignores (fmt never fails a case). Harmless, but noise.
