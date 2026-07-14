#!/bin/bash
set -e  # Exit immediately if a command exits with a non-zero status

# Check if nvim is already installed
if command -v nvim &> /dev/null; then
    echo "Neovim is already installed!"
    nvim --version | head -n1
    exit 0
fi

# Variables
NVIM_VERSION="0.11.1"
NVIM_ARCHIVE="nvim-linux-x86_64.tar.gz"
NVIM_URL="https://github.com/neovim/neovim/releases/download/v$NVIM_VERSION/$NVIM_ARCHIVE"

# Create a temporary directory
TMP_DIR=$(mktemp -d /tmp/neovim-install.XXXXXX)
echo "Using temporary directory: $TMP_DIR"
cd "$TMP_DIR"

echo "Downloading Neovim..."
curl -LO $NVIM_URL

echo "Extracting archive..."
tar xzf $NVIM_ARCHIVE

# Create directories if they don't exist
echo "Installing Neovim to /usr/local..."
sudo mkdir -p /usr/local/bin
sudo mkdir -p /usr/local/lib
sudo mkdir -p /usr/local/share

# Copy files to their corresponding locations
sudo cp -r nvim-linux-x86_64/bin/* /usr/local/bin/
sudo cp -r nvim-linux-x86_64/lib/* /usr/local/lib/
sudo cp -r nvim-linux-x86_64/share/* /usr/local/share/

echo "Neovim has been successfully installed!"
echo "You can now run nvim to start Neovim."

# Clean up temporary directory
cd
if [[ "$TMP_DIR" == /tmp/neovim-install.* && -d "$TMP_DIR" ]]; then
    echo "Cleaning up temporary directory: $TMP_DIR"
    rm -rf "$TMP_DIR"
else
    echo "Warning: Temporary directory not removed: $TMP_DIR"
fi

# Check installation
which nvim
nvim --version | head -n1

