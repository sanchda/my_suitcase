#!/bin/bash
# Claude Code status line: ccusage cost/burn plus 5h/7d subscription limit bars,
# rendered by bin/cc-statusline.
#
# The settings.json wiring is the `statusline` cc-mod, so this script is just the
# on/off switch for it:
#
#   setup_claude_statusline.sh              on, unless you disabled it
#   setup_claude_statusline.sh --disable    off, and it stays off
#   setup_claude_statusline.sh --enable     on again after a --disable
#   setup_claude_statusline.sh --status     what it owns
#
# Requires jq. At runtime the status line shells out to `npx ccusage`, so the box
# needs Node.js -- cc-mod prints a note when npx is missing.
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SUITCASE_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

# shellcheck source-path=SCRIPTDIR source=../lib/cc-mod-shim.sh
. "$SUITCASE_ROOT/personalize/lib/cc-mod-shim.sh"

cc_mod_shim statusline "$@"
