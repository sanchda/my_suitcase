#!/bin/bash
# Install shell configuration (bashrc + zshrc + zshenv)

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/common.sh"

cfg_bash="${HOME}/.bashrc"
cfg_zsh="${HOME}/.zshrc"
cfg_zshenv="${HOME}/.zshenv"

# On macOS, bash reads .bash_profile for login shells
if [ "$SC_OS" = "darwin" ]; then
  cfg_bash="${HOME}/.bash_profile"
fi

# rc_body <shell-specific-file>
#   Emit a thin rc file that bootstraps sc_source (with a trivial fallback if
#   boot.sh is missing) and guard-sources core + the shell-specific file, so a
#   moved/renamed file warns clearly instead of erroring on every startup.
rc_body() {
  cat <<RCEOF
$HEADER
export SUITCASE="$SUITCASE"
if [ -r "\$SUITCASE/shell/boot.sh" ]; then . "\$SUITCASE/shell/boot.sh"; fi
command -v sc_source >/dev/null 2>&1 || sc_source() { [ -r "\$1" ] && . "\$1"; }
sc_source "\$SUITCASE/shell/core.sh"
sc_source "\$SUITCASE/shell/$1"
RCEOF
}

# --- bashrc ---
backup_if_needed "$cfg_bash"
rc_body "bash.sh" > "$cfg_bash"
echo "Installed $cfg_bash"

# --- zshrc ---
backup_if_needed "$cfg_zsh"
rc_body "zsh.sh" > "$cfg_zsh"
echo "Installed $cfg_zsh"

# --- zshenv ---
# Owned by the suitcase so it can't drift. Holds only truly-global env that
# non-interactive zsh needs (core.sh early-returns on non-interactive shells).
backup_if_needed "$cfg_zshenv"
cat > "$cfg_zshenv" <<ZSHENVEOF
$HEADER
export SUITCASE="$SUITCASE"
[ -r "\$HOME/.cargo/env" ] && . "\$HOME/.cargo/env"
ZSHENVEOF
echo "Installed $cfg_zshenv"
