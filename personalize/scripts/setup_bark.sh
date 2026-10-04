#!/bin/bash
# Build and install bark -- a one-shot "post this line to a Discord webhook"
# CLI for scripts, cron jobs, and long-running loops.
#
# - Builds the Rust binary from suitcase/tools/bark (cargo).
# - Installs it to ~/.local/bin/bark.
# - Seeds ~/.config/bark/config.toml (0600) if it does not exist yet.
# - Turns on the `bark-notify` cc-mod, which is what makes Claude Code bark on
#   Stop/Notification/SessionEnd.
#
# Flags (the mod, not the binary):
#   --disable   stop Claude Code barking, and keep it that way
#   --enable    start again after a --disable
#   --status    show what the mod owns
#
# The webhook is read from $BARK_WEBHOOK. Never commit a real webhook URL here:
# anyone holding it can post to the channel. An existing config is never
# touched, so this is safe to re-run.
#
# Requires: cargo (rustup). At runtime: curl.
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SUITCASE_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

# shellcheck source-path=SCRIPTDIR source=../lib/cc-mod-shim.sh
. "$SUITCASE_ROOT/personalize/lib/cc-mod-shim.sh"

# Turning the notifier off (or just asking about it) needs no build.
if cc_mod_skips_build "$@"; then
  cc_mod_shim bark-notify "$@"
  exit $?
fi

PROJECT_DIR="$SUITCASE_ROOT/tools/bark"
BIN_DIR="$HOME/.local/bin"
BIN="$BIN_DIR/bark"
CONFIG_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/bark"
CONFIG="$CONFIG_DIR/config.toml"

if [ ! -f "$PROJECT_DIR/Cargo.toml" ]; then
  echo "bark project not found at $PROJECT_DIR" >&2
  exit 1
fi

if ! command -v cargo >/dev/null 2>&1; then
  echo "cargo not found. Install the Rust toolchain (https://rustup.rs) first." >&2
  exit 1
fi

if ! command -v curl >/dev/null 2>&1; then
  echo "warning: curl not found -- bark needs it at runtime to POST." >&2
fi

echo "Building bark (release)..."
cargo build --release --manifest-path "$PROJECT_DIR/Cargo.toml"

mkdir -p "$BIN_DIR"
# Replace atomically: Linux rejects overwriting an executable that a running
# process has mapped ("Text file busy"), while rename is always safe.
BIN_TMP="$(mktemp "$BIN_DIR/.bark.XXXXXX")"
trap 'rm -f "$BIN_TMP"' EXIT
cp "$PROJECT_DIR/target/release/bark" "$BIN_TMP"
chmod 755 "$BIN_TMP"
mv -f "$BIN_TMP" "$BIN"
trap - EXIT
echo "Installed: $BIN"

if [ -f "$CONFIG" ]; then
  echo "Config already present, leaving it alone: $CONFIG"
else
  WEBHOOK="${BARK_WEBHOOK:-}"
  if [ -z "$WEBHOOK" ]; then
    echo "No webhook to seed (set BARK_WEBHOOK, or run 'bark init --webhook <url>')."
  else
    mkdir -p "$CONFIG_DIR"
    # --config so an exported BARK_CONFIG cannot redirect the seed elsewhere;
    # bark writes the file 0600 itself.
    "$BIN" init --config "$CONFIG" --webhook "$WEBHOOK" >/dev/null
    echo "Wrote: $CONFIG"
  fi
fi

echo "Ensure ~/.local/bin is on your PATH, then: bark hello 'first bark'"

# Claude Code notifications. `ensure` respects an earlier --disable, so a
# personalize run never turns your pings back on behind your back.
cc_mod_shim bark-notify "$@"
