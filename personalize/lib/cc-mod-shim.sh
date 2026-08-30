#!/bin/bash
# Shared argument handling for personalize scripts that own one cc-mod.
# Sourced, not executed (it lives outside personalize/scripts/ so the runner
# never tries to run it).
#
# No args is `ensure`, which is how `personalize` invokes these: on unless you
# disabled it, and a no-op when already applied. See cc_mod_shim_usage below for
# the rest of the flags.
#
# Usage in a script that also builds something:
#
#   source "$SUITCASE_ROOT/personalize/lib/cc-mod-shim.sh"
#   cc_mod_skips_build "$@" && { cc_mod_shim <mod> "$@"; exit $?; }
#   ...build and install the binary...
#   cc_mod_shim <mod> "$@"

CC_MOD_SHIM_LIB="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CC_MOD_BIN="$(cd "$CC_MOD_SHIM_LIB/../.." && pwd)/bin/cc-mod"

# True for flags where building or installing anything first is pointless.
cc_mod_skips_build() {
  case "${1:-}" in
    --disable|--status|-h|--help) return 0 ;;
    *) return 1 ;;
  esac
}

cc_mod_shim_usage() {
  local mod="$1" self="${2:-$(basename "$0")}"
  cat <<EOF
$self -- manage the '$mod' Claude Code config mod

  $self              turn it on unless you disabled it (idempotent)
  $self --enable     turn it on even if you disabled it before
  $self --disable    turn it off, and keep it off
  $self --status     show what it owns
  $self -h           this help

Equivalent to: cc-mod ensure|enable|disable|status $mod
EOF
}

cc_mod_shim() {
  local mod="$1"
  shift
  if [ ! -x "$CC_MOD_BIN" ]; then
    echo "cc-mod not found or not executable: $CC_MOD_BIN" >&2
    return 1
  fi
  if ! command -v jq >/dev/null 2>&1; then
    echo "jq is required by cc-mod. Install jq first." >&2
    return 1
  fi

  case "${1:-}" in
    "") "$CC_MOD_BIN" ensure "$mod" ;;
    --enable) "$CC_MOD_BIN" enable "$mod" ;;
    --disable) "$CC_MOD_BIN" disable "$mod" ;;
    --status) "$CC_MOD_BIN" status "$mod" ;;
    -h|--help) cc_mod_shim_usage "$mod" ;;
    *)
      echo "unknown option: $1" >&2
      cc_mod_shim_usage "$mod" >&2
      return 2
      ;;
  esac
}
