#!/bin/bash
# Opens Dave's suitcase — modular installer

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
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

# rlwrap histories
[ ! -d "${HOME}/history" ] && mkdir -m 0700 "${HOME}/history"

echo ""
echo "=== Suitcase installed ==="
