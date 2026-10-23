#!/usr/bin/env bash
# Runs `orq verify` against real-world Rust projects and prints a markdown table.
#
# Usage: benches/realworld.sh <workdir> [orq-binary]
#
#   <workdir>     scratch directory (outside this repo); projects are cloned
#                 into <workdir>/<name>, shallow, at the pinned ref below
#   orq-binary    path to orq (default: target/release/orq of this repo, else
#                 `orq` from PATH)
#
# Environment:
#   ONLY="ripgrep serde"   run only these projects (names from the table)
#   STEP_TIMEOUT=3600      per-`orq verify` timeout in seconds (when `timeout`
#                          or `gtimeout` exists)
#
# Per project:
#   (a) `orq verify --no-daemon --up-to check --files Cargo.toml` on a clean
#       tree (a bare verify of a clean tree is a no-op, naming the root
#       manifest forces a whole-workspace check); expect exit 0 (1 only if
#       upstream itself is broken), never 2;
#   (b) append a comment to one .rs file of a leaf crate, verify -> exit 0,
#       then verify again -> "(cache: yes)";
#   (c) append `fn __orq_broken() -> u32 { "x" }` -> exit 1 with E0308;
#   (d) restore with `git checkout -- .`.
#
# The script exits non-zero if any project returns exit code 2 or an
# unexpected result. The markdown table goes to stdout, progress and the
# per-step orq output (logs) go to stderr and <workdir>/logs/.
#
# Works on Linux and macOS (bash 3.2 compatible; no GNU-only flags).

set -euo pipefail

# name | git url | ref (tag) | file edited in step (b)/(c), relative to the root
# Keep refs pinned so a failure means orq changed, not upstream.
PROJECTS=(
  "ripgrep|https://github.com/BurntSushi/ripgrep.git|14.1.1|crates/core/main.rs"
  "serde|https://github.com/serde-rs/serde.git|v1.0.219|serde_derive/src/lib.rs"
  "clap|https://github.com/clap-rs/clap.git|v4.5.41|clap_mangen/src/lib.rs"
  "tokio|https://github.com/tokio-rs/tokio.git|tokio-1.47.1|tokio-stream/src/lib.rs"
)
# Notes: serde is a virtual-manifest workspace that also holds a proc-macro
# crate (serde_derive); tokio is a virtual-manifest workspace; clap has a
# root package plus workspace members; ripgrep is a root package with
# members under crates/.

