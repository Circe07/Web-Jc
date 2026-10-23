# Fixtures

Standalone Cargo projects used by the tests (PLAN.md §6). They are excluded
from the main workspace and each one declares its own `[workspace]`, so they
also work when copied to a temporary directory.

| Name | Content | Expected |
| --- | --- | --- |
| `multi` | workspace `base` ← `mid` ← `app`, plus independent `leaf` | passes |
| `single` | one crate with modules | passes |
| `broken-type` | deliberate `E0308` at `src/lib.rs:6:19` | check fails |
| `cascade` | misspelled `use` → `E0432` + 12 derived errors | check fails |
| `failing-test` | `tests::adds_wrongly` fails | test fails |
| `clippy-warn` | 3 clippy warnings | passes (warnings only) |
| `build-rs` | crate with `build.rs` | passes |
