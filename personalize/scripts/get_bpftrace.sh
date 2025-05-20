#!/bin/bash
set -e

# Check if bpftrace is already installed
if command -v bpftrace &> /dev/null; then
    echo "bpftrace is already installed!"
    bpftrace --version
    exit 0
fi

# Variables
BPFTRACE_VERSION="0.23.2"
BPFTRACE_URL="https://github.com/bpftrace/bpftrace/releases/download/v${BPFTRACE_VERSION}/bpftrace"

# Create temporary directory
TMP_DIR=$(mktemp -d /tmp/bpftrace-install.XXXXXX)
echo "Using temporary directory: $TMP_DIR"
cd $TMP_DIR

echo "Downloading bpftrace v${BPFTRACE_VERSION}..."
curl -LO $BPFTRACE_URL

echo "Installing bpftrace to /usr/local/bin..."

# Install binary and make it executable
sudo mkdir -p /usr/local/bin
sudo cp bpftrace /usr/local/bin/
sudo chmod +x /usr/local/bin/bpftrace

# Clean up temporary directory
cd
if [[ "$TMP_DIR" == /tmp/bpftrace-install.* && -d "$TMP_DIR" ]]; then
    echo "Cleaning up temporary directory: $TMP_DIR"
    rm -rf "$TMP_DIR"
else
    echo "Warning: Temporary directory not removed: $TMP_DIR"
fi

# Check installation
which bpftrace
bpftrace --version

