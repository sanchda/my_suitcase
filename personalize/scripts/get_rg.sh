#!/bin/bash
set -e

# Check if ripgrep is already installed
if command -v rg &> /dev/null; then
    echo "ripgrep is already installed!"
    rg --version | head -n1
    exit 0
fi

# Variables
RG_VERSION="14.1.1"
RG_ARCHIVE="ripgrep-$RG_VERSION-x86_64-unknown-linux-musl.tar.gz"
RG_URL="https://github.com/BurntSushi/ripgrep/releases/download/$RG_VERSION/$RG_ARCHIVE"
EXTRACT_DIR="ripgrep-$RG_VERSION-x86_64-unknown-linux-musl"

# Create temporary directory
TMP_DIR=$(mktemp -d /tmp/ripgrep-install.XXXXXX)
echo "Using temporary directory: $TMP_DIR"
cd $TMP_DIR

echo "Downloading ripgrep $RG_VERSION..."
curl -LO $RG_URL

echo "Extracting archive..."
tar xzf $RG_ARCHIVE

echo "Installing ripgrep to /usr/local..."

# NB: mkdir -p for safety
sudo mkdir -p /usr/local/bin
sudo mkdir -p /usr/local/share/man/man1
sudo mkdir -p /usr/local/share/doc/ripgrep
sudo mkdir -p /usr/local/share/bash-completion/completions

# Install binary
sudo cp $EXTRACT_DIR/rg /usr/local/bin/

# Install documentation
sudo cp $EXTRACT_DIR/doc/rg.1 /usr/local/share/man/man1/
sudo cp $EXTRACT_DIR/doc/*.md /usr/local/share/doc/ripgrep/
sudo cp $EXTRACT_DIR/*.md /usr/local/share/doc/ripgrep/
sudo cp $EXTRACT_DIR/COPYING /usr/local/share/doc/ripgrep/
sudo cp $EXTRACT_DIR/UNLICENSE /usr/local/share/doc/ripgrep/
sudo cp $EXTRACT_DIR/LICENSE-MIT /usr/local/share/doc/ripgrep/

# Install shell completions
sudo cp $EXTRACT_DIR/complete/rg.bash /usr/local/share/bash-completion/completions/rg

# Clean up temporary directory
cd
if [[ "$TMP_DIR" == /tmp/ripgrep-install.* && -d "$TMP_DIR" ]]; then
    echo "Cleaning up temporary directory: $TMP_DIR"
    rm -rf "$TMP_DIR"
else
    echo "Warning: Temporary directory not removed: $TMP_DIR"
fi

# Check installation
which rg
rg --version | head -n1
