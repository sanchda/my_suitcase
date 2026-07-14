#!/bin/bash
# Install atuin config symlink: ~/.config/atuin/config.toml -> suitcase

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/common.sh"

src="$SUITCASE/atuin/config.toml"
dst_dir="$HOME/.config/atuin"
dst="$dst_dir/config.toml"

if [ ! -f "$src" ]; then
  echo "No atuin/config.toml in suitcase, skipping."
  exit 0
fi

mkdir -p "$dst_dir"

if [ -L "$dst" ]; then
  if [ "$(readlink "$dst")" = "$src" ]; then
    echo "$dst already points at suitcase, skipping."
    exit 0
  fi
  echo "Existing symlink at $dst pointing elsewhere. Removing."
  rm -f "$dst"
elif [ -e "$dst" ]; then
  bakdir="$(_bakdir)"
  echo "Non-suitcase $dst detected. Backing up to $bakdir"
  mkdir -p "$bakdir"
  mv "$dst" "$bakdir/"
fi

ln -s "$src" "$dst"
echo "Installed $dst -> $src"
