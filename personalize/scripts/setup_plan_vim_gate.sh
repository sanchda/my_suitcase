#!/bin/bash
# Build and install plan-vim-gate, then wire it as the Claude Code ExitPlanMode
# gate. plan-vim-gate is a terminal-native plan reviewer: when Claude finishes
# planning, the plan opens in nvim in a tmux split; you edit it, save & close
# the pane, and Claude proceeds with your edited plan.
#
# - Builds the Rust binary from suitcase/plan-vim-gate (cargo).
# - Installs it to ~/.local/bin/plan-vim-gate.
# - Merges ONLY the ExitPlanMode PermissionRequest hook into
#   ~/.claude/settings.json — other hooks/settings are preserved. Idempotent.
#
# Requires: cargo (rustup), and at runtime tmux + nvim + Claude Code in tmux.
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SUITCASE_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
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

if ! command -v jq >/dev/null 2>&1; then
  echo "jq is required to merge Claude settings. Install jq first." >&2
  exit 1
fi

echo "Building plan-vim-gate (release)..."
cargo build --release --manifest-path "$PROJECT_DIR/Cargo.toml"

mkdir -p "$BIN_DIR"
cp "$PROJECT_DIR/target/release/plan-vim-gate" "$BIN"
echo "Installed: $BIN"

# --- Wire the ExitPlanMode gate into ~/.claude/settings.json ---
settings_dir="$HOME/.claude"
settings="$settings_dir/settings.json"
mkdir -p "$settings_dir"

# Load existing settings (or {}). Back up invalid JSON rather than clobber it.
if [ -f "$settings" ]; then
  if jq empty "$settings" >/dev/null 2>&1; then
    base="$(cat "$settings")"
  else
    bak="${settings}.bak.$(date +%s)"
    echo "Existing settings.json is invalid JSON; backing up to $bak"
    cp "$settings" "$bak"
    base='{}'
  fi
else
  base='{}'
fi

# Replace any existing ExitPlanMode PermissionRequest matcher with ours (a gate
# can only have one owner), preserving every other matcher and hook event.
merged="$(printf '%s' "$base" | jq --arg cmd "$BIN" '
  .hooks.PermissionRequest = (
    ((.hooks.PermissionRequest // []) | map(select(.matcher != "ExitPlanMode")))
    + [ { matcher: "ExitPlanMode",
          hooks: [ { type: "command", command: $cmd, timeout: 345600 } ] } ]
  )
')"

tmp="$(mktemp)"
printf '%s\n' "$merged" > "$tmp"
mv "$tmp" "$settings"

echo "ExitPlanMode gate wired into $settings"
echo "Run Claude Code inside tmux to use it (the gate opens nvim in a split)."
