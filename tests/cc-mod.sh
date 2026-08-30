#!/bin/bash
# Tests for cc-mod. Everything runs against a temp settings.json, temp mods dir,
# and temp receipt dir -- your real ~/.claude is never touched.
#
#   tests/cc-mod.sh          run all
#   tests/cc-mod.sh -v       show cc-mod output for each case
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SUITCASE_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
CC_MOD="$SUITCASE_ROOT/bin/cc-mod"
VERBOSE=0
[ "${1:-}" = "-v" ] && VERBOSE=1

pass=0
fail=0
current=""

setup() {
  ROOT="$(mktemp -d)"
  export CC_MOD_DIR="$ROOT/mods"
  export CLAUDE_SETTINGS="$ROOT/claude/settings.json"
  export CC_MOD_STATE="$ROOT/state"
  mkdir -p "$CC_MOD_DIR" "$(dirname "$CLAUDE_SETTINGS")"
}

teardown() { [ -n "${ROOT:-}" ] && rm -rf "$ROOT"; }

# mod <name> <json>
mod() {
  mkdir -p "$CC_MOD_DIR/$1"
  printf '%s\n' "$2" >"$CC_MOD_DIR/$1/mod.json"
}

settings() { printf '%s\n' "$1" >"$CLAUDE_SETTINGS"; }

cc() {
  if [ "$VERBOSE" = 1 ]; then
    "$CC_MOD" "$@"
  else
    "$CC_MOD" "$@" >/dev/null 2>&1
  fi
}

cc_out() { "$CC_MOD" "$@" 2>&1; }

# assert_json <jq filter> <expected> [label] -- a missing file reads as {}
assert_json() {
  local got
  if [ -f "$CLAUDE_SETTINGS" ]; then
    got="$(jq -c "$1" "$CLAUDE_SETTINGS" 2>&1)"
  else
    got="$(printf '{}' | jq -c "$1" 2>&1)"
  fi
  if [ "$got" = "$2" ]; then
    pass=$((pass + 1))
  else
    fail=$((fail + 1))
    printf 'FAIL %s\n  %s\n  want: %s\n  got:  %s\n' "$current" "${3:-$1}" "$2" "$got" >&2
  fi
}

note_pass() { pass=$((pass + 1)); }
note_fail() {
  fail=$((fail + 1))
  printf 'FAIL %s\n  %s\n' "$current" "$1" >&2
}

# succeeded <rc> <label> / failed <rc> <label>
succeeded() { if [ "$1" -eq 0 ]; then note_pass; else note_fail "$2 (rc=$1)"; fi; }
failed() { if [ "$1" -ne 0 ]; then note_pass; else note_fail "$2 (expected nonzero rc)"; fi; }

exists() { if [ -e "$1" ] || [ -L "$1" ]; then note_pass; else note_fail "$2: $1 missing"; fi; }
absent() { if [ ! -e "$1" ] && [ ! -L "$1" ]; then note_pass; else note_fail "$2: $1 still there"; fi; }
is_link() { if [ -L "$1" ]; then note_pass; else note_fail "$2: $1 is not a symlink"; fi; }

# assert_contains <haystack> <needle> <label>
assert_contains() {
  case "$1" in
    *"$2"*) pass=$((pass + 1)) ;;
    *)
      fail=$((fail + 1))
      printf 'FAIL %s\n  %s\n  missing %q in:\n%s\n' "$current" "$3" "$2" "$1" >&2
      ;;
  esac
}

test_case() { current="$1"; setup; }

HOOK_MOD='{
  "description": "test hook mod",
  "settings": {"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "/bin/mine"}]}]}}
}'

# --- round trip on an empty settings file ------------------------------------
test_case "enable then disable leaves no trace"
mod hooky "$HOOK_MOD"
cc enable hooky
assert_json '.hooks.Stop | length' '1' "hook appended"
assert_json '.hooks.Stop[0].hooks[0].command' '"/bin/mine"' "command written"
cc disable hooky
assert_json '.' '{}' "settings empty again (no orphan containers)"
absent "$CC_MOD_STATE/receipts/hooky.json" "receipt removed"
teardown

# --- co-tenancy: another owner's entry in the same array ---------------------
test_case "disable keeps entries this mod did not add"
mod hooky "$HOOK_MOD"
settings '{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"/bin/theirs"}]}]},"theme":"dark"}'
cc enable hooky
assert_json '.hooks.Stop | length' '2' "both entries present"
cc disable hooky
assert_json '.hooks.Stop | length' '1' "only ours removed"
assert_json '.hooks.Stop[0].hooks[0].command' '"/bin/theirs"' "their entry intact"
assert_json '.theme' '"dark"' "unrelated keys intact"
teardown

