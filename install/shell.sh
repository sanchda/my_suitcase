#!/bin/bash
# Install shell configuration (bashrc + zshrc)

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/common.sh"

cfg_bash="${HOME}/.bashrc"
cfg_zsh="${HOME}/.zshrc"

# On macOS, bash reads .bash_profile for login shells
if [ "$SC_OS" = "darwin" ]; then
  cfg_bash="${HOME}/.bash_profile"
fi

# --- bashrc ---
backup_if_needed "$cfg_bash"

cat > "$cfg_bash" <<BASHEOF
$HEADER
export SUITCASE="$SUITCASE"
. "\$SUITCASE/shell/core.sh"
. "\$SUITCASE/shell/bash.sh"
BASHEOF

echo "Installed $cfg_bash"

# --- zshrc ---
backup_if_needed "$cfg_zsh"

cat > "$cfg_zsh" <<ZSHEOF
$HEADER
export SUITCASE="$SUITCASE"
. "\$SUITCASE/shell/core.sh"
. "\$SUITCASE/shell/zsh.sh"
ZSHEOF

echo "Installed $cfg_zsh"
