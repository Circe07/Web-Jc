# Evaluating orq

Question: does orq's compact report help an agent fix Rust errors **as well as,
or better than, raw cargo output**, and at what token cost? The case format is
defined in `eval/README.md`; this page explains how to run the evaluation and
read it.

## What is measured

Two complementary layers:

1. **Offline, deterministic** (`orq-eval`, no API key). For each case it captures
   raw cargo output and orq's report at several budgets and measures size in
   tokens and *fact recall* (does the report still name the error code,
   `file:line`, primary label, failing test?). It runs in CI.
2. **Agent harness** (`eval/agent/agent_eval.py`, needs `ANTHROPIC_API_KEY`).
   Claude receives the task, every `.rs` file and `Cargo.toml` of the broken
   project, and the feedback of the failing step. It replies with full new file
   contents (`<file path="...">...</file>` blocks). The harness applies them,
   re-verifies, and feeds the new result back, for up to `--max-turns` turns.
   - mode `raw`: feedback is the stdout+stderr of the failing cargo step
     (`cargo check --all-targets`, `cargo clippy --all-targets -- -D warnings`,
     `cargo test`).
   - mode `orq`: feedback is `orq verify --no-daemon --no-cache --up-to <step>
     --budget N`.
   - **Success is judged by the case's own cargo ladder** (the same commands as
     `eval/validate.sh`), never by the feedback mode, so both modes are scored
     identically. rustfmt never fails a case.

Per run it records: success, turns used, input/output/cache tokens, feedback
tokens per turn (chars / 4), wall time, stop reasons and any error.

## Running it

Offline layer and corpus validation:

```sh
eval/validate.sh                      # every case fails/passes as declared
cargo test -p orq-eval                # offline eval (see the crate)
```

Agent harness, locally:

```sh
cargo build --release -p orq-cli
pip install -r eval/agent/requirements.txt

# no API, exercises the whole loop (answers on turn 1 for orq, turn 2 for raw)
python3 eval/agent/agent_eval.py --mock --orq target/release/orq

# free: Gemini with an AI Studio key (https://aistudio.google.com/apikey)
export GEMINI_API_KEY=...
python3 eval/agent/agent_eval.py --orq target/release/orq --provider gemini

# cheap smoke run against the Claude API
export ANTHROPIC_API_KEY=...
python3 eval/agent/agent_eval.py --orq target/release/orq \
    --model claude-haiku-5-5 --repeat 1

# the real thing
python3 eval/agent/agent_eval.py --orq target/release/orq \
    --model claude-opus-5-5 --effort medium --repeat 3
```

Options: `--cases DIR` (default `eval/cases`), `--case ID...`, `--modes raw,orq`,
`--max-turns 4`, `--budget 1500` (orq report budget), `--model`,
`--provider auto|anthropic|gemini`, `--rpm 8` (Gemini only), `--effort`
(`low|medium|high|xhigh|max`), `--no-fallbacks`, `--orq PATH`,
`--out eval/results/agent`, `--mock`, `--repeat N`.

Models: `claude-opus-5-5` (default), `claude-sonnet-5-5`, `claude-haiku-5-5`.
Server-side refusal fallbacks are requested by default for Opus and Sonnet and
skipped automatically for Haiku (`--no-fallbacks` disables them everywhere).
Calls retry 408/409/429/5xx up to 5 times inside the SDK; a call that still
fails is recorded as `error`, not as a failed fix.

### Gemini (free tier)

`--provider gemini` (or any `gemini-*`/`gemma-*` model) calls Gemini's REST
API with the standard library only, so `pip install` is not needed. The
default model is `gemini-flash-lite-latest`, Google's alias for the current
Flash-Lite model. On the free tier it has a much higher daily quota than Flash
(hundreds of requests a day versus tens). `gemini-flash-latest` is the
stronger choice when the quota allows.

The free tier limits requests per minute and per day, and the numbers change
often (check the rate-limit page in AI Studio). So that a run fits:
- calls are spaced to `--rpm` (default 8);
- a per-minute 429 or a 5xx is retried with backoff;
- a per-day 429 stops the run cleanly, and the report notes it.

A full run (22 cases × 2 modes, 1–4 turns each) makes about 60–120 calls.
Run a subset with `--case` if the daily quota is lower than that. `--effort`
and the refusal fallbacks are Claude-only and are ignored. Cost is not
estimated for Gemini (free tier).

Free-tier prompts may be used by Google to improve its products. The corpus
holds only the toy projects in `eval/cases/`, so nothing private is sent.

Tests of the harness itself (no API): `python3 -m unittest discover -s eval/agent`
(needs `target/release/orq`, or set `ORQ_BIN`).

### In CI

`.github/workflows/eval.yml` is `workflow_dispatch` only (never on push or PR).
Add the repository secret `GEMINI_API_KEY` (free; the default model is
`gemini-flash-lite-latest`) or `ANTHROPIC_API_KEY` for Claude models. Then run
*Agent eval* from the Actions tab and choose model, requests per minute,
effort, max turns, repeat and cases. The job fails
early with a clear message if the secret is missing, writes the markdown report
to the job summary, and uploads `eval/results/agent` as the
`agent-eval-results` artifact.

## Reading the results

Each run writes `eval/results/agent/<run-id>.jsonl` (one line per run) and
`<run-id>.md`. The markdown has a per-mode table and a per-case table:

- **success rate**: runs whose project passed the ladder within `--max-turns`
  (runs that ended in `error` are excluded and counted separately).
- **mean turns**: lower means the feedback got the agent to a fix faster.
- **mean in+out tokens**: total model tokens per run, cache tokens included in
  input. The headline comparison is this number at equal or higher success.
- **mean feedback tokens/turn**: size of what the tool showed the agent
  (estimated as chars / 4).
- **cost**: an **estimate** from the price table in the script.

A good result for orq: success rate not lower than `raw`, fewer turns and fewer
tokens. A `refusal` outcome records `stop_details.category`; `max_tokens` is
noted in the stop reasons.

## Cost guidance

Prices in the script (USD per million tokens, input / output): Opus 5.5 $4 / $20,
Sonnet 5.5 $2 / $10, Haiku 5.5 $0.10 / $0.50; cache reads 0.1x and cache writes
1.25x of input. Check the current price list before relying on them.

A typical turn sends 2-5K input tokens (task, all sources, feedback; the history
grows each turn) and returns around 0.5K of output. Assuming about 10K input
and 1.5K output tokens per run (2-3 turns), and a 20-case corpus with 2 modes
and `--repeat 1` (40 runs):

| model | per run | full corpus (40 runs) |
|---|---|---|
| `claude-opus-5-5` | ~$0.07 | ~$3 |
| `claude-sonnet-5-5` | ~$0.035 | ~$1.40 |
| `claude-haiku-5-5` | ~$0.002 | ~$0.07 |

Multiply by `--repeat`. Start with `--model claude-haiku-5-5 --repeat 1` as a smoke
run, then move up. The system prompt is short and identical across calls, so it
is cached when large enough, but most input is per-turn content.

## Caveats

- The corpus is small, so single numbers are anecdotal. Variance between runs is
  real: use `--repeat 3` or more and compare means, not single runs.
- Small projects favour raw output (little noise to cut); the benefit of orq
  grows with output size.
- The agent sees the full sources in both modes, so only the feedback differs.
- Wall time includes cargo builds and is not a model-quality signal.
- Mock numbers (`--mock`) only prove the harness works; they say nothing about
  orq's real effect.
