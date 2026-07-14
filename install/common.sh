#!/bin/bash
# Shared installer utilities for suitcase modules

HEADER="#DAVEGEN_SC"

# Resolve SUITCASE to the repo root (parent of install/)
if [ -z "$SUITCASE" ]; then
  SUITCASE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
fi
export SUITCASE

# Backup directory — one per install run
_BAKDIR=""
_bakdir() {
  if [ -z "$_BAKDIR" ]; then
    _BAKDIR="${HOME}/dotbak/SCB_$(head -c 8 /dev/urandom | od -An -tx1 | tr -d ' \n')"
  fi
  echo "$_BAKDIR"
}

# backup_if_needed <file>
#   - If file has the suitcase header, delete it
#   - If file exists without header, back it up
#   - If file doesn't exist, do nothing
backup_if_needed() {
  local file="$1"
  if [ ! -e "$file" ]; then
    return 0
  fi
  if [ -f "$file" ] && [ "$(head -n 1 "$file")" = "$HEADER" ]; then
    echo "Suitcase-generated $file detected. Removing."
    rm -f "$file"
  else
    local bakdir
    bakdir="$(_bakdir)"
    echo "Non-suitcase $file detected. Backing up to $bakdir"
    mkdir -p "$bakdir"
    mv "$file" "$bakdir/"
  fi
}

# detect_os — sets SC_OS to darwin, wsl, or linux
detect_os() {
  case "$(uname -s)" in
    Darwin)
      SC_OS="darwin"
      ;;
    Linux)
      if [ -f /proc/version ] && grep -qi microsoft /proc/version; then
        SC_OS="wsl"
      else
        SC_OS="linux"
      fi
      ;;
    *)
      SC_OS="linux"
      ;;
  esac
  export SC_OS
}

detect_os
