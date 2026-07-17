#!/bin/bash
# Build and install claude-top: a live read-only TUI showing local Claude Code
# instances (tmux pane, dir, git branch/worktree, account) and current-account
# usage by model and by instance.
#
# - Builds the Rust binary from suitcase/claude-top (cargo).
# - Installs it to ~/.local/bin/claude-top.
# - No settings.json changes.
#
# Requires: cargo (rustup). At runtime, optionally uses tmux/git/lsof (all
# degrade gracefully when absent).
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SUITCASE_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
PROJECT_DIR="$SUITCASE_ROOT/tools/claude-top"
BIN_DIR="$HOME/.local/bin"
BIN="$BIN_DIR/claude-top"

if [ ! -f "$PROJECT_DIR/Cargo.toml" ]; then
  echo "claude-top project not found at $PROJECT_DIR" >&2
  exit 1
fi

if ! command -v cargo >/dev/null 2>&1; then
  echo "cargo not found. Install the Rust toolchain (https://rustup.rs) first." >&2
  exit 1
fi

echo "Building claude-top (release)..."
cargo build --release --manifest-path "$PROJECT_DIR/Cargo.toml"

mkdir -p "$BIN_DIR"
cp "$PROJECT_DIR/target/release/claude-top" "$BIN"
echo "Installed: $BIN"
echo "Run 'claude-top' inside a terminal (ideally within tmux). q quits, t cycles the usage window."
