# Benchmarks: `cargo build && cargo test` vs `orq verify`

`benches/compare.sh` applies N synthetic changes (one comment line appended to
the i-th tracked `.rs` file, preferring `src/`) to a clean git work tree and
compares, per change:

1. `cargo build && cargo test` (all output captured), then reverts;
2. `orq verify --no-daemon` after re-applying the same change;
3. a repeated `orq verify --no-daemon` with no further change (cache hit).

Both modes are warmed up first, so every number is warm-vs-warm (dependencies
already compiled; only the incremental work after one edit is measured).
Output size is the combined stdout and stderr in bytes; tokens are estimated
as bytes / 4. The script restores the project with `git checkout -- .` on exit.

## Machine

- Date: 2026-10-22
- Linux 6.18 (Firecracker VM), 4 vCPU Intel Xeon @ 2.10 GHz, 15 GiB RAM
- rustc 1.97.0, `orq` built with `cargo build --release -p orq-cli`
- Cold, noisy cloud VM: treat absolute times as indicative only.

## Commands

```sh
cargo build --release -p orq-cli

# fixtures/multi, copied to a scratch dir and git-initialised
cp -r fixtures/multi /tmp/orq-bench/multi && rm -rf /tmp/orq-bench/multi/target
echo 'target/' > /tmp/orq-bench/multi/.gitignore
(cd /tmp/orq-bench/multi && git init -q && git add -A && git commit -qm init)
benches/compare.sh /tmp/orq-bench/multi 5

# a real, medium-sized workspace (ripgrep, 10 crates), cloned outside the repo
git clone --depth 1 https://github.com/BurntSushi/ripgrep.git /tmp/orq-bench/rg
benches/compare.sh /tmp/orq-bench/rg 3
```

## Results

### `fixtures/multi` (4 small crates), N=5

| Mode | time mean (ms) | time median (ms) | output mean (bytes) | output median (bytes) | tokens mean | tokens median |
|---|---:|---:|---:|---:|---:|---:|
| cargo build && cargo test | 407 | 404 | 2346 | 2491 | 586 | 622 |
| orq verify | 827 | 742 | 65 | 63 | 16 | 15 |
| orq verify (repeat, cache hit) | 143 | 111 | 66 | 64 | 16 | 16 |

### ripgrep (`3fce3b5`, 10 crates), N=3

| Mode | time mean (ms) | time median (ms) | output mean (bytes) | output median (bytes) | tokens mean | tokens median |
|---|---:|---:|---:|---:|---:|---:|
| cargo build && cargo test | 4418 | 4358 | 18896 | 18896 | 4724 | 4724 |
| orq verify | 6574 | 5969 | 335 | 335 | 83 | 83 |
| orq verify (repeat, cache hit) | 3530 | 3596 | 335 | 335 | 83 | 83 |

## Reading the numbers

- **Output size is the clear win**: the agent sees about 40x (multi) to 56x
  (ripgrep) fewer tokens than the raw cargo output.
- **Wall time is not a win yet** in this setup. `orq verify` runs the whole
  ladder (fmt, check, clippy, test), which does more work than
  `cargo build && cargo test`, so a cold-ish single edit costs 1.4x to 2x
  more. The time benefit is expected from the daemon and the result cache, not
  from `--no-daemon`.
- **Cache hit works on `multi`**, about 3x faster than the baseline. On
  ripgrep the repeated run above still recomputed (~3.5 s). The cause was a
  tracked symlink to a directory (`HomebrewFormula`): reading it for the cache
  key failed with `EISDIR`, and that silently disabled the cache. Fixed in
  `orq-cache`: symlinks are now hashed by their target and directories are
  skipped. After the fix, the repeated `orq verify --up-to check` on ripgrep is
  a cache hit in 125–171 ms (release build). The ripgrep table above was
  measured before the fix.
- Not measured: daemon mode, `--up-to check`, and nextest (see "Edit loop" below for nextest).

## Edit loop (wave 8)

`benches/edit_loop.py` measures what an agent waits for after each edit, with
no model involved. For every build configuration it copies the test project
(`eval/projects/tasktrack`, 4 crates with real dependencies) to a temp dir,
builds it once (untimed, reported as "cold build"), then applies a small real
edit N times and times each checker. Two edit kinds: a **leaf** edit (a crate
nothing depends on) and a **root** edit (a crate every other crate depends on).
Cells are medians in seconds.

```
cargo build --release -p orq-cli
python3 benches/edit_loop.py --iterations 5 \
    --config default --config line-tables --config deps-opt1 \
    --config sccache --config wild --json edit-loop.json
```

`--config` and `--checker` are repeatable; the default is `default` with all
checkers.

**Checkers**

- `cargo`: `cargo clippy --all-targets -- -D warnings`, then `cargo test`. What
  an agent without orq runs.
- `nextest`: the same clippy, then `cargo nextest run`. Isolates the effect of
  the test runner. Needs `cargo-nextest`; otherwise the cell says "not installed".
- `orq`: `orq verify --no-daemon --no-cache --up-to test`, so it does the same
  work as the others (no daemon, no result cache).

**Configurations** (a missing tool is reported as "not installed", never a crash)

| Config | What it changes |
| --- | --- |
| `default` | Nothing: the baseline. |
| `line-tables` | `CARGO_PROFILE_DEV_DEBUG=line-tables-only`: smaller debug info. |
| `debug0` | `CARGO_PROFILE_DEV_DEBUG=0`: no debug info. |
| `no-incremental` | `CARGO_INCREMENTAL=0`. |
| `deps-opt1` | Dependencies at `opt-level = 1`, through `cargo --config 'profile.dev.package."*".opt-level=1'` (env vars cannot express the `"*"` key). `orq verify` has no way to pass cargo arguments, so orq is not measured here and its cells say "n/a". |
| `sccache` | `RUSTC_WRAPPER=sccache`. |
| `wild` | Links with the `wild` linker through clang (`--ld-path=wild`). |
| `mold` | Links with `mold` through clang (`-fuse-ld=mold`). |

**Take the real numbers on an idle machine.** Other builds running at the same
time (another agent, an editor running rust-analyzer, a CI job) change every
timing, so a run on a busy machine is only a smoke test of the script. Use
`--iterations 1` for that, and do not record its numbers here.
