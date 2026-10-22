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

Pending: wave 1 (T1.1–T1.4).
