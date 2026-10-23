#!/usr/bin/env python3
"""Edit → verify loop timing on a test project, without any model.

What an agent waits for after each edit is the verification: this measures
it for several build configurations and checkers, on a project whose
dependencies are already built (a developer's machine, not a cold build).

For every configuration (env overrides and extra cargo arguments, such as a
profile, a linker or a compiler wrapper); a configuration whose tool is not
installed is reported as "not installed" instead of failing:
  1. copy the project to a temp dir and build it once (untimed);
  2. for each edit kind, N times: apply a small real edit (a new public
     function in a leaf or a root crate), then time each checker.

Checkers:
  cargo   `cargo clippy --all-targets -- -D warnings` then `cargo test`
          (what an agent without orq runs to know it is done)
  nextest `cargo clippy --all-targets -- -D warnings` then `cargo nextest run`
          (the same lints with the community test runner; needs cargo-nextest)
  orq     `orq verify --no-daemon --no-cache --up-to test`
          (shown as "n/a" for a configuration orq cannot express)
  orq-daemon
          `orq verify --up-to test` with the daemon running, asked --think
          seconds after the edit (untimed), as when the agent's model is still
          writing its reply; runs on its own copy and target dir so the
          daemon never competes with the other checkers

Usage:
  python3 benches/edit_loop.py --project eval/projects/tasktrack \\
      --orq target/release/orq --iterations 5 --config default --config debug0

Numbers are only meaningful on an idle machine: run nothing else meanwhile.
"""
from __future__ import annotations

import argparse
import json
import os
import shutil
import statistics
import subprocess
import sys
import tempfile
import time
from pathlib import Path

CHECKERS = ("cargo", "nextest", "orq", "orq-daemon")

# Named build configurations. "env" overrides the caller's environment;
# "cargo_args" go right after `cargo` (global flags such as --config, which
# env vars cannot express for `profile.dev.package."*"`); "tool" is the
# program that must be on PATH; "orq": False skips orq when it has no way to
# pass the configuration through.
CONFIGS: dict[str, dict] = {
    "default": {},
    "line-tables": {"env": {"CARGO_PROFILE_DEV_DEBUG": "line-tables-only"}},
    "debug0": {"env": {"CARGO_PROFILE_DEV_DEBUG": "0"}},
    "no-incremental": {"env": {"CARGO_INCREMENTAL": "0"}},
    "deps-opt1": {"cargo_args": ["--config", 'profile.dev.package."*".opt-level=1'],
                  "orq": False},
    "sccache": {"env": {"RUSTC_WRAPPER": "sccache"}, "tool": "sccache"},
    "wild": {"env": {"RUSTFLAGS": "-Clink-arg=--ld-path=wild", "CC": "clang",
                     "CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER": "clang"},
             "tool": "wild"},
    "mold": {"env": {"RUSTFLAGS": "-Clink-arg=-fuse-ld=mold", "CC": "clang",
                     "CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER": "clang"},
             "tool": "mold"},
}

# Where each edit kind lands: a leaf crate (nothing depends on it) and the
# root crate (every other crate depends on it).
EDITS = {
    "leaf": "crates/cli/src/render.rs",
    "root": "crates/model/src/lib.rs",
}


def run(cmd: list[str], cwd: Path, env: dict) -> tuple[float, int]:
    start = time.monotonic()
    try:
        p = subprocess.run(cmd, cwd=cwd, env=env, stdout=subprocess.DEVNULL,
                           stderr=subprocess.DEVNULL)
    except OSError:
        return time.monotonic() - start, 127
    return time.monotonic() - start, p.returncode


def has_nextest() -> bool:
    return run(["cargo", "nextest", "--version"], Path.cwd(), dict(os.environ))[1] == 0


