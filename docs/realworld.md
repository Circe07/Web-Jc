# Real-world runs

`benches/realworld.sh <workdir> [orq-binary]` clones pinned real projects and
drives `orq verify --no-daemon --up-to check` through four steps:

- a: clean tree (`--files Cargo.toml`, because a bare verify of a clean tree is
  a no-op and would measure nothing);
- b1: append a comment to a leaf-crate `.rs` file;
- b2: repeat b1, which must be a cache hit;
- c: append `fn __orq_broken() -> u32 { "x" }`, which must exit 1 with `E0308`.

The nightly workflow `.github/workflows/realworld.yml` (ubuntu, schedule and
`workflow_dispatch` only, never on PRs) builds orq in release, runs the script,
publishes the table as the job summary and uploads it plus the logs as an
artifact.

## Run of 2026-10-23

orq 0.3.0 (commit 24e644b plus T5.5), rustc 1.97.0, Linux x86_64, release build.

| project | exit a/b1/b2/c | times ms a/b1/b2/c | b2 cache hit | tokens a/b1/c | E0308 in c |
|---|---|---|---|---|---|
| ripgrep 14.1.1 | 1 / 0 / 0 / 1 | 22607 / 5898 / 198 / 1062 | yes | 49 / 13 / 76 | yes |
| serde v1.0.219 | 1 / 1 / 1 / 1 | 23427 / 563 / 124 / 566 | no | 1484 / 60 / 83 | no |
| clap v4.5.41 | 0 / 0 / 0 / 1 | 85448 / 5634 / 193 / 600 | yes | 14 / 14 / 108 | yes |
| tokio 1.47.1 | 0 / 1 / 1 / 1 | 79028 / 833 / 111 / 932 | no | 14 / 68 / 91 | no |

No project returned exit 2 and no crash occurred. ripgrep and clap behave as
expected. serde and tokio expose bug 1 below.

## Bugs found

### 1. `-p <name>` is ambiguous when the lockfile holds two versions of a workspace member (orq bug)

Affects serde and tokio, any workspace whose members are also pulled from
crates.io by some dev-dependency (trybuild for serde, a bench or test
dependency for tokio). Every incremental verify fails, so steps b and c are
unusable and the type error is never reported.

Reproduction:

```
git clone --depth 1 --branch tokio-1.47.1 https://github.com/tokio-rs/tokio.git
cd tokio
orq verify --no-daemon --up-to check --files Cargo.toml   # exit 0 (full check)
echo '// x' >> tokio-stream/src/lib.rs
orq verify --no-daemon --up-to check
```

Output (exit 1):

```
FAIL check (1 error) · crates: benches, examples, ... tokio, +4 · 0.8s
[1] `cargo check` failed: error: specification `tokio` is ambiguous
    help: re-run this command with one of the following specifications
    tokio@1.47.1 tokio@1.54.1
```

Same with serde v1.0.219: `specification `serde` is ambiguous (serde@1.0.219
serde@1.0.229)`, after editing `serde_derive/src/lib.rs`. Neither repository
commits a `Cargo.lock`, so the registry copy is resolved at the newest version.

Root cause guess: `crates/orq-runner/src/lib.rs:427` builds the argument list
with `-p <crate name>`. Use the package id spec instead (`-p name@version`, or
the `cargo metadata` pkgid, `path+file://...#name@version`), which is always
unambiguous. `orq_core` crate info would need the version, which `cargo
metadata` already provides. Also worth a regression test with a fixture whose
workspace member name collides with a registry dependency.

### 2. Diagnostics from registry dependencies are shown as the workspace's errors (serde, step a)

Running a whole-workspace check on serde reports `FAIL check (34 errors)` with
entries such as `E0433 cannot find __require_serde_not_serde_core in _serde —
.../registry/src/.../trybuild-1.0.122/src/cargo.rs:13:10`. This is upstream
breakage, not orq's: the workspace's serde 1.0.219 is resolved next to a newer
registry serde that trybuild uses, so the check fails in a dependency. orq
behaves correctly (exit 1, not 2), but a report that is 34 lines of errors in
files under `~/.cargo/registry` is poor for an agent. Suggestion: group
errors whose file is outside the root as "in dependency X" and summarise them
in one line, or drop them from the numbered list.

### 3. ripgrep step a exits 1 on stable (known, fixed by T5.1)

`crates/globset/benches/bench.rs` has `#![feature(test)]`, so `--all-targets`
fails with `E0554` on stable. The script accepts exit 1 for step a, so this
does not fail the run. After T5.1 lands it should become exit 0.

### 4. Minor: the unformatted warning on the injected line

Step c also prints `[W1] file is not formatted according to rustfmt` for the
injected `fn __orq_broken() ...` line. Expected given the script's injection,
not a bug.

## Notes

- The script uses `perl` for millisecond timing because BSD `date` has no `%N`
  on macOS; perl ships with both systems.
- The first run on a cold disk needs about 2 GB per large project; the
  disk-full failure seen in an earlier partial run was the machine, not orq.
- `STEP_TIMEOUT` (default 3600 s) bounds each verify when `timeout` or
  `gtimeout` exists.
