#!/usr/bin/env python3
"""Agent harness: does orq's compact report help a Claude agent fix Rust errors?

For every case, mode (raw cargo output vs. orq report) and repeat, the harness
copies the broken project to a temp dir, shows Claude the failure, applies the
files it returns, and re-verifies, for up to --max-turns turns.

Success is always judged by the case's own cargo ladder (same commands as
eval/validate.sh), never by the feedback mode.

Needs ANTHROPIC_API_KEY unless --mock is given. Stdlib + `anthropic` only.
"""
from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from types import SimpleNamespace

try:  # only needed for real runs; --mock and the tests work without it
    import anthropic  # type: ignore
except ImportError:  # pragma: no cover
    anthropic = None

try:
    import tomllib  # type: ignore  # Python 3.11+
except ImportError:  # pragma: no cover
    tomllib = None

DEFAULT_MODEL = "claude-opus-5-5"
HAIKU = "claude-haiku-5-5"
FALLBACK_BETA = "server-side-fallback-2026-07-01"
MAX_TOKENS = 16000
CMD_TIMEOUT = 900

# Fixed and byte-identical across calls so prompt caching can work.
# Nothing volatile (dates, ids, paths) may ever be added here.
SYSTEM = (
    "You are fixing a Rust crate that does not pass its build and test ladder "
    "(cargo check, clippy, test). You are given the task, the full contents of "
    "every source file, and the output of the failing verification step.\n"
    "Reply ONLY with the full new contents of each file you change, as blocks "
    'of the form <file path="src/lib.rs">...</file>, where the path is relative '
    "to the project root. Include the complete file, not a diff. Do not add any "
    "explanation outside the blocks. Do not change tests unless the task says so."
)

# USD per million tokens (input, output). Estimate; check the current price list.
PRICING = {
    "claude-opus-5-5": (4.0, 20.0),
    "claude-sonnet-5-5": (2.0, 10.0),
    "claude-haiku-5-5": (0.10, 0.50),
}
CACHE_READ_MULT = 0.1
CACHE_WRITE_MULT = 1.25


# --------------------------------------------------------------------------
# Pure helpers (unit-tested)
# --------------------------------------------------------------------------

def est_tokens(text: str) -> int:
    """Rough token count: chars / 4, rounded up."""
    return (len(text) + 3) // 4


def cost_usd(model: str, input_tokens: int, output_tokens: int,
             cache_read: int = 0, cache_write: int = 0) -> float | None:
    """Estimated cost in USD, or None when the model is not in the table.

    `input_tokens` is the uncached input as reported by the API; cache reads
    cost 0.1x and cache writes 1.25x of the input price.
    """
    if model not in PRICING:
        return None
    p_in, p_out = PRICING[model]
    total = (input_tokens * p_in
             + cache_read * p_in * CACHE_READ_MULT
             + cache_write * p_in * CACHE_WRITE_MULT
             + output_tokens * p_out)
    return total / 1_000_000


_FILE_RE = re.compile(r'<file\s+path="([^"]*)"\s*>(.*?)</file>', re.DOTALL)


def parse_file_blocks(text: str) -> list[tuple[str, str]]:
    """Extract (path, contents) pairs from `<file path="...">...</file>` blocks."""
    out = []
    for path, body in _FILE_RE.findall(text):
        if body.startswith("\r\n"):
            body = body[2:]
        elif body.startswith("\n"):
            body = body[1:]
        if body and not body.endswith("\n"):
            body += "\n"
        out.append((path.strip(), body))
    return out


def safe_target(root: Path, rel: str) -> Path | None:
    """Resolve `rel` inside `root`, or None if it escapes or is not allowed."""
    if not rel or "\\" in rel or "\0" in rel or rel.startswith(("/", "~")):
        return None
    parts = Path(rel).parts
    if not parts or ".." in parts or Path(rel).is_absolute():
        return None
    if parts[0] in ("target", ".git") or not (rel.endswith(".rs") or rel.endswith(".toml")):
        return None
    root_r = root.resolve()
    target = (root / rel).resolve()
    try:
        target.relative_to(root_r)
    except ValueError:
        return None
    return target


def apply_files(root: Path, blocks: list[tuple[str, str]]) -> tuple[list[str], list[str]]:
    """Write blocks into `root`. Returns (written paths, rejected paths)."""
    written, rejected = [], []
    for rel, body in blocks:
        target = safe_target(root, rel)
        if target is None:
            rejected.append(rel)
            continue
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(body, encoding="utf-8")
        written.append(rel)
    return written, rejected


