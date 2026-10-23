#!/usr/bin/env python3
"""Edit → verify loop timing on a test project, without any model.

What an agent waits for after each edit is the verification: this measures
it for several build configurations and checkers, on a project whose
dependencies are already built (a developer's machine, not a cold build).

For every configuration (env overrides such as a profile or a linker):
  1. copy the project to a temp dir and build it once (untimed);
  2. for each edit kind, N times: apply a small real edit (a new public
     function in a leaf or a root crate), then time each checker.

Checkers:
  cargo   `cargo clippy --all-targets -- -D warnings` then `cargo test`
          (what an agent without orq runs to know it is done)
  orq     `orq verify --no-daemon --no-cache --up-to test`

Usage:
  python3 benches/edit_loop.py --project eval/projects/tasktrack \\
      --orq target/release/orq --iterations 5 --config default --config debug0
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

# Named build configurations: environment overrides on top of the caller's.
CONFIGS: dict[str, dict[str, str]] = {
    "default": {},
    "line-tables": {"CARGO_PROFILE_DEV_DEBUG": "line-tables-only"},
    "debug0": {"CARGO_PROFILE_DEV_DEBUG": "0"},
    "no-incremental": {"CARGO_INCREMENTAL": "0"},
    "sccache": {"RUSTC_WRAPPER": "sccache"},
    "wild": {"RUSTFLAGS": "-Clink-arg=--ld-path=wild", "CC": "clang",
             "CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER": "clang"},
    "mold": {"RUSTFLAGS": "-Clink-arg=-fuse-ld=mold", "CC": "clang",
             "CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER": "clang"},
}

# Where each edit kind lands: a leaf crate (nothing depends on it) and the
# root crate (every other crate depends on it).
EDITS = {
    "leaf": "crates/cli/src/render.rs",
    "root": "crates/model/src/lib.rs",
}


def run(cmd: list[str], cwd: Path, env: dict) -> tuple[float, int]:
    start = time.monotonic()
    p = subprocess.run(cmd, cwd=cwd, env=env, stdout=subprocess.DEVNULL,
                       stderr=subprocess.DEVNULL)
    return time.monotonic() - start, p.returncode


def cargo_check(project: Path, env: dict) -> tuple[float, int]:
    t1, rc = run(["cargo", "clippy", "--all-targets", "--", "-D", "warnings"], project, env)
    if rc != 0:
        return t1, rc
    t2, rc = run(["cargo", "test", "--quiet"], project, env)
    return t1 + t2, rc


def orq_check(orq: str, project: Path, env: dict) -> tuple[float, int]:
    files = ",".join(str(p.relative_to(project)) for p in project.rglob("*.rs")
                     if "target" not in p.parts)
    return run([orq, "verify", "--no-daemon", "--no-cache", "--up-to", "test",
                "--files", files], project, env)


def apply_edit(project: Path, rel: str, n: int) -> None:
    """Adds a public function: a real API change, so dependents rebuild."""
    path = project / rel
    with path.open("a", encoding="utf-8") as f:
        f.write(f"\n/// Added by the edit-loop bench.\npub fn bench_edit_{n}() -> u32 {{\n    {n}\n}}\n")


def bench_config(name: str, args) -> dict:
    env = dict(os.environ)
    env.update(CONFIGS[name])
    env.update({"CARGO_TERM_COLOR": "never", "RUST_BACKTRACE": "0", "ORQ_NO_DAEMON": "1"})
    work = Path(tempfile.mkdtemp(prefix=f"edit-loop-{name}-"))
    try:
        project = work / "project"
        shutil.copytree(args.project, project, copy_function=shutil.copy,
                        ignore=shutil.ignore_patterns("target", "tasks"))
        (project / "orq.toml").write_text("deny_warnings = true\n", encoding="utf-8")
        env["CARGO_TARGET_DIR"] = str(work / "target")
        t0 = time.monotonic()
        _, rc = cargo_check(project, env)
        cold = time.monotonic() - t0
        if rc != 0:
            return {"config": name, "error": "the cold build failed (tool missing?)"}
        orq_check(args.orq, project, env)  # warm orq's own runs too
        out = {"config": name, "cold_seconds": round(cold, 1), "edits": {}}
        n = 0
        for kind, rel in EDITS.items():
            times: dict[str, list[float]] = {"cargo": [], "orq": []}
            for _ in range(args.iterations):
                for checker in ("cargo", "orq"):
                    n += 1
                    apply_edit(project, rel, n)
                    secs, rc = (cargo_check(project, env) if checker == "cargo"
                                else orq_check(args.orq, project, env))
                    if rc != 0:
                        return {"config": name, "error": f"{checker} failed after a {kind} edit"}
                    times[checker].append(secs)
            out["edits"][kind] = {c: round(statistics.median(v), 2) for c, v in times.items()}
        return out
    finally:
        shutil.rmtree(work, ignore_errors=True)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--project", default="eval/projects/tasktrack", type=Path)
    ap.add_argument("--orq", default="target/release/orq")
    ap.add_argument("--iterations", type=int, default=5)
    ap.add_argument("--config", action="append", choices=sorted(CONFIGS),
                    help="configuration to measure (repeatable; default: default)")
    ap.add_argument("--json", type=Path, help="also write the results as JSON")
    args = ap.parse_args()
    args.orq = str(Path(args.orq).resolve()) if Path(args.orq).exists() else args.orq
    results = []
    for name in args.config or ["default"]:
        print(f"measuring {name} ...", file=sys.stderr)
        results.append(bench_config(name, args))
    print("| config | cold build | leaf edit: cargo | leaf edit: orq | root edit: cargo | root edit: orq |")
    print("|---|---:|---:|---:|---:|---:|")
    for r in results:
        if "error" in r:
            print(f"| {r['config']} | {r['error']} | | | | |")
            continue
        e = r["edits"]
        print(f"| {r['config']} | {r['cold_seconds']} s | {e['leaf']['cargo']} s | "
              f"{e['leaf']['orq']} s | {e['root']['cargo']} s | {e['root']['orq']} s |")
    if args.json:
        args.json.write_text(json.dumps(results, indent=2), encoding="utf-8")
    return 0


if __name__ == "__main__":
    sys.exit(main())
