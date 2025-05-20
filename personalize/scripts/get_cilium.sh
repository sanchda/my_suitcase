#!/bin/bash
set -e  # Exit immediately if a command exits with a non-zero status

# Check if cilium is already installed
if command -v cilium &> /dev/null; then
    echo "Cilium CLI is already installed!"
    cilium version
    exit 0
fi

# Create a temporary directory
TMP_DIR=$(mktemp -d /tmp/cilium-install.XXXXXX)
echo "Using temporary directory: $TMP_DIR"
cd "$TMP_DIR"

# Get the latest stable version of Cilium CLI
CILIUM_CLI_VERSION=$(curl -s https://raw.githubusercontent.com/cilium/cilium-cli/main/stable.txt)
echo "Installing Cilium CLI version: $CILIUM_CLI_VERSION"

# Detect architecture
CLI_ARCH=amd64
if [ "$(uname -m)" = "aarch64" ]; then
    CLI_ARCH=arm64
fi
echo "Detected architecture: $CLI_ARCH"

echo "Downloading Cilium CLI..."
curl -L --fail --remote-name-all https://github.com/cilium/cilium-cli/releases/download/${CILIUM_CLI_VERSION}/cilium-linux-${CLI_ARCH}.tar.gz{,.sha256sum}

echo "Verifying checksum..."
sha256sum --check cilium-linux-${CLI_ARCH}.tar.gz.sha256sum

echo "Installing Cilium CLI to /usr/local/bin..."
sudo tar xzvfC cilium-linux-${CLI_ARCH}.tar.gz /usr/local/bin

echo "Cilium CLI has been successfully installed!"

# Clean up
cd
if [[ "$TMP_DIR" == /tmp/cilium-install.* && -d "$TMP_DIR" ]]; then
    echo "Cleaning up temporary directory: $TMP_DIR"
    rm -rf "$TMP_DIR"
else
    echo "Warning: Temporary directory not removed: $TMP_DIR"
fi

# Check installation
which cilium
cilium version