if [ $# -lt 1 ]; then
  echo "usage: $0 <workdir> [orq-binary]" >&2
  exit 2
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
mkdir -p "$1"
WORKDIR="$(cd "$1" && pwd)"
LOGS="$WORKDIR/logs"
mkdir -p "$LOGS"

if [ -n "${2:-}" ]; then
  ORQ="$2"
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

TIMEOUT_CMD=""
if command -v timeout >/dev/null 2>&1; then
  TIMEOUT_CMD="timeout ${STEP_TIMEOUT:-3600}"
elif command -v gtimeout >/dev/null 2>&1; then
  TIMEOUT_CMD="gtimeout ${STEP_TIMEOUT:-3600}"
fi

now_ms() {
  perl -MTime::HiRes=time -e 'printf "%d\n", time()*1000'
}

# run_orq <log-file> [args...]: sets RC, MS, TOKENS, OUT_FILE.
run_orq() {
  local log="$1"
  shift
  local start end
  start="$(now_ms)"
  set +e
  # shellcheck disable=SC2086
  $TIMEOUT_CMD "$ORQ" verify --no-daemon --up-to check "$@" >"$log" 2>&1
  RC=$?
  set -e
  end="$(now_ms)"
  MS=$((end - start))
  local bytes
  bytes="$(wc -c <"$log" | tr -d ' ')"
  TOKENS=$((bytes / 4))
}

FAILED=0
ROWS=""
note() { echo "[realworld] $*" >&2; }
fail() {
  note "FAIL: $*"
  FAILED=1
  PROBLEMS="${PROBLEMS:-}$1: $2; "
}

for entry in "${PROJECTS[@]}"; do
  IFS='|' read -r NAME URL REF FILE <<<"$entry"
  if [ -n "${ONLY:-}" ] && ! echo " $ONLY " | grep -q " $NAME "; then
    continue
  fi
  DIR="$WORKDIR/$NAME"
  note "== $NAME ($REF)"
  if [ ! -d "$DIR/.git" ]; then
    git clone --quiet --depth 1 --branch "$REF" "$URL" "$DIR"
  fi
  cd "$DIR"
  git checkout --quiet -- . || true
  git clean -fdq -e target >/dev/null 2>&1 || true
  if [ ! -f "$FILE" ]; then
    fail "$NAME" "edit file $FILE missing"
    ROWS="$ROWS| $NAME | - | - | - | - | - |"$'\n'
    continue
  fi

  # (a) clean tree. With no changes `orq verify` is a no-op (PASS in ~0.2 s),
  # so name the root manifest as changed: that forces a whole-workspace check.
  run_orq "$LOGS/$NAME.a.log" --files Cargo.toml
  A_RC=$RC; A_MS=$MS; A_TOK=$TOKENS
  note "a: exit $A_RC in ${A_MS} ms"
  [ "$A_RC" -le 1 ] || fail "$NAME" "(a) exit $A_RC on a clean tree (see $LOGS/$NAME.a.log)"

  # (b) harmless edit, then repeat
  echo "// orq realworld touch" >>"$FILE"
  run_orq "$LOGS/$NAME.b1.log"
  B1_RC=$RC; B1_MS=$MS; B1_TOK=$TOKENS
  note "b1: exit $B1_RC in ${B1_MS} ms"
  if [ "$A_RC" -eq 0 ]; then
    [ "$B1_RC" -eq 0 ] || fail "$NAME" "(b) comment edit gave exit $B1_RC (see $LOGS/$NAME.b1.log)"
  else
    [ "$B1_RC" -le 1 ] || fail "$NAME" "(b) comment edit gave exit $B1_RC (see $LOGS/$NAME.b1.log)"
  fi
  run_orq "$LOGS/$NAME.b2.log"
  B2_RC=$RC; B2_MS=$MS
  CACHE="no"
  if grep -q "cache: yes" "$LOGS/$NAME.b2.log"; then CACHE="yes"; fi
  note "b2: exit $B2_RC in ${B2_MS} ms, cache hit: $CACHE"
  if [ "$B1_RC" -eq 0 ]; then
    [ "$B2_RC" -eq 0 ] || fail "$NAME" "(b) repeat gave exit $B2_RC (see $LOGS/$NAME.b2.log)"
    [ "$CACHE" = "yes" ] || fail "$NAME" "(b) repeat was not a cache hit (see $LOGS/$NAME.b2.log)"
  fi

  # (c) type error
  echo 'fn __orq_broken() -> u32 { "x" }' >>"$FILE"
  run_orq "$LOGS/$NAME.c.log"
  C_RC=$RC; C_MS=$MS; C_TOK=$TOKENS
  E0308="no"
  if grep -q "E0308" "$LOGS/$NAME.c.log"; then E0308="yes"; fi
  note "c: exit $C_RC in ${C_MS} ms, E0308: $E0308"
  [ "$C_RC" -eq 1 ] || fail "$NAME" "(c) type error gave exit $C_RC, expected 1 (see $LOGS/$NAME.c.log)"
  [ "$E0308" = "yes" ] || fail "$NAME" "(c) E0308 missing from the output (see $LOGS/$NAME.c.log)"

  # (d) restore
  git checkout --quiet -- .

  ROWS="$ROWS| $NAME | $A_RC / $B1_RC / $B2_RC / $C_RC | $A_MS / $B1_MS / $B2_MS / $C_MS | $CACHE | $A_TOK / $B1_TOK / $C_TOK | $E0308 |"$'\n'
done

echo "## orq real-world run"
echo
echo "orq: \`$("$ORQ" --version 2>/dev/null || echo unknown)\`, rustc: \`$(rustc -V 2>/dev/null || echo unknown)\`, host: \`$(uname -sm)\`"
echo
echo "Steps: a = clean tree, b1 = comment edit, b2 = repeat, c = type error."
echo
echo "| project | exit codes a/b1/b2/c | times ms a/b1/b2/c | b2 cache hit | tokens a/b1/c | E0308 in c |"
echo "|---|---|---|---|---|---|"
printf '%s' "$ROWS"
if [ "$FAILED" -ne 0 ]; then
  echo
  echo "**Unexpected results:** ${PROBLEMS:-}"
  exit 1
fi
