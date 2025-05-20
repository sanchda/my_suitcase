#!/bin/bash
set -e  # Exit immediately if a command exits with a non-zero status

# Check if hubble is already installed
if command -v hubble &> /dev/null; then
    echo "Hubble is already installed!"
    hubble version
    exit 0
fi

# Variables
HUBBLE_VERSION="1.17.3"
HUBBLE_ARCHIVE="hubble-linux-amd64.tar.gz"
HUBBLE_URL="https://github.com/cilium/hubble/releases/download/v$HUBBLE_VERSION/$HUBBLE_ARCHIVE"

# Create a temporary directory
TMP_DIR=$(mktemp -d /tmp/hubble-install.XXXXXX)
echo "Using temporary directory: $TMP_DIR"
cd "$TMP_DIR"

echo "Downloading Hubble..."
curl -LO $HUBBLE_URL

echo "Extracting archive..."
tar xzf $HUBBLE_ARCHIVE

# Create directory if it doesn't exist
echo "Installing Hubble to /usr/local/bin..."
sudo mkdir -p /usr/local/bin

# Copy binary to destination
sudo cp hubble /usr/local/bin/
sudo chmod +x /usr/local/bin/hubble

echo "Hubble has been successfully installed!"

# Clean up temporary directory
cd
if [[ "$TMP_DIR" == /tmp/hubble-install.* && -d "$TMP_DIR" ]]; then
    echo "Cleaning up temporary directory: $TMP_DIR"
    rm -rf "$TMP_DIR"
else
    echo "Warning: Temporary directory not removed: $TMP_DIR"
fi

# Check installation
which hubble
hubble version