def project_files(project: Path) -> list[Path]:
    """Every .rs file and Cargo.toml (not under target/), sorted, relative."""
    found = []
    for p in sorted(project.rglob("*")):
        rel = p.relative_to(project)
        if not p.is_file() or rel.parts[0] in ("target", ".git"):
            continue
        if p.suffix == ".rs" or p.name == "Cargo.toml":
            found.append(rel)
    return found


def build_user_prompt(description: str, project: Path, feedback: str) -> str:
    parts = [f"Task: {description}", "", "Project files:"]
    for rel in project_files(project):
        body = (project / rel).read_text(encoding="utf-8")
        parts.append(f'<file path="{rel.as_posix()}">\n{body}</file>')
    parts += ["", "Verification output:", feedback]
    return "\n".join(parts)


def load_case(case_dir: Path) -> dict:
    """Parse case.toml (tomllib when available, else a tiny top-level parser)."""
    text = (case_dir / "case.toml").read_text(encoding="utf-8")
    if tomllib is not None:
        data = tomllib.loads(text)
    else:  # pragma: no cover
        data = {}
        for line in text.splitlines():
            if line.startswith("["):
                break
            m = re.match(r'^(\w+)\s*=\s*"(.*)"\s*$', line)
            if m:
                data[m.group(1)] = m.group(2)
    data["dir"] = case_dir
    return data


def summarize(records: list[dict]) -> dict:
    """Aggregate run records per mode: counts, means and estimated cost."""
    modes: dict[str, dict] = {}
    for mode in sorted({r["mode"] for r in records}):
        rs = [r for r in records if r["mode"] == mode]
        ok_runs = [r for r in rs if r["outcome"] != "error"]
        n = len(ok_runs)

        def mean(vals):
            vals = list(vals)
            return sum(vals) / len(vals) if vals else 0.0

        fb = [t for r in ok_runs for t in r["feedback_tokens"]]
        costs = [r["cost_usd"] for r in rs if r.get("cost_usd") is not None]
        modes[mode] = {
            "runs": len(rs),
            "errors": len(rs) - n,
            "successes": sum(1 for r in ok_runs if r["success"]),
            "success_rate": (sum(1 for r in ok_runs if r["success"]) / n) if n else 0.0,
            "mean_turns": mean(r["turns"] for r in ok_runs),
            "mean_tokens": mean(r["input_tokens"] + r["output_tokens"] for r in ok_runs),
            "mean_feedback_tokens": mean(fb),
            "cost_total": sum(costs),
            "cost_mean": mean(costs),
        }
    return modes


def render_markdown(run_id: str, model: str, args_desc: str, records: list[dict]) -> str:
    modes = summarize(records)
    lines = [f"# Agent eval {run_id}", "",
             f"- model: `{model}`", f"- settings: {args_desc}",
             f"- runs: {len(records)}", "",
             "Tokens are input (uncached + cache read + cache write) plus output. "
             "Cost is an **estimate** from the price table in the script "
             "(cache reads 0.1x, cache writes 1.25x of input).", "",
             "## Per mode", "",
             "| mode | runs | success rate | mean turns | mean in+out tokens | "
             "mean feedback tokens/turn | est. cost/run | est. cost total | errors |",
             "|---|---|---|---|---|---|---|---|---|"]
    for mode, s in modes.items():
        lines.append(
            f"| {mode} | {s['runs']} | {s['successes']}/{s['runs'] - s['errors']} "
            f"({s['success_rate']:.0%}) | {s['mean_turns']:.2f} | {s['mean_tokens']:.0f} | "
            f"{s['mean_feedback_tokens']:.0f} | ${s['cost_mean']:.4f} | "
            f"${s['cost_total']:.4f} | {s['errors']} |")
    lines += ["", "## Per case", "",
              "| case | mode | runs | success | mean turns | mean in+out tokens | "
              "mean feedback tokens/turn |",
              "|---|---|---|---|---|---|---|"]
    for case in sorted({r["case"] for r in records}):
        for mode in sorted({r["mode"] for r in records}):
            rs = [r for r in records if r["case"] == case and r["mode"] == mode
                  and r["outcome"] != "error"]
            allrs = [r for r in records if r["case"] == case and r["mode"] == mode]
            if not allrs:
                continue
            if not rs:
                lines.append(f"| {case} | {mode} | {len(allrs)} | error | - | - | - |")
                continue
            fb = [t for r in rs for t in r["feedback_tokens"]]
            lines.append(
                f"| {case} | {mode} | {len(allrs)} | "
                f"{sum(1 for r in rs if r['success'])}/{len(rs)} | "
                f"{sum(r['turns'] for r in rs) / len(rs):.2f} | "
                f"{sum(r['input_tokens'] + r['output_tokens'] for r in rs) / len(rs):.0f} | "
                f"{(sum(fb) / len(fb)) if fb else 0:.0f} |")
    errs = [r for r in records if r["error"]]
    if errs:
        lines += ["", "## Errors", ""]
        for r in errs:
            lines.append(f"- {r['case']} / {r['mode']} / repeat {r['repeat']}: {r['error']}")
    return "\n".join(lines) + "\n"


# --------------------------------------------------------------------------
# Running cargo and orq
# --------------------------------------------------------------------------

def _run(cmd: list[str], cwd: Path, env: dict) -> tuple[int, str]:
    try:
        p = subprocess.run(cmd, cwd=cwd, env=env, capture_output=True, text=True,
                           timeout=CMD_TIMEOUT)
    except FileNotFoundError as e:
        raise HarnessError(f"cannot run {cmd[0]}: {e}") from e
    except subprocess.TimeoutExpired:
        return 124, f"(timed out after {CMD_TIMEOUT}s)"
    return p.returncode, (p.stdout or "") + (p.stderr or "")


class HarnessError(Exception):
    """Infrastructure failure (missing tool, etc.), recorded as a run error."""


def cargo_env(target_dir: Path) -> dict:
    env = dict(os.environ)
    env.update({"CARGO_TERM_COLOR": "never", "CARGO_TARGET_DIR": str(target_dir),
                "CARGO_NET_OFFLINE": "true", "NO_COLOR": "1"})
    return env


def judge(project: Path, up_to: str, env: dict) -> tuple[bool, str]:
    """Run the ladder (same commands as validate.sh; fmt never fails a case).

    Returns (passed, raw output of the first failing step).
    """
    steps = [("check", ["cargo", "check", "--all-targets"])]
    if up_to in ("clippy", "test"):
        steps.append(("clippy", ["cargo", "clippy", "--all-targets", "--", "-D", "warnings"]))
    if up_to == "test":
        steps.append(("test", ["cargo", "test"]))
    for _name, cmd in steps:
        rc, out = _run(cmd, project, env)
        if rc != 0:
            return False, out.strip()
    return True, ""


def orq_feedback(orq: str, project: Path, up_to: str, budget: int, env: dict) -> str:
    """orq's compact report for the project (all source files passed as changed)."""
    e = dict(env)
    e["ORQ_NO_DAEMON"] = "1"
    files = ",".join(p.as_posix() for p in project_files(project))
    cmd = [orq, "verify", "--no-daemon", "--no-cache", "--up-to", up_to,
           "--budget", str(budget), "--files", files]
    _rc, out = _run(cmd, project, e)
    return out.strip()


# --------------------------------------------------------------------------
# Model clients
# --------------------------------------------------------------------------

class MockClient:
    """Fake client with the `.beta.messages.create` surface; no network.

    Answers with the case's `fix/` files on turn `answer_turn`, and with a
    useless reply (no file blocks) before that.
    """

    def __init__(self, fix_dir: Path, answer_turn: int):
        self.fix_dir = fix_dir
        self.answer_turn = answer_turn
        self.calls = 0
        self.beta = SimpleNamespace(messages=SimpleNamespace(create=self._create))

    def _create(self, **kw):
        self.calls += 1
        prompt_chars = sum(len(s["text"]) for s in kw.get("system", []))
        for m in kw["messages"]:
            prompt_chars += len(m["content"])
        if self.calls >= self.answer_turn:
            blocks = []
            for p in sorted(self.fix_dir.rglob("*")):
                if p.is_file():
                    rel = p.relative_to(self.fix_dir).as_posix()
                    blocks.append(f'<file path="{rel}">\n{p.read_text(encoding="utf-8")}</file>')
            text = "\n".join(blocks)
        else:
            text = "I need to look at the problem more closely before changing anything."
        usage = SimpleNamespace(input_tokens=est_tokens("x" * prompt_chars),
                                output_tokens=est_tokens(text),
                                cache_read_input_tokens=0,
                                cache_creation_input_tokens=0)
        return SimpleNamespace(content=[SimpleNamespace(type="text", text=text)],
                               stop_reason="end_turn", stop_details=None, usage=usage)