def cargo_check(project: Path, env: dict, cargo_args: list[str],
                tests: str = "cargo") -> tuple[float, int]:
    """Lints then tests; `tests` picks the runner (`cargo` or `nextest`)."""
    t1, rc = run(["cargo", *cargo_args, "clippy", "--all-targets", "--", "-D", "warnings"],
                 project, env)
    if rc != 0:
        return t1, rc
    test_cmd = ["test", "--quiet"] if tests == "cargo" else ["nextest", "run"]
    t2, rc = run(["cargo", *cargo_args, *test_cmd], project, env)
    return t1 + t2, rc


def orq_check(orq: str, project: Path, env: dict, changed: str | None = None) -> tuple[float, int]:
    """`orq verify` told which file changed, as the Claude Code hook does.
    Without `changed` (the warm-up), every source file counts as changed."""
    files = changed or ",".join(str(p.relative_to(project)) for p in project.rglob("*.rs")
                                if "target" not in p.parts)
    return run([orq, "verify", "--no-daemon", "--no-cache", "--up-to", "test",
                "--files", files], project, env)


def apply_edit(project: Path, rel: str, n: int) -> None:
    """Adds a public function: a real API change, so dependents rebuild.

    It goes before the file's `#[cfg(test)]` module, if any: an item after
    the test module trips `clippy::items_after_test_module`.
    """
    path = project / rel
    text = path.read_text(encoding="utf-8")
    item = f"/// Added by the edit-loop bench.\npub fn bench_edit_{n}() -> u32 {{\n    {n}\n}}\n\n"
    at = text.find("#[cfg(test)]")
    text = text[:at] + item + text[at:] if at >= 0 else text + "\n" + item
    path.write_text(text, encoding="utf-8")


def daemon_times(args, env: dict, cargo_args: list[str]) -> dict[str, float]:
    """Median `orq verify` latency per edit kind with a warm daemon."""
    work = Path(tempfile.mkdtemp(prefix="edit-loop-daemon-"))
    env = {k: v for k, v in env.items() if k != "ORQ_NO_DAEMON"}
    project = work / "project"
    try:
        shutil.copytree(args.project, project, copy_function=shutil.copy,
                        ignore=shutil.ignore_patterns("target", "tasks"))
        (project / "orq.toml").write_text("deny_warnings = true\n", encoding="utf-8")
        env["CARGO_TARGET_DIR"] = str(work / "target")
        # A git repository, as real projects are: the daemon's speculative
        # results are keyed by the changes git reports.
        for cmd in (["git", "init", "-q"], ["git", "add", "-A"],
                    ["git", "-c", "user.name=bench", "-c", "user.email=bench@example.com",
                     "commit", "-qm", "base"]):
            run(cmd, project, env)
        cargo_check(project, env, cargo_args)
        orq_check(args.orq, project, env)
        run([args.orq, "daemon", "start"], project, env)
        time.sleep(args.think)
        out = {}
        n = 10_000
        for kind, rel in EDITS.items():
            times = []
            for _ in range(args.iterations):
                n += 1
                apply_edit(project, rel, n)
                time.sleep(args.think)
                secs, rc = run([args.orq, "verify", "--up-to", "test", "--files", rel],
                               project, env)
                if rc != 0:
                    return {kind: f"failed after a {kind} edit" for kind in EDITS}
                times.append(secs)
            out[kind] = round(statistics.median(times), 2)
        return out
    finally:
        run([args.orq, "daemon", "stop"], project, env)
        shutil.rmtree(work, ignore_errors=True)


