#!/bin/bash
set -e

if command -v hubble &> /dev/null; then
    echo "Hubble is already installed!"
    hubble version
    exit 0
fi

HUBBLE_VERSION="1.17.3"
HUBBLE_ARCHIVE="hubble-linux-amd64.tar.gz"
HUBBLE_URL="https://github.com/cilium/hubble/releases/download/v$HUBBLE_VERSION/$HUBBLE_ARCHIVE"

TMP_DIR=$(mktemp -d /tmp/hubble-install.XXXXXX)
echo "Using temporary directory: $TMP_DIR"
cd "$TMP_DIR"

echo "Downloading Hubble..."
curl -LO $HUBBLE_URL

echo "Extracting archive..."
tar xzf $HUBBLE_ARCHIVE

echo "Installing Hubble to /usr/local/bin..."
sudo mkdir -p /usr/local/bin

sudo cp hubble /usr/local/bin/
sudo chmod +x /usr/local/bin/hubble

echo "Hubble has been successfully installed!"

cd
if [[ "$TMP_DIR" == /tmp/hubble-install.* && -d "$TMP_DIR" ]]; then
    echo "Cleaning up temporary directory: $TMP_DIR"
    rm -rf "$TMP_DIR"
else
    echo "Warning: Temporary directory not removed: $TMP_DIR"
fi

which hubble
hubble version

