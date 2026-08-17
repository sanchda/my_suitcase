#!/bin/bash
set -e

GH_VERSION="2.75.1"
GH_ARCHIVE="gh_${GH_VERSION}_linux_amd64.tar.gz"
GH_URL="https://github.com/cli/cli/releases/download/v${GH_VERSION}/${GH_ARCHIVE}"
EXTRACT_DIR="gh_${GH_VERSION}_linux_amd64"

TMP_DIR=$(mktemp -d /tmp/gh-install.XXXXXX)
echo "Using temporary directory: $TMP_DIR"
cd $TMP_DIR

echo "Downloading GitHub CLI v${GH_VERSION}..."
curl -LO $GH_URL

echo "Extracting archive..."
tar xzf $GH_ARCHIVE

echo "Installing GitHub CLI to /usr/local..."

sudo mkdir -p /usr/local/bin
sudo cp $EXTRACT_DIR/bin/gh /usr/local/bin/

cd
if [[ "$TMP_DIR" == /tmp/gh-install.* && -d "$TMP_DIR" ]]; then
    echo "Cleaning up temporary directory: $TMP_DIR"
    rm -rf "$TMP_DIR"
else
    echo "Warning: Temporary directory not removed: $TMP_DIR"
fi

which gh
gh --version | head -n1