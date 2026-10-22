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
- **Cache hit works on `multi`** (about 3x faster than the baseline) **but did
  not hit on ripgrep**: the repeated run recomputed (`cache_hit: false` in
  `target/orq/stats.jsonl`, ~3.5 s). This needs investigation in the
  runner/cache layer; the benchmark only exposes it.
- Not measured: daemon mode, `--up-to check`, and nextest.
