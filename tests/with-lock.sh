#!/usr/bin/env bash
# Tests for with-lock. Everything runs against a temp WITH_LOCK_DIR --
# your real ~/.locks is never touched.
#
#   tests/with-lock.sh          run all
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SUITCASE_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
WL="$SUITCASE_ROOT/bin/with-lock"

pass=0
fail=0
current=""

setup() {
  ROOT="$(mktemp -d)"
  export WITH_LOCK_DIR="$ROOT/locks"
  unset WITH_LOCK_LABEL 2>/dev/null || true
}

teardown() { [ -n "${ROOT:-}" ] && rm -rf "$ROOT"; }

note_pass() { pass=$((pass + 1)); }
note_fail() {
  fail=$((fail + 1))
  printf 'FAIL %s\n  %s\n' "$current" "$1" >&2
}

# rc_is <got> <want> <label>
rc_is() { if [ "$1" -eq "$2" ]; then note_pass; else note_fail "$3 (rc=$1, want $2)"; fi; }
exists() { if [ -e "$1" ]; then note_pass; else note_fail "$2: $1 missing"; fi; }
absent() { if [ ! -e "$1" ]; then note_pass; else note_fail "$2: $1 still there"; fi; }

# assert_contains <haystack> <needle> <label>
assert_contains() {
  case "$1" in
    *"$2"*) note_pass ;;
    *) note_fail "$3: '$2' not in output: $1" ;;
  esac
}

# wait_for <path> -- poll up to ~2s for a file to appear
wait_for() {
  local _
  for _ in $(seq 1 40); do
    [ -e "$1" ] && return 0
    sleep 0.05
  done
  return 1
}

# --- cases -------------------------------------------------------------------

current="acquire, run, release"
setup
out="$("$WL" --resource r1 --label t1 -- echo ran 2>&1)"; rc=$?
rc_is "$rc" 0 "plain run succeeds"
assert_contains "$out" "ran" "wrapped command output passes through"
exists "$WITH_LOCK_DIR/r1.lock" "lock file created"
absent "$WITH_LOCK_DIR/r1.lock.holder" "holder file removed after exit"
teardown

current="wrapped exit code propagates"
setup
"$WL" --resource r1 -- sh -c 'exit 3'; rc=$?
rc_is "$rc" 3 "exit code 3 propagates"
teardown

current="holder metadata while held"
setup
"$WL" --resource r1 --label holder1 -- sleep 3 &
bg=$!
wait_for "$WITH_LOCK_DIR/r1.lock.holder" || note_fail "holder file never appeared"
holder="$(cat "$WITH_LOCK_DIR/r1.lock.holder" 2>/dev/null || true)"
assert_contains "$holder" "label=holder1" "holder records the label"
assert_contains "$holder" "cmd=" "holder records the command"

current="contention times out with 75 and prints holder"
out="$("$WL" --resource r1 --label loser --timeout 1 -- true 2>&1)"; rc=$?
rc_is "$rc" 75 "contender exits 75"
assert_contains "$out" "label=holder1" "contender sees the holder"

current="--status HELD while held"
out="$("$WL" --resource r1 --status 2>&1)"
assert_contains "$out" "HELD" "status reports HELD"
assert_contains "$out" "label=holder1" "status prints holder"
wait "$bg" 2>/dev/null
out="$("$WL" --resource r1 --status 2>&1)"
assert_contains "$out" "FREE" "status reports FREE after release"
teardown

current="crash safety: SIGKILL frees the lock, stale holder file ignored"
setup
setsid "$WL" --resource r1 --label doomed -- sleep 60 &
bg=$!
wait_for "$WITH_LOCK_DIR/r1.lock.holder" || note_fail "holder file never appeared"
kill -9 -- "-$bg" 2>/dev/null
wait "$bg" 2>/dev/null
exists "$WITH_LOCK_DIR/r1.lock.holder" "holder file is stale (trap never ran)"
out="$("$WL" --resource r1 --status 2>&1)"
assert_contains "$out" "FREE" "status trusts the lock, not the stale file"
"$WL" --resource r1 --timeout 2 --label next -- true; rc=$?
rc_is "$rc" 0 "next acquisition succeeds immediately"
teardown

current="--list"
setup
"$WL" --resource alpha -- true
"$WL" --resource beta --label lister -- sleep 3 &
bg=$!
wait_for "$WITH_LOCK_DIR/beta.lock.holder" || note_fail "holder file never appeared"
out="$("$WL" --list 2>&1)"
assert_contains "$out" "alpha" "list shows alpha"
assert_contains "$out" "FREE" "alpha is free"
assert_contains "$out" "beta" "list shows beta"
assert_contains "$out" "HELD" "beta is held"
wait "$bg" 2>/dev/null
teardown

current="usage errors"
setup
"$WL" -- true 2>/dev/null; rc=$?
rc_is "$rc" 2 "missing --resource is a usage error"
"$WL" --resource r1 2>/dev/null; rc=$?
rc_is "$rc" 2 "missing command is a usage error"
"$WL" --status 2>/dev/null; rc=$?
rc_is "$rc" 2 "--status without --resource is a usage error"
teardown

printf '%d passed, %d failed\n' "$pass" "$fail"
[ "$fail" -eq 0 ]
