#!/bin/bash
# Turn on every default Claude Code config mod (claude/mods/*/mod.json) via cc-mod.
#
# Individual mods have their own scripts (setup_bark.sh, setup_claude_statusline.sh,
# setup_plan_vim_gate.sh); this is the catch-all, so a mod added to claude/mods/
# with "default": true gets picked up without a new script.
#
# Run with -h for the flags.
#
# The zz_ prefix is load-bearing: `personalize/personalize` runs scripts in sorted
# order, and mods declare requirements on binaries the setup_* scripts build.
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SUITCASE_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
CC_MOD="$SUITCASE_ROOT/bin/cc-mod"

if [ ! -x "$CC_MOD" ]; then
  echo "cc-mod not found or not executable: $CC_MOD" >&2
  exit 1
fi

if ! command -v jq >/dev/null 2>&1; then
  echo "jq is required by cc-mod. Install jq first." >&2
  exit 1
fi

usage() {
  cat <<'EOF'
zz_setup_claude_mods.sh -- turn on the default Claude Code config mods

  zz_setup_claude_mods.sh                  ensure every default mod is on
  zz_setup_claude_mods.sh --disable [mod]  off (all mods, or just the named one)
  zz_setup_claude_mods.sh --enable <mod>   on again after a --disable
  zz_setup_claude_mods.sh --list           what exists and what is on
  zz_setup_claude_mods.sh --status [mod]   what each mod owns

Individual mods also have their own scripts (setup_bark.sh,
setup_claude_statusline.sh, setup_plan_vim_gate.sh) taking the same flags. This
is the catch-all, so a new mod marked "default": true needs no new script.

`ensure` never overrides a mod you disabled by hand, and does nothing at all when
a mod is already applied -- safe on every personalize run.
EOF
}

case "${1:-}" in
  "")
    "$CC_MOD" ensure --default
    "$CC_MOD" doctor || true
    ;;
  --disable)
    shift
    if [ "$#" -gt 0 ]; then
      "$CC_MOD" disable "$@"
    else
      "$CC_MOD" disable --all
    fi
    ;;
  --enable)
    shift
    [ "$#" -gt 0 ] || { echo "--enable needs a mod name (see --list)" >&2; exit 2; }
    "$CC_MOD" enable "$@"
    ;;
  --list)
    "$CC_MOD" list
    ;;
  --status)
    shift
    "$CC_MOD" status "$@"
    ;;
  -h|--help)
    usage
    ;;
  *)
    echo "unknown option: $1" >&2
    usage >&2
    exit 2
    ;;
esac
