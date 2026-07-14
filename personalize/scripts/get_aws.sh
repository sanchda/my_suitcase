#!/bin/bash
set -e

# Check if aws is already installed
if command -v aws &> /dev/null; then
    echo "AWS CLI is already installed!"
    aws --version
    exit 0
fi

# Variables
AWS_ARCHIVE="awscli-exe-linux-x86_64.zip"
AWS_URL="https://awscli.amazonaws.com/${AWS_ARCHIVE}"

# Create temporary directory
TMP_DIR=$(mktemp -d /tmp/awscli-install.XXXXXX)
echo "Using temporary directory: $TMP_DIR"
cd $TMP_DIR

echo "Downloading AWS CLI..."
curl -L "$AWS_URL" -o "$AWS_ARCHIVE"

echo "Extracting archive..."
unzip -q "$AWS_ARCHIVE"

echo "Installing AWS CLI to /usr/local..."
sudo ./aws/install

# Clean up temporary directory
cd
if [[ "$TMP_DIR" == /tmp/awscli-install.* && -d "$TMP_DIR" ]]; then
    echo "Cleaning up temporary directory: $TMP_DIR"
    rm -rf "$TMP_DIR"
else
    echo "Warning: Temporary directory not removed: $TMP_DIR"
fi

# Check installation
which aws
aws --version