class ModelCallError(Exception):
    """The API call failed for good (after the SDK's own retries)."""


def use_fallbacks(model: str, no_fallbacks: bool) -> bool:
    return not no_fallbacks and model != HAIKU


def call_model(client, model: str, effort: str, messages: list[dict],
               fallbacks: bool, real: bool):
    # No temperature/top_p/top_k, no thinking config, no prefill: they 400.
    kwargs = dict(
        model=model,
        max_tokens=MAX_TOKENS,
        system=[{"type": "text", "text": SYSTEM, "cache_control": {"type": "ephemeral"}}],
        messages=messages,
        output_config={"effort": effort},
    )
    if fallbacks:
        kwargs["betas"] = [FALLBACK_BETA]
        kwargs["fallbacks"] = "default"
    if not real:
        return client.beta.messages.create(**kwargs)
    try:
        return client.beta.messages.create(**kwargs)
    except anthropic.RateLimitError as e:
        raise ModelCallError(f"rate limited: {e}") from e
    except anthropic.APIStatusError as e:
        raise ModelCallError(f"API status {getattr(e, 'status_code', '?')}: {e}") from e
    except anthropic.APIConnectionError as e:
        raise ModelCallError(f"connection error: {e}") from e


def response_text(resp) -> str:
    return "".join(b.text for b in resp.content if getattr(b, "type", None) == "text")


# --------------------------------------------------------------------------
# One run
# --------------------------------------------------------------------------

def run_one(case: dict, mode: str, repeat: int, args, client, run_id: str) -> dict:
    start = time.monotonic()
    rec = {
        "run_id": run_id, "case": case["id"], "mode": mode, "repeat": repeat,
        "model": args.model, "effort": args.effort, "mock": bool(args.mock),
        "success": False, "outcome": "fail", "turns": 0,
        "input_tokens": 0, "output_tokens": 0,
        "cache_read_tokens": 0, "cache_creation_tokens": 0,
        "feedback_tokens": [], "stop_reasons": [], "wall_seconds": 0.0,
        "cost_usd": None, "error": None,
    }
    up_to = case.get("up_to", "test")
    work = Path(tempfile.mkdtemp(prefix="orq-agent-eval-"))
    try:
        project = work / "project"
        shutil.copytree(case["dir"] / "project", project,
                        ignore=shutil.ignore_patterns("target", "Cargo.lock"))
        env = cargo_env(work / "target")
        if args.mock:
            client = MockClient(case["dir"] / "fix", 1 if mode == "orq" else 2)

        def feedback_for(raw_out: str) -> str:
            if mode == "raw":
                return raw_out
            return orq_feedback(args.orq, project, up_to, args.budget, env)

        passed, raw_out = judge(project, up_to, env)
        if passed:
            raise HarnessError("broken project already passes; invalid case")
        fb = feedback_for(raw_out)
        rec["feedback_tokens"].append(est_tokens(fb))
        messages = [{"role": "user",
                     "content": build_user_prompt(case["description"], project, fb)}]
        fallbacks = use_fallbacks(args.model, args.no_fallbacks)

        for _turn in range(args.max_turns):
            resp = call_model(client, args.model, args.effort, messages, fallbacks,
                              real=not args.mock)
            rec["turns"] += 1
            u = resp.usage
            rec["input_tokens"] += getattr(u, "input_tokens", 0) or 0
            rec["output_tokens"] += getattr(u, "output_tokens", 0) or 0
            rec["cache_read_tokens"] += getattr(u, "cache_read_input_tokens", 0) or 0
            rec["cache_creation_tokens"] += getattr(u, "cache_creation_input_tokens", 0) or 0
            rec["stop_reasons"].append(resp.stop_reason)
            if resp.stop_reason == "refusal":
                details = getattr(resp, "stop_details", None)
                cat = getattr(details, "category", None) if details else None
                rec["outcome"] = "refusal"
                rec["error"] = f"refusal (category: {cat})"
                break
            text = response_text(resp)
            written, rejected = apply_files(project, parse_file_blocks(text))
            passed, raw_out = judge(project, up_to, env)
            if passed:
                rec["success"] = True
                rec["outcome"] = "success"
                break
            if _turn + 1 >= args.max_turns:
                break
            note = ""
            if not written:
                note += "Your reply contained no valid <file> blocks, so nothing changed.\n"
            if rejected:
                note += f"Rejected paths: {', '.join(rejected)}\n"
            if resp.stop_reason == "max_tokens":
                note += "Your reply was cut off by the token limit.\n"
            fb = feedback_for(raw_out)
            rec["feedback_tokens"].append(est_tokens(fb))
            messages.append({"role": "assistant", "content": text or "(empty reply)"})
            messages.append({"role": "user", "content":
                             f"{note}Verification output after applying your files:\n{fb}"})
        if rec["outcome"] == "fail" and rec["stop_reasons"] and rec["stop_reasons"][-1] == "max_tokens":
            rec["error"] = "last reply hit max_tokens"
    except ModelCallError as e:
        rec["outcome"], rec["error"] = "error", str(e)
    except HarnessError as e:
        rec["outcome"], rec["error"] = "error", str(e)
    finally:
        shutil.rmtree(work, ignore_errors=True)
    rec["wall_seconds"] = round(time.monotonic() - start, 2)
    rec["cost_usd"] = cost_usd(args.model, rec["input_tokens"], rec["output_tokens"],
                               rec["cache_read_tokens"], rec["cache_creation_tokens"])
    return rec