# --- prior scalar/object values are restored --------------------------------
test_case "disable restores a value the mod overwrote"
mod line '{"description":"sl","settings":{"statusLine":{"type":"command","command":"/bin/new"}}}'
settings '{"statusLine":{"type":"command","command":"/bin/old"}}'
cc enable line
assert_json '.statusLine.command' '"/bin/new"' "overwritten"
cc disable line
assert_json '.statusLine.command' '"/bin/old"' "prior value restored"
teardown

# --- adoption: settings already match the mod -------------------------------
test_case "enabling over identical config adopts it"
mod line '{"description":"sl","settings":{"statusLine":{"type":"command","command":"/bin/same"}}}'
settings '{"statusLine":{"type":"command","command":"/bin/same"}}'
cc enable line
assert_json '.statusLine.command' '"/bin/same"' "unchanged"
cc disable line
assert_json '.' '{}' "adopted config is removed on disable"
teardown

test_case "enabling over an identical hook entry does not duplicate it"
mod hooky "$HOOK_MOD"
settings '{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"/bin/mine"}]}]}}'
cc enable hooky
assert_json '.hooks.Stop | length' '1' "no duplicate"
cc disable hooky
assert_json '.' '{}' "removed on disable"
teardown

# --- idempotency ------------------------------------------------------------
test_case "enable is idempotent"
mod hooky "$HOOK_MOD"
cc enable hooky
cc enable hooky
cc enable hooky
assert_json '.hooks.Stop | length' '1' "still one entry"
cc disable hooky
assert_json '.' '{}' "single disable cleans up"
teardown

test_case "disable twice is harmless"
mod hooky "$HOOK_MOD"
cc enable hooky
cc disable hooky
out="$(cc_out disable hooky)"
assert_contains "$out" "already disabled" "second disable says so"
assert_json '.' '{}' "still clean"
teardown

# --- manifest changes -------------------------------------------------------
test_case "reapply follows a changed manifest"
mod hooky "$HOOK_MOD"
cc enable hooky
mod hooky '{
  "description": "test hook mod",
  "settings": {"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "/bin/renamed"}]}]}}
}'
cc reapply
assert_json '.hooks.Stop | length' '1' "no leftover from the old manifest"
assert_json '.hooks.Stop[0].hooks[0].command' '"/bin/renamed"' "new command applied"
cc disable hooky
assert_json '.' '{}' "clean after the rename"
teardown

# --- multiple mods sharing a subtree ----------------------------------------
test_case "two mods in one hooks tree are independent"
mod hooky "$HOOK_MOD"
mod other '{
  "description": "other",
  "settings": {"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "/bin/other"}]}]},
               "theme": "light"}
}'
cc enable hooky
cc enable other
assert_json '.hooks.Stop | length' '2' "both appended"
cc disable hooky
assert_json '.hooks.Stop | length' '1' "other survives"
assert_json '.hooks.Stop[0].hooks[0].command' '"/bin/other"' "the survivor is the other mod"
assert_json '.theme' '"light"' "other mod's scalar intact"
cc disable other
assert_json '.' '{}' "both gone"
teardown

# --- requirements -----------------------------------------------------------
test_case "a missing requirement blocks enable"
mod needy '{
  "description": "needs a thing",
  "requires": {"commands": ["definitely-not-installed-xyz"]},
  "settings": {"theme": "dark"}
}'
out="$(cc_out enable needy)"; rc=$?
failed "$rc" "enable refused"
assert_contains "$out" "definitely-not-installed-xyz" "names the missing command"
assert_json '.' '{}' "settings untouched"
absent "$CC_MOD_STATE/receipts/needy.json" "no receipt written"
teardown

test_case "a missing required file blocks enable"
mod needy '{
  "description": "needs a file",
  "requires": {"files": ["/nonexistent/thing"]},
  "settings": {"theme": "dark"}
}'
cc enable needy
assert_json '.' '{}' "settings untouched"
teardown

# --- links ------------------------------------------------------------------
test_case "links are created and removed"
mod linky '{
  "description": "links a file",
  "links": {"agents/tester.md": "assets/tester.md"},
  "settings": {"theme": "dark"}
}'
mkdir -p "$CC_MOD_DIR/linky/assets"
echo "agent" >"$CC_MOD_DIR/linky/assets/tester.md"
cc enable linky
is_link "$ROOT/claude/agents/tester.md" "symlink created"
cc disable linky
absent "$ROOT/claude/agents/tester.md" "symlink removed"
teardown