def bench_config(name: str, args, nextest: bool) -> dict:
    conf = CONFIGS[name]
    tool = conf.get("tool")
    if tool and shutil.which(tool) is None:
        return {"config": name, "error": f"not installed ({tool})"}
    cargo_args = conf.get("cargo_args", [])
    use_orq = conf.get("orq", True)
    wanted = args.checker or CHECKERS
    checkers = [c for c in wanted if c != "orq-daemon"
                and (c != "nextest" or nextest) and (c != "orq" or use_orq)]
    env = dict(os.environ)
    env.update(conf.get("env", {}))
    env.update({"CARGO_TERM_COLOR": "never", "RUST_BACKTRACE": "0", "ORQ_NO_DAEMON": "1"})
    work = Path(tempfile.mkdtemp(prefix=f"edit-loop-{name}-"))
    try:
        project = work / "project"
        shutil.copytree(args.project, project, copy_function=shutil.copy,
                        ignore=shutil.ignore_patterns("target", "tasks"))
        (project / "orq.toml").write_text("deny_warnings = true\n", encoding="utf-8")
        env["CARGO_TARGET_DIR"] = str(work / "target")
        t0 = time.monotonic()
        _, rc = cargo_check(project, env, cargo_args)
        cold = time.monotonic() - t0
        if rc != 0:
            return {"config": name, "error": "the cold build failed (tool missing?)"}
        if "nextest" in checkers:
            cargo_check(project, env, cargo_args, "nextest")  # warm nextest's runs
        if "orq" in checkers:
            orq_check(args.orq, project, env)  # warm orq's own runs too
        out = {"config": name, "cold_seconds": round(cold, 1), "edits": {}}
        n = 0
        for kind, rel in EDITS.items():
            times: dict[str, list[float]] = {c: [] for c in checkers}
            for _ in range(args.iterations):
                for checker in checkers:
                    n += 1
                    apply_edit(project, rel, n)
                    secs, rc = (orq_check(args.orq, project, env, rel) if checker == "orq"
                                else cargo_check(project, env, cargo_args, checker))
                    if rc != 0:
                        return {"config": name, "error": f"{checker} failed after a {kind} edit"}
                    times[checker].append(secs)
            out["edits"][kind] = {c: round(statistics.median(v), 2) for c, v in times.items()}
        if "orq-daemon" in wanted and use_orq and not cargo_args:
            for kind, secs in daemon_times(args, env, cargo_args).items():
                out["edits"][kind]["orq-daemon"] = secs
        for kind in EDITS:
            for c in CHECKERS:  # checkers that did not run: say why
                if c not in out["edits"][kind]:
                    out["edits"][kind][c] = (
                        "not installed" if c == "nextest" and not nextest
                        else "n/a" if c == "orq" and not use_orq else "-")
        return out
    finally:
        shutil.rmtree(work, ignore_errors=True)


def cell(value) -> str:
    return f"{value} s" if isinstance(value, (int, float)) else str(value)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--project", default="eval/projects/tasktrack", type=Path)
    ap.add_argument("--orq", default="target/release/orq")
    ap.add_argument("--iterations", type=int, default=5)
    ap.add_argument("--think", type=float, default=3.0,
                    help="seconds between an edit and the daemon verify (untimed)")
    ap.add_argument("--config", action="append", choices=sorted(CONFIGS),
                    help="configuration to measure (repeatable; default: default)")
    ap.add_argument("--checker", action="append", choices=CHECKERS,
                    help="checker to time (repeatable; default: all)")
    ap.add_argument("--json", type=Path, help="also write the results as JSON")
    args = ap.parse_args()
    args.orq = str(Path(args.orq).resolve()) if Path(args.orq).exists() else args.orq
    nextest = has_nextest()
    results = []
    for name in args.config or ["default"]:
        print(f"measuring {name} ...", file=sys.stderr)
        results.append(bench_config(name, args, nextest))
    cols = [f"{kind} edit: {c}" for kind in EDITS for c in CHECKERS]
    print("| config | cold build | " + " | ".join(cols) + " |")
    print("|---|---:|" + "---:|" * len(cols))
    for r in results:
        if "error" in r:
            print(f"| {r['config']} | {r['error']} |" + " |" * len(cols))
            continue
        cells = [cell(r["edits"][kind][c]) for kind in EDITS for c in CHECKERS]
        print(f"| {r['config']} | {r['cold_seconds']} s | " + " | ".join(cells) + " |")
    if args.json:
        args.json.write_text(json.dumps(results, indent=2), encoding="utf-8")
    return 0


if __name__ == "__main__":
    sys.exit(main())
