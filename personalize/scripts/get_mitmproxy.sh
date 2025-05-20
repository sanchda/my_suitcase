#!/bin/bash
set -e

# Check if mitmproxy is already installed
if command -v mitmproxy &> /dev/null; then
    echo "mitmproxy is already installed!"
    mitmproxy --version
    exit 0
fi

# Variables
MITM_VERSION="12.0.1"
MITM_ARCHIVE="mitmproxy-$MITM_VERSION-linux-x86_64.tar.gz"
MITM_URL="https://downloads.mitmproxy.org/$MITM_VERSION/$MITM_ARCHIVE"

# Create temporary directory
TMP_DIR=$(mktemp -d /tmp/mitmproxy-install.XXXXXX)
echo "Using temporary directory: $TMP_DIR"
cd $TMP_DIR

echo "Downloading mitmproxy $MITM_VERSION..."
curl -LO $MITM_URL

echo "Extracting archive..."
tar xzf $MITM_ARCHIVE

echo "Installing mitmproxy to /usr/local/bin..."

# Install binaries
sudo mkdir -p /usr/local/bin
sudo cp mitmproxy /usr/local/bin/
sudo cp mitmdump /usr/local/bin/
sudo cp mitmweb /usr/local/bin/
sudo chmod +x /usr/local/bin/mitmproxy
sudo chmod +x /usr/local/bin/mitmdump
sudo chmod +x /usr/local/bin/mitmweb

# Clean up temporary directory
cd
if [[ "$TMP_DIR" == /tmp/mitmproxy-install.* && -d "$TMP_DIR" ]]; then
    echo "Cleaning up temporary directory: $TMP_DIR"
    rm -rf "$TMP_DIR"
else
    echo "Warning: Temporary directory not removed: $TMP_DIR"
fi

# Check installation
which mitmproxy
mitmproxy --version