test_case "an existing real file is moved aside and restored"
mod linky '{
  "description": "links a file",
  "links": {"agents/tester.md": "assets/tester.md"},
  "settings": {"theme": "dark"}
}'
mkdir -p "$CC_MOD_DIR/linky/assets" "$ROOT/claude/agents"
echo "mine" >"$CC_MOD_DIR/linky/assets/tester.md"
echo "yours" >"$ROOT/claude/agents/tester.md"
cc enable linky
assert_contains "$(cat "$ROOT/claude/agents/tester.md")" "mine" "mod's file is live"
cc disable linky
assert_contains "$(cat "$ROOT/claude/agents/tester.md")" "yours" "original restored"
teardown

# --- drift ------------------------------------------------------------------
test_case "doctor reports drift and reapply fixes it"
mod hooky "$HOOK_MOD"
cc enable hooky
settings '{}'   # simulate a hand edit that wiped the hook
out="$(cc_out doctor)"
assert_contains "$out" "drifted" "doctor notices"
cc reapply
assert_json '.hooks.Stop | length' '1' "reapply restores it"
out="$(cc_out doctor)"
assert_contains "$out" "ok   hooky" "doctor is happy again"
teardown

test_case "status reports holds vs drift"
mod hooky "$HOOK_MOD"
cc enable hooky
out="$(cc_out status hooky)"
assert_contains "$out" "(holds)" "status says holds"
settings '{}'
out="$(cc_out status hooky)"
assert_contains "$out" "DRIFT" "status says drift"
teardown

# --- safety -----------------------------------------------------------------
test_case "invalid settings.json is refused, not clobbered"
mod hooky "$HOOK_MOD"
printf 'not json at all' >"$CLAUDE_SETTINGS"
out="$(cc_out enable hooky)"; rc=$?
failed "$rc" "enable refused"
assert_contains "$out" "not valid JSON" "explains why"
assert_contains "$(cat "$CLAUDE_SETTINGS")" "not json at all" "file left alone"
teardown

test_case "dry-run changes nothing"
mod hooky "$HOOK_MOD"
settings '{"theme":"dark"}'
out="$(cc_out --dry-run enable hooky)"
assert_contains "$out" '"Stop"' "prints the would-be settings"
assert_json '.hooks' 'null' "nothing written"
absent "$CC_MOD_STATE/receipts/hooky.json" "no receipt"
teardown

test_case "unknown mod is an error"
out="$(cc_out enable nope)"; rc=$?
failed "$rc" "enable refused"
assert_contains "$out" "unknown mod: nope" "names it"
teardown

# --- listing ----------------------------------------------------------------
test_case "list shows state and description"
mod hooky "$HOOK_MOD"
mod line '{"description":"sl","settings":{"statusLine":{"type":"command","command":"/bin/x"}}}'
out="$(cc_out list)"
assert_contains "$out" "hooky            disabled  test hook mod" "disabled row"
cc enable hooky
out="$(cc_out list)"
assert_contains "$out" "hooky            enabled   test hook mod" "enabled row"
teardown

test_case "enable --default only takes mods marked default"
mod on '{"description":"d","default":true,"settings":{"theme":"dark"}}'
mod off '{"description":"nd","settings":{"model":"opus"}}'
cc enable --default
assert_json '.theme' '"dark"' "default mod enabled"
assert_json '.model' 'null' "non-default mod skipped"
teardown

test_case "disable sticks across a bulk enable"
mod on '{"description":"d","default":true,"settings":{"theme":"dark"}}'
mod two '{"description":"d2","default":true,"settings":{"model":"opus"}}'
cc enable --default
cc disable on
out="$(cc_out enable --default)"
assert_contains "$out" "on: left off" "bulk enable respects the opt-out"
assert_json '.theme' 'null' "stays off"
assert_json '.model' '"opus"' "the other default is still on"
out="$(cc_out list)"
assert_contains "$out" "opted-out" "list shows the opt-out"
cc enable on
assert_json '.theme' '"dark"' "naming it explicitly turns it back on"
out="$(cc_out enable --default)"
case "$out" in *"left off"*) note_fail "opt-out should be cleared by an explicit enable" ;; *) note_pass ;; esac
teardown

test_case "a bulk enable skips, not aborts, on unmet requirements"
mod good '{"description":"ok","default":true,"settings":{"theme":"dark"}}'
mod needy '{"description":"needs","default":true,
  "requires":{"commands":["definitely-not-installed-xyz"]},
  "settings":{"model":"opus"}}'
