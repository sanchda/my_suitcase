#!/bin/bash
# Turn on the coordination + ralph Claude Code skill mods, and be loud when
# the binaries they need are missing.
#
# Owns four cc-mods (all links-only skills under claude/mods/):
#   agent-coord    multi-agent-coordination (needs bin/with-lock, in-repo)
#   ralph-loop     needs ~/.local/bin/ralph
#   ralphd-bridge  needs ~/.local/bin/ralphd
#   ralph-kanban   needs ralph + with-lock
#
# The zz_ catch-all ensures these too, but it skips a mod with missing
# requirements quietly. This script exists to nag: if ralph/ralphd are not
# built yet it says so and points at setup_ralph.sh (which builds both),
# then still ensures whatever can be ensured.
#
# Flags (applied to all four mods):
#   setup_ralph_skills.sh              ensure: on unless you disabled them
#   setup_ralph_skills.sh --enable     on again after a --disable
#   setup_ralph_skills.sh --disable    off, and they stay off
#   setup_ralph_skills.sh --status     what each mod owns
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SUITCASE_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
# shellcheck source-path=SCRIPTDIR source=../lib/cc-mod-shim.sh
. "$SUITCASE_ROOT/personalize/lib/cc-mod-shim.sh"

MODS="agent-coord ralph-loop ralphd-bridge ralph-kanban"
BIN_DIR="$HOME/.local/bin"

usage() {
  sed -n '2,21p' "$0" | sed 's/^# \{0,1\}//'
}

case "${1:-}" in
  -h|--help)
    usage
    exit 0
    ;;
esac

# Nag before ensure/enable; --disable and --status need no binaries.
if ! cc_mod_skips_build "$@"; then
  if [ ! -x "$BIN_DIR/ralph" ]; then
    echo "NOTE: ralph is not installed ($BIN_DIR/ralph missing)." >&2
    echo "      The ralph-loop and ralph-kanban skills stay off until you run:" >&2
    echo "      $SUITCASE_ROOT/personalize/scripts/setup_ralph.sh" >&2
  fi
  if [ ! -x "$BIN_DIR/ralphd" ]; then
    echo "NOTE: ralphd is not installed ($BIN_DIR/ralphd missing)." >&2
    echo "      The ralphd-bridge skill stays off until you run:" >&2
    echo "      $SUITCASE_ROOT/personalize/scripts/setup_ralph.sh  (builds ralphd too)" >&2
  fi
fi

rc=0
for mod in $MODS; do
  cc_mod_shim "$mod" "$@" || rc=$?
done
exit "$rc"
