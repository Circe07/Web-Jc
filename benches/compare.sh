#!/usr/bin/env bash
# Compares `cargo build && cargo test` against `orq verify` on a project.
#
# Usage: benches/compare.sh <project-dir> [N] [orq-binary]
#
#   <project-dir>  a clean git work tree containing a Cargo project
#   N              number of synthetic changes (default 3)
#   orq-binary     path to the orq binary (default: target/release/orq of this
#                  repo, else `orq` from PATH)
#
# For each change i in 1..N the script appends a comment line to the i-th
# source file (sorted `git ls-files '*.rs'`, preferring files under src/), then
#   1. times `cargo build && cargo test` (output captured), then reverts;
#   2. re-applies the same change and times `orq verify --no-daemon`;
#   3. repeats `orq verify --no-daemon` with no further change (cache hit).
# Both modes are warmed up first, so the comparison is warm-vs-warm: the
# dependency graph is already compiled and only the incremental work after one
# edit is measured. Tokens are estimated as bytes/4 (same rule as orq).
# The project is restored with `git checkout -- .` on exit or error.
#
# Prints a markdown table to stdout; progress goes to stderr.

set -euo pipefail

if [ $# -lt 1 ]; then
  echo "usage: $0 <project-dir> [N] [orq-binary]" >&2
  exit 2
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT="$(cd "$1" && pwd)"
N="${2:-3}"
case "$N" in '' | *[!0-9]* | 0) echo "N must be a positive integer" >&2; exit 2 ;; esac

if [ -n "${3:-}" ]; then
  ORQ="$3"
elif [ -x "$SCRIPT_DIR/../target/release/orq" ]; then
  ORQ="$SCRIPT_DIR/../target/release/orq"
else
  ORQ="$(command -v orq || true)"
fi
if [ -z "$ORQ" ] || [ ! -x "$ORQ" ]; then
  echo "orq binary not found; build it with: cargo build --release -p orq-cli" >&2
  exit 2
fi
ORQ="$(cd "$(dirname "$ORQ")" && pwd)/$(basename "$ORQ")"

cd "$PROJECT"
if ! git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  echo "$PROJECT is not a git work tree" >&2
  exit 2
fi
if [ -n "$(git status --porcelain)" ]; then
  echo "$PROJECT has uncommitted changes; refusing to run" >&2
  exit 2
fi

WORK="$(mktemp -d)"
restore() {
  git -C "$PROJECT" checkout -- . >/dev/null 2>&1 || true
  rm -rf "$WORK"
}
trap restore EXIT

# Milliseconds since the epoch. Prefers bash 5 $EPOCHREALTIME (Linux and
# macOS with a recent bash), then python3, then GNU date.
if [ -n "${EPOCHREALTIME:-}" ]; then
  now_ms() {
    local t="${EPOCHREALTIME/[.,]/}" # microseconds
    echo $((t / 1000))
  }
elif command -v python3 >/dev/null 2>&1; then
  now_ms() { python3 -c 'import time; print(int(time.time() * 1000))'; }
else
  now_ms() { echo $(($(date +%s%N) / 1000000)); }
fi

# Candidate files: sorted tracked .rs files, preferring those under src/.
FILES=()
while IFS= read -r f; do FILES+=("$f"); done < <(git ls-files '*.rs' | LC_ALL=C sort | grep -E '(^|/)src/' || true)
if [ "${#FILES[@]}" -eq 0 ]; then
  while IFS= read -r f; do FILES+=("$f"); done < <(git ls-files '*.rs' | LC_ALL=C sort)
fi
if [ "${#FILES[@]}" -eq 0 ]; then
  echo "no tracked .rs files in $PROJECT" >&2
  exit 2
fi

# apply_change <index> <tag>: appends a harmless comment to the index-th file.
apply_change() {
  local idx=$1 tag=$2 file="${FILES[$(($1 % ${#FILES[@]}))]}"
  printf '\n// orq-bench change %s\n' "$tag" >>"$PROJECT/$file"
  echo "$file"
}

revert() { git -C "$PROJECT" checkout -- . >/dev/null 2>&1; }

# run_timed <outfile> <cmd...>: runs the command capturing stdout+stderr into
# outfile, ignoring its exit status; echoes "<ms> <bytes> <tokens>".
run_timed() {
  local out=$1
  shift
  local t0 t1 bytes
  t0=$(now_ms)
  "$@" >"$out" 2>&1 || true
  t1=$(now_ms)
  bytes=$(wc -c <"$out" | tr -d ' ')
  echo "$((t1 - t0)) $bytes $((bytes / 4))"
}

base_cmd() { cargo build && cargo test; }

echo "warm-up: cargo build && cargo test" >&2
apply_change 0 "warmup-$$" >/dev/null
run_timed "$WORK/out" base_cmd >/dev/null
echo "warm-up: orq verify" >&2
run_timed "$WORK/out" "$ORQ" verify --no-daemon >/dev/null
revert
run_timed "$WORK/out" base_cmd >/dev/null # rebuild the unmodified state

BASE=() ORQV=() HIT=()
for i in $(seq 1 "$N"); do
  tag="$$-$i-$(now_ms)"
  idx=$((i - 1))

  file=$(apply_change "$idx" "$tag")
  echo "[$i/$N] $file: cargo build && cargo test" >&2
  BASE+=("$(run_timed "$WORK/base-$i" base_cmd)")
  revert

  apply_change "$idx" "$tag" >/dev/null
  echo "[$i/$N] $file: orq verify" >&2
  ORQV+=("$(run_timed "$WORK/orq-$i" "$ORQ" verify --no-daemon)")
  echo "[$i/$N] $file: orq verify (repeat, cache hit)" >&2
  HIT+=("$(run_timed "$WORK/hit-$i" "$ORQ" verify --no-daemon)")
  revert
done

# column <field> <values...>: prints "mean median" of the given 1-based field.
column() {
  local field=$1
  shift
  printf '%s\n' "$@" | awk -v f="$field" '
    { v[NR] = $f; s += $f }
    END {
      n = NR
      for (i = 1; i <= n; i++) for (j = i + 1; j <= n; j++) if (v[j] < v[i]) { t = v[i]; v[i] = v[j]; v[j] = t }
      med = (n % 2) ? v[(n + 1) / 2] : (v[n / 2] + v[n / 2 + 1]) / 2
      printf "%.0f %.0f", s / n, med
    }'
}

row() {
  local name=$1
  shift
  local t b k
  t=$(column 1 "$@")
  b=$(column 2 "$@")
  k=$(column 3 "$@")
  set -- $t $b $k
  echo "| $name | $1 | $2 | $3 | $4 | $5 | $6 |"
}

echo "Project: \`$(basename "$PROJECT")\` ($(git rev-parse --short HEAD)), N=$N synthetic changes, warm-vs-warm"
echo "(both modes were built once before measuring; each change appends one comment line to a source file)."
echo
echo "| Mode | time mean (ms) | time median (ms) | output mean (bytes) | output median (bytes) | tokens mean | tokens median |"
echo "|---|---:|---:|---:|---:|---:|---:|"
row "cargo build && cargo test" "${BASE[@]}"
row "orq verify" "${ORQV[@]}"
row "orq verify (repeat, cache hit)" "${HIT[@]}"