out="$(cc_out enable --default)"; rc=$?
succeeded "$rc" "batch still succeeds"
assert_contains "$out" "needy: skipped" "reports the skip"
assert_json '.theme' '"dark"' "the healthy mod is enabled"
assert_json '.model' 'null' "the broken one is not"
absent "$CC_MOD_STATE/receipts/needy.json" "no receipt for the skipped mod"
teardown

test_case "a receipt with no mod.json is surfaced and disablable"
mod ghost "$HOOK_MOD"
cc enable ghost
rm -rf "$CC_MOD_DIR/ghost"
out="$(cc_out list)"
assert_contains "$out" "no mod.json" "list flags the orphan"
out="$(cc_out doctor)"
assert_contains "$out" "unknown mod ghost" "doctor flags it"
cc disable ghost
assert_json '.' '{}' "orphan receipt still reverts cleanly"
teardown

# --- ensure (the verb personalize scripts use) -------------------------------
test_case "ensure turns a mod on, then does nothing"
mod hooky "$HOOK_MOD"
settings '{"theme":"dark"}'
cc ensure hooky
assert_json '.hooks.Stop | length' '1' "enabled on first ensure"
before="$(find "$CC_MOD_STATE/backups" -name 'settings.*.json' 2>/dev/null | wc -l | tr -d ' ')"
out="$(cc_out ensure hooky)"
assert_contains "$out" "already current" "second ensure is a no-op"
after="$(find "$CC_MOD_STATE/backups" -name 'settings.*.json' 2>/dev/null | wc -l | tr -d ' ')"
if [ "$before" = "$after" ]; then note_pass; else note_fail "no-op ensure still wrote settings ($before -> $after)"; fi
teardown

test_case "ensure will not undo a disable"
mod hooky "$HOOK_MOD"
cc ensure hooky
cc disable hooky
out="$(cc_out ensure hooky)"; rc=$?
succeeded "$rc" "ensure still succeeds"
assert_contains "$out" "left off" "says why"
assert_json '.' '{}' "stays off"
cc enable hooky
assert_json '.hooks.Stop | length' '1' "explicit enable overrides the opt-out"
teardown

test_case "ensure skips what it cannot satisfy, without failing"
mod needy '{"description":"needs","default":true,
  "requires":{"commands":["definitely-not-installed-xyz"]},
  "settings":{"model":"opus"}}'
out="$(cc_out ensure needy)"; rc=$?
succeeded "$rc" "ensure succeeds anyway"
assert_contains "$out" "not installed yet" "explains the skip"
assert_json '.' '{}' "nothing written"
teardown

test_case "ensure heals a hand-edit and follows a changed manifest"
mod hooky "$HOOK_MOD"
cc ensure hooky
settings '{}'
cc ensure hooky
assert_json '.hooks.Stop | length' '1' "drift healed"
mod hooky '{"description":"h","settings":{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"/bin/v2"}]}]}}}'
cc ensure hooky
assert_json '.hooks.Stop | length' '1' "no leftover from the old manifest"
assert_json '.hooks.Stop[0].hooks[0].command' '"/bin/v2"' "new command applied"
teardown

test_case "ensure ignores a mod that no longer exists"
out="$(cc_out ensure ghostmod)"; rc=$?
succeeded "$rc" "ensure does not fail a personalize run"
assert_contains "$out" "no such mod" "says so"
teardown

# --- backups ----------------------------------------------------------------
test_case "settings are backed up before each write"
mod hooky "$HOOK_MOD"
settings '{"theme":"dark"}'
cc enable hooky
count="$(find "$CC_MOD_STATE/backups" -name 'settings.*.json' 2>/dev/null | wc -l | tr -d ' ')"
if [ "$count" -ge 1 ]; then note_pass; else note_fail "no backup written"; fi
teardown

# --- real mods in this repo -------------------------------------------------
current="repo mods are valid"
for manifest in "$SUITCASE_ROOT"/claude/mods/*/mod.json; do
  [ -f "$manifest" ] || continue
  name="$(basename "$(dirname "$manifest")")"
  if jq empty "$manifest" >/dev/null 2>&1; then
    pass=$((pass + 1))
  else
    fail=$((fail + 1))
    printf 'FAIL %s\n  %s is not valid JSON\n' "$current" "$manifest" >&2
  fi
  desc="$(jq -r '.description // empty' "$manifest")"
  if [ -n "$desc" ]; then
    pass=$((pass + 1))
  else
    fail=$((fail + 1))
    printf 'FAIL %s\n  %s has no description\n' "$current" "$name" >&2
  fi
done

printf '\n%d passed, %d failed\n' "$pass" "$fail"
[ "$fail" -eq 0 ]
