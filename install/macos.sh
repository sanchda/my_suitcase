#!/bin/bash
# macOS-specific setup: Homebrew and GNU coreutils

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/common.sh"

if [ "$SC_OS" != "darwin" ]; then
  echo "Not macOS, skipping."
  exit 0
fi

# Ensure Homebrew is installed
if ! command -v brew &>/dev/null; then
  echo "Homebrew not found. Installing..."
  /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
fi

# GNU core utilities for Linux-like behavior
brew install coreutils findutils gnu-tar gnu-sed gawk grep

echo "macOS setup complete."