# --------------------------------------------------------------------------
# CLI
# --------------------------------------------------------------------------

def parse_args(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--cases", default="eval/cases", help="cases directory")
    ap.add_argument("--case", nargs="+", default=None, help="case ids (default: all)")
    ap.add_argument("--modes", default="raw,orq", help="comma list of raw,orq")
    ap.add_argument("--max-turns", type=int, default=4)
    ap.add_argument("--budget", type=int, default=1500, help="orq report token budget")
    ap.add_argument("--model", default=DEFAULT_MODEL)
    ap.add_argument("--effort", default="medium", choices=["low", "medium", "high", "xhigh", "max"])
    ap.add_argument("--no-fallbacks", action="store_true",
                    help="do not request server-side refusal fallbacks")
    ap.add_argument("--orq", default="orq", help="path to the orq binary")
    ap.add_argument("--out", default="eval/results/agent")
    ap.add_argument("--mock", action="store_true", help="fake client, no network")
    ap.add_argument("--repeat", type=int, default=1)
    return ap.parse_args(argv)


def main(argv=None) -> int:
    args = parse_args(argv)
    modes = [m.strip() for m in args.modes.split(",") if m.strip()]
    if not modes or any(m not in ("raw", "orq") for m in modes):
        print("error: --modes must be a comma list of raw,orq", file=sys.stderr)
        return 2
    if "orq" in modes and shutil.which(args.orq) is None and not Path(args.orq).exists():
        print(f"error: orq binary not found: {args.orq} (use --orq PATH)", file=sys.stderr)
        return 2
    cases_dir = Path(args.cases)
    ids = args.case or sorted(p.name for p in cases_dir.iterdir() if (p / "case.toml").is_file())
    cases = []
    for cid in ids:
        if not (cases_dir / cid / "case.toml").is_file():
            print(f"error: unknown case {cid}", file=sys.stderr)
            return 2
        cases.append(load_case(cases_dir / cid))

    client = None
    if not args.mock:
        if anthropic is None:
            print("error: `pip install anthropic` (see eval/agent/requirements.txt)", file=sys.stderr)
            return 2
        if not os.environ.get("ANTHROPIC_API_KEY"):
            print("error: ANTHROPIC_API_KEY is not set (use --mock to run offline)", file=sys.stderr)
            return 2
        client = anthropic.Anthropic(max_retries=5)

    stamp = dt.datetime.now(dt.timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    run_id = f"{stamp}-{'mock' if args.mock else args.model}"
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    jsonl = out / f"{run_id}.jsonl"

    records = []
    with jsonl.open("w", encoding="utf-8") as f:
        for case in cases:
            for mode in modes:
                for rep in range(1, args.repeat + 1):
                    rec = run_one(case, mode, rep, args, client, run_id)
                    records.append(rec)
                    f.write(json.dumps(rec) + "\n")
                    f.flush()
                    print(f"{case['id']:28} {mode:4} #{rep} {rec['outcome']:8} "
                          f"turns={rec['turns']} tokens={rec['input_tokens'] + rec['output_tokens']}",
                          file=sys.stderr)
    desc = (f"max_turns={args.max_turns}, budget={args.budget}, effort={args.effort}, "
            f"repeat={args.repeat}, mock={args.mock}")
    md = render_markdown(run_id, args.model, desc, records)
    (out / f"{run_id}.md").write_text(md, encoding="utf-8")
    print(md)
    print(f"wrote {jsonl} and {out / (run_id + '.md')}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
