#!/usr/bin/env bash
# Checks every case in eval/cases: project/ fails at expected_fail_step,
# project/ + fix/ passes up to up_to. Usage: eval/validate.sh [case-id...]
# CASES_DIR picks another directory of cases (e.g. a project's tasks/). A
# case with `base = "<dir>"` (relative to the case) starts from that project,
# with its project/ copied on top.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
work="$(mktemp -d)"; trap 'rm -rf "$work"' EXIT
export CARGO_TARGET_DIR="${VALIDATE_TARGET_DIR:-$work/target}"  # reuse one to skip cold builds
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
cases_dir="${CASES_DIR:-$here/cases}"
cases=("$@"); [ ${#cases[@]} -eq 0 ] && cases=($(ls "$cases_dir"))
for id in "${cases[@]}"; do
  c="$cases_dir/$id"; toml="$c/case.toml"
  up_to=$(field up_to "$toml"); expect=$(field expected_fail_step "$toml")
  [ "$(field id "$toml")" = "$id" ] || { echo "FAIL $id: id mismatch"; fail=1; continue; }
  rm -rf "$work/p"
  base=$(field base "$toml")
  if [ -n "$base" ]; then
    mkdir -p "$work/p"
    (cd "$c/$base" && tar cf - --exclude=./tasks --exclude=./target .) | tar xmf - -C "$work/p"  # m: fresh mtimes, the target dir may be shared
    cp -r "$c/project/." "$work/p/"
  else
    cp -r "$c/project" "$work/p"
  fi
  if run_up_to "$work/p" "$up_to"; then echo "FAIL $id: broken project passes up to $up_to"; fail=1; continue; fi
  if [ "$expect" != check ] && ! run_up_to "$work/p" check; then echo "FAIL $id: fails before $expect"; fail=1; continue; fi
  cp -r "$c/fix/." "$work/p/"
  if ! run_up_to "$work/p" "$up_to"; then echo "FAIL $id: fixed project does not pass up to $up_to"; fail=1; continue; fi
  echo "ok   $id"
done
exit $fail
