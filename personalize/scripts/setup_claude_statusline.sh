#!/bin/bash
# Configure the Claude Code status line to show live usage/cost via ccusage.
#
# Merges ONLY the statusLine key into ~/.claude/settings.json, so any existing
# per-machine config (model, enabledPlugins, ...) is preserved. Idempotent —
# safe to re-run. The status line shells out to `npx ccusage`, so the box needs
# Node.js at runtime.
set -e

settings_dir="$HOME/.claude"
settings="$settings_dir/settings.json"

# The statusLine config we want present.
read -r -d '' FRAG <<'JSON' || true
{
  "statusLine": {
    "type": "command",
    "command": "npx -y ccusage statusline"
  }
}
JSON

if ! command -v jq >/dev/null 2>&1; then
  echo "jq is required to merge Claude settings. Install jq first." >&2
  exit 1
fi

if ! command -v npx >/dev/null 2>&1; then
  echo "Warning: npx (Node.js) not found. ccusage runs via npx at runtime —" >&2
  echo "install Node.js or the status line will error." >&2
fi

mkdir -p "$settings_dir"

# Load existing settings (or {}). If the file exists but is invalid JSON, back
# it up rather than clobber it, then start from an empty object.
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

# Deep-merge: fragment wins on the statusLine key; all other keys preserved.
merged="$(printf '%s\n%s\n' "$base" "$FRAG" | jq -s '.[0] * .[1]')"

tmp="$(mktemp)"
printf '%s\n' "$merged" > "$tmp"
mv "$tmp" "$settings"

echo "Claude status line configured in $settings"
echo "Restart Claude Code (or start a new session) to see it."
