#!/usr/bin/env bash
# Checks every case in eval/cases: project/ fails at expected_fail_step,
# project/ + fix/ passes up to up_to. Usage: eval/validate.sh [case-id...]
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
work="$(mktemp -d)"; trap 'rm -rf "$work"' EXIT
export CARGO_TARGET_DIR="$work/target"
field() { sed -n "s/^$1 *= *\"\(.*\)\"/\1/p" "$2" | head -1; }
run_up_to() { # dir step -> exit status of the ladder up to step
  local d=$1 s=$2
  (cd "$d" && cargo fmt --check >/dev/null 2>&1 || true)  # fmt never fails a case
  (cd "$d" && cargo check -q --all-targets 2>/dev/null) || return 1
  [ "$s" = check ] && return 0
  (cd "$d" && cargo clippy -q --all-targets -- -D warnings 2>/dev/null) || return 1
  [ "$s" = clippy ] && return 0
  (cd "$d" && cargo test -q 2>/dev/null >/dev/null) || return 1
}
fail=0
cases=("$@"); [ ${#cases[@]} -eq 0 ] && cases=($(ls "$here/cases"))
for id in "${cases[@]}"; do
  c="$here/cases/$id"; toml="$c/case.toml"
  up_to=$(field up_to "$toml"); expect=$(field expected_fail_step "$toml")
  [ "$(field id "$toml")" = "$id" ] || { echo "FAIL $id: id mismatch"; fail=1; continue; }
  rm -rf "$work/p" && cp -r "$c/project" "$work/p"
  if run_up_to "$work/p" "$up_to"; then echo "FAIL $id: broken project passes up to $up_to"; fail=1; continue; fi
  if [ "$expect" != check ] && ! run_up_to "$work/p" check; then echo "FAIL $id: fails before $expect"; fail=1; continue; fi
  cp -r "$c/fix/." "$work/p/"
  if ! run_up_to "$work/p" "$up_to"; then echo "FAIL $id: fixed project does not pass up to $up_to"; fail=1; continue; fi
  echo "ok   $id"
done
exit $fail
