#!/bin/bash
# Opens Dave's suitcase — modular installer

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# install.sh is the authoritative entry point and lives at the repo root, so
# force SUITCASE from its own location. This overrides any stale SUITCASE
# inherited from a prior install (e.g. an old ~/my_suitcase checkout), which
# would otherwise win via the -z guard in common.sh. Sub-scripts still inherit
# this resolved value.
export SUITCASE="$SCRIPT_DIR"
source "$SCRIPT_DIR/install/common.sh"

echo "=== Suitcase Installer ==="
echo "OS detected: $SC_OS"
echo "Suitcase root: $SUITCASE"
echo ""

# macOS prerequisites
if [ "$SC_OS" = "darwin" ]; then
  bash "$SUITCASE/install/macos.sh"
fi

# Shell config
bash "$SUITCASE/install/shell.sh"

# tmux
bash "$SUITCASE/install/tmux.sh"

# atuin (config symlink only — binary install lives in personalize/scripts/get_atuin.sh)
bash "$SUITCASE/install/atuin.sh"

# rlwrap histories
[ ! -d "${HOME}/history" ] && mkdir -m 0700 "${HOME}/history"

echo ""
echo "=== Suitcase installed ==="

# Verify — run in a subshell with SUITCASE exported so sc-doctor can resolve
# sourced files even though this shell never sourced the new rc files.
echo ""
SUITCASE="$SUITCASE" bash "$SUITCASE/bin/sc-doctor" || true
