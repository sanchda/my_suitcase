#!/bin/bash
set -e

if command -v aws &> /dev/null; then
    echo "AWS CLI is already installed!"
    aws --version
    exit 0
fi

AWS_ARCHIVE="awscli-exe-linux-x86_64.zip"
AWS_URL="https://awscli.amazonaws.com/${AWS_ARCHIVE}"

TMP_DIR=$(mktemp -d /tmp/awscli-install.XXXXXX)
echo "Using temporary directory: $TMP_DIR"
cd $TMP_DIR

echo "Downloading AWS CLI..."
curl -L "$AWS_URL" -o "$AWS_ARCHIVE"

echo "Extracting archive..."
unzip -q "$AWS_ARCHIVE"

echo "Installing AWS CLI to /usr/local..."
sudo ./aws/install

cd
if [[ "$TMP_DIR" == /tmp/awscli-install.* && -d "$TMP_DIR" ]]; then
    echo "Cleaning up temporary directory: $TMP_DIR"
    rm -rf "$TMP_DIR"
else
    echo "Warning: Temporary directory not removed: $TMP_DIR"
fi

which aws
aws --version
