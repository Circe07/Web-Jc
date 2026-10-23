# orq evaluation

Does orq's compact report help an agent fix Rust errors as well as raw cargo
output does, with fewer tokens? This directory answers that in two ways:

- **Offline, deterministic** (`orq-eval`, `crates/orq-eval`): for every case,
  capture raw cargo output and orq's report at several budgets, and measure
  the size in tokens and the *fact recall*: does the report still contain
  what is needed to fix the bug (error code, file:line, primary label,
  failing test name)? It needs no API key and runs in CI.
- **With a real agent** (`eval/agent/agent_eval.py`): Claude gets the broken
  project and the feedback (raw cargo output, or orq's report), returns a
  patch, the harness applies it and verifies again, for up to K turns. It
  measures the success rate, the number of turns and the tokens. It needs an
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
