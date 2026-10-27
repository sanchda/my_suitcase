#!/bin/bash
# Build and install panel — ensemble pairwise ranking with decorrelated
# one-shot voters (a panel judges every pair against one fixed criterion; a
# Copeland tally turns per-pair majorities into an order).
#
# - Builds the Rust binary from suitcase/tools/panel (cargo).
# - Installs it to ~/.local/bin/panel.
#
# panel is driven by two files you supply: a criterion written before anything
# reads the source, and a manifest of items and voter personae. Run
# `panel --help`, and see the ensemble-pairwise-ranking skill for the protocol
# the two files belong to.
#
# Requires: cargo (rustup). At runtime: the authenticated `claude` CLI on PATH,
# and `codex` too if a manifest puts voters on that backend.
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SUITCASE_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
PROJECT_DIR="$SUITCASE_ROOT/tools/panel"
BIN_DIR="$HOME/.local/bin"
BIN="$BIN_DIR/panel"

if [ ! -f "$PROJECT_DIR/Cargo.toml" ]; then
  echo "panel project not found at $PROJECT_DIR" >&2
  exit 1
fi

if ! command -v cargo >/dev/null 2>&1; then
  echo "cargo not found. Install the Rust toolchain (https://rustup.rs) first." >&2
  exit 1
fi

echo "Building panel (release)..."
cargo build --release --manifest-path "$PROJECT_DIR/Cargo.toml"

mkdir -p "$BIN_DIR"
# Replace atomically: overwriting a mapped executable can fail with "Text file
# busy", while rename is safe for any run still in flight.
BIN_TMP="$(mktemp "$BIN_DIR/.panel.XXXXXX")"
trap 'rm -f "$BIN_TMP"' EXIT
cp "$PROJECT_DIR/target/release/panel" "$BIN_TMP"
chmod 755 "$BIN_TMP"
mv -f "$BIN_TMP" "$BIN"
trap - EXIT
echo "Installed: $BIN"

echo "Ensure ~/.local/bin is on your PATH, then run 'panel --help'."
# The matching Claude skill (claude/mods/ensemble-pairwise-ranking-skill) is a
# default mod, so zz_setup_claude_mods.sh installs it after this script has
# built the binary it requires.
echo "Claude skill: cc-mod ensure ensemble-pairwise-ranking-skill (personalize does this for you)."
