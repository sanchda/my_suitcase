#!/bin/bash
# Build and install the ralph runner — the external autonomous loop for Claude
# Code (fresh-context `claude -p` iterations with budgets, an opt-in
# per-iteration timeout, and no-progress escalation).
#
# - Builds the Rust binary from suitcase/tools/ralph (cargo).
# - Installs it to ~/.local/bin/ralph.
#
# ralph is driven entirely by files LOCAL to whatever repo you run it in
# (tools/ralph/PROMPT.md, VISION/BACKLOG/PROGRESS, and an optional ralph.toml).
# See suitcase/tools/ralph/README.md.
#
# Requires: cargo (rustup). At runtime: the authenticated `claude` CLI on PATH.
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SUITCASE_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
PROJECT_DIR="$SUITCASE_ROOT/tools/ralph"
BIN_DIR="$HOME/.local/bin"
BIN="$BIN_DIR/ralph"

if [ ! -f "$PROJECT_DIR/Cargo.toml" ]; then
  echo "ralph project not found at $PROJECT_DIR" >&2
  exit 1
fi

if ! command -v cargo >/dev/null 2>&1; then
  echo "cargo not found. Install the Rust toolchain (https://rustup.rs) first." >&2
  exit 1
fi

echo "Building ralph (release)..."
cargo build --release --manifest-path "$PROJECT_DIR/Cargo.toml"

mkdir -p "$BIN_DIR"
cp "$PROJECT_DIR/target/release/ralph" "$BIN"
echo "Installed: $BIN"
echo "Ensure ~/.local/bin is on your PATH, then run 'ralph --help' from a target repo."
