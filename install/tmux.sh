#!/bin/bash
# Install tmux configuration

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/common.sh"

if [ ! -f "$SUITCASE/tmux.conf" ]; then
  echo "No tmux.conf in suitcase, skipping."
  exit 0
fi

backup_if_needed "${HOME}/.tmux.conf"

cat > "${HOME}/.tmux.conf" <<EOF
$HEADER
source-file $SUITCASE/tmux.conf
EOF

echo "Installed ~/.tmux.conf"
