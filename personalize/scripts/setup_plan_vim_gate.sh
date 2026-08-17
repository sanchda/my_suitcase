#!/bin/bash
# Build and install plan-vim-gate: a terminal-native plan reviewer. When Claude
# finishes planning, the plan opens in nvim in a tmux split; you edit it, save &
# close the pane, and Claude proceeds with your edited plan.
#
# - Builds the Rust binary from suitcase/plan-vim-gate (cargo).
# - Installs it to ~/.local/bin/plan-vim-gate.
#
# The settings.json wiring is the `plan-vim-gate` cc-mod, turned on here.
#
# Flags (the mod, not the binary):
#   --disable   stop gating ExitPlanMode, and keep it that way
#   --enable    start again after a --disable
#   --status    show what the mod owns
#
# Requires: cargo (rustup), and at runtime tmux + nvim + Claude Code in tmux.
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SUITCASE_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

# shellcheck source-path=SCRIPTDIR source=../lib/cc-mod-shim.sh
. "$SUITCASE_ROOT/personalize/lib/cc-mod-shim.sh"

# Ungating (or just asking) needs no build.
if cc_mod_skips_build "$@"; then
  cc_mod_shim plan-vim-gate "$@"
  exit $?
fi

PROJECT_DIR="$SUITCASE_ROOT/plan-vim-gate"
BIN_DIR="$HOME/.local/bin"
BIN="$BIN_DIR/plan-vim-gate"

if [ ! -f "$PROJECT_DIR/Cargo.toml" ]; then
  echo "plan-vim-gate project not found at $PROJECT_DIR" >&2
  exit 1
fi

if ! command -v cargo >/dev/null 2>&1; then
  echo "cargo not found. Install the Rust toolchain (https://rustup.rs) first." >&2
  exit 1
fi

echo "Building plan-vim-gate (release)..."
cargo build --release --manifest-path "$PROJECT_DIR/Cargo.toml"

mkdir -p "$BIN_DIR"
cp "$PROJECT_DIR/target/release/plan-vim-gate" "$BIN"
echo "Installed: $BIN"

cc_mod_shim plan-vim-gate "$@"
echo "Run Claude Code inside tmux to use it (the gate opens nvim in a split)."
