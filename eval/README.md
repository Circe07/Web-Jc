# orq evaluation

Does orq's compact report help an agent fix Rust errors as well as raw cargo
output does, with fewer tokens? This directory answers that in two ways:

- **Offline, deterministic** (`orq-eval`, `crates/orq-eval`): for every case,
  capture raw cargo output and orq's report at several budgets, and measure
  the size in tokens and the *fact recall*: does the report still contain
  what is needed to fix the bug (error code, file:line, primary label,
  failing test name)? It needs no API key and runs in CI.
- **With a real agent** (`eval/agent/agent_eval.py`): a model gets the broken
  project and the feedback (raw cargo output, or orq's report), returns a
  patch, the harness applies it and verifies again, for up to K turns. It
  measures the success rate, the number of turns and the tokens. It needs a
  `GEMINI_API_KEY` (free tier, `--provider gemini`) or an
  `ANTHROPIC_API_KEY`. `--mock` runs the whole loop without the API, for
  testing.

## Case format (contract)

Each case is a directory `eval/cases/<id>/`:

```
eval/cases/<id>/
  case.toml     # metadata, below
  project/      # the BROKEN cargo project (standalone: its Cargo.toml has [workspace])
  fix/          # reference fix: files that, copied over project/, make it pass
```

`case.toml`:

```toml
id = "type-mismatch-parse"           # same as the directory name
category = "type-mismatch"            # free text, used to group results
difficulty = "easy"                   # easy | medium | hard
description = "Make `handle` return the request id as a number."  # the task shown to the agent
up_to = "test"                        # the step that must pass after the fix: check | clippy | test
expected_fail_step = "check"          # where the broken project fails: fmt | check | clippy | test

# Facts a useful report must contain. Every field is optional and is matched
# against the report text (file/line as "file:line").
[[facts]]
code = "E0308"
file = "src/lib.rs"
line = 6
label = "expected `u64`"              # substring of the primary span label

[[facts]]
test = "tests::parses"                # name of a failing test that must be named
```

Rules:
- `project/` must fail at `expected_fail_step`. `project/` with `fix/`
  copied over it must pass `cargo` up to `up_to`. `eval/validate.sh`
  checks both.
- Cases must build offline (no crates.io dependencies) and quickly.
- Do not put the answer in `description`.

## Corpus

28 cases (`eval/validate.sh` checks all of them). "Fails at" is
`expected_fail_step`; every case must pass up to its `up_to` after the fix.

| id | category | difficulty | fails at |
|----|----------|------------|----------|
| `type-mismatch-parse` | type-mismatch | easy | check |
| `type-mismatch-question-mark` | type-mismatch (`?` conversion, E0277) | medium | check |
| `generic-trait-object-mismatch` | type-mismatch (generics, `dyn`, E0308) | hard | check |
| `borrow-e0502-push-while-borrowed` | borrow-checker (E0502) | easy | check |
| `borrow-e0499-double-mut` | borrow-checker (E0499) | medium | check |
| `borrow-use-after-move` | borrow-checker (E0382) | easy | check |
| `lifetime-missing-specifier` | lifetime (E0106) | easy | check |
| `lifetime-return-local` | lifetime (E0515) | medium | check |
| `trait-missing-ord-display` | missing-trait (E0277 x2) | medium | check |
| `import-cascade-misspelled-trait` | unresolved-import (1 root cause, 7 errors) | hard | check |
| `wrong-method-field-names` | wrong-name (E0599, E0609) | easy | check |
| `non-exhaustive-match` | non-exhaustive (E0004) | easy | check |
| `workspace-api-change` | workspace (fix in the dependent crate) | hard | check |
| `feature-gated-default` | feature-gate (E0425 without `fast`) | medium | check |
| `macro-invocation-type` | macro (E0308 in a `macro_rules!` call) | hard | check |
| `clippy-needless-range-loop` | clippy (`up_to = "clippy"`) | easy | clippy |
| `failing-test-add` | logic-bug | easy | test |
| `logic-off-by-one-window` | logic-bug (off by one) | medium | test |
| `logic-wrong-boolean` | logic-bug (wrong `&&`/`\|\|`) | easy | test |
| `logic-page-count-rounding` | logic-bug (integer rounding) | easy | test |
| `panic-index-out-of-bounds` | test-panic (index) | medium | test |
| `panic-unwrap-none` | test-panic (`unwrap` on `None`) | medium | test |
| `staged-errors-ladder` | staged (E0308, then clippy, then a test) | hard | check |
| `thread-shared-counter` | concurrency (`Rc` across threads, E0277) | medium | check |
| `generic-bounds-cascade` | missing-trait (7 errors over 4 generic fns) | hard | check |
| `lifetime-struct-iterator` | lifetime (E0106 in a struct + `Iterator` impl) | hard | check |
| `workspace-three-crates-api` | workspace (API change breaks 2 of 3 crates) | hard | check |
| `many-failing-tests-two-bugs` | logic-bug (2 bugs, 4 failing tests) | hard | test |

Notes: `clippy-needless-range-loop` uses the lint name as `code`
(`clippy::needless_range_loop`). `logic-wrong-boolean` uses
`u32::is_multiple_of`, so the corpus needs Rust 1.89 or later (the workspace
`rust-version`).
