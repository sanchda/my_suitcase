#!/bin/bash
set -e

if command -v bpftrace &> /dev/null; then
    echo "bpftrace is already installed!"
    bpftrace --version
    exit 0
fi

BPFTRACE_VERSION="0.23.2"
BPFTRACE_URL="https://github.com/bpftrace/bpftrace/releases/download/v${BPFTRACE_VERSION}/bpftrace"

TMP_DIR=$(mktemp -d /tmp/bpftrace-install.XXXXXX)
echo "Using temporary directory: $TMP_DIR"
cd $TMP_DIR

echo "Downloading bpftrace v${BPFTRACE_VERSION}..."
curl -LO $BPFTRACE_URL

echo "Installing bpftrace to /usr/local/bin..."

sudo mkdir -p /usr/local/bin
sudo cp bpftrace /usr/local/bin/
sudo chmod +x /usr/local/bin/bpftrace

cd
if [[ "$TMP_DIR" == /tmp/bpftrace-install.* && -d "$TMP_DIR" ]]; then
    echo "Cleaning up temporary directory: $TMP_DIR"
    rm -rf "$TMP_DIR"
else
    echo "Warning: Temporary directory not removed: $TMP_DIR"
fi

which bpftrace
bpftrace --version

