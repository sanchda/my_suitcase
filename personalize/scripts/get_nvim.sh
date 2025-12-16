#!/bin/bash
set -euo pipefail

# Check if nvim is already installed
if command -v nvim &> /dev/null; then
  echo "Neovim is already installed!"
  nvim --version | head -n1
  exit 0
fi

# ===== Config =====
NVIM_VERSION="${NVIM_VERSION:-0.11.5}"

OS="$(uname -s)"
ARCH="$(uname -m)"

# Normalize ARCH for Linux ARM
# - macOS reports arm64
# - many Linux distros report aarch64
if [[ "$ARCH" == "aarch64" ]]; then
  ARCH="arm64"
fi

# Pick archive based on OS/ARCH
case "$OS" in
  Darwin)
    case "$ARCH" in
      x86_64)
        NVIM_ARCHIVE="nvim-macos-x86_64.tar.gz"
        ;;
      arm64)
        NVIM_ARCHIVE="nvim-macos-arm64.tar.gz"
        ;;
      *)
        echo "Unsupported macOS architecture: $ARCH"
        exit 1
        ;;
    esac
    ;;
  Linux)
    case "$ARCH" in
      x86_64)
        NVIM_ARCHIVE="nvim-linux-x86_64.tar.gz"
        ;;
      arm64)
        NVIM_ARCHIVE="nvim-linux-arm64.tar.gz"
        ;;
      *)
        echo "Unsupported Linux architecture: $ARCH"
        exit 1
        ;;
    esac
    ;;
  *)
    echo "Unsupported OS: $OS"
    exit 1
    ;;
esac

NVIM_URL="https://github.com/neovim/neovim/releases/download/v${NVIM_VERSION}/${NVIM_ARCHIVE}"
EXTRACT_DIR="${NVIM_ARCHIVE%.tar.gz}"

# Create a temporary directory
TMP_DIR="$(mktemp -d /tmp/neovim-install.XXXXXX)"
echo "Using temporary directory: $TMP_DIR"
cd "$TMP_DIR"

echo "Downloading Neovim ${NVIM_VERSION} for ${OS}/${ARCH}..."
curl -fL -o "$NVIM_ARCHIVE" "$NVIM_URL"

# On macOS, clear quarantine attribute to reduce "unknown developer" friction.
# The Releases page suggests using xattr on the downloaded archive. :contentReference[oaicite:2]{index=2}
if [[ "$OS" == "Darwin" ]]; then
  xattr -c "$NVIM_ARCHIVE" 2>/dev/null || true
fi

echo "Extracting archive..."
tar xzf "$NVIM_ARCHIVE"

if [[ ! -d "$EXTRACT_DIR" ]]; then
  echo "Expected extracted directory '$EXTRACT_DIR' not found."
  ls -la
  exit 1
fi

echo "Installing Neovim to /usr/local..."
sudo mkdir -p /usr/local/bin /usr/local/lib /usr/local/share

# Copy files to their corresponding locations
sudo cp -R "${EXTRACT_DIR}/bin/"* /usr/local/bin/
sudo cp -R "${EXTRACT_DIR}/lib/"* /usr/local/lib/
sudo cp -R "${EXTRACT_DIR}/share/"* /usr/local/share/

echo "Neovim has been successfully installed!"
echo "You can now run nvim to start Neovim."

# Clean up temporary directory
cd /
if [[ "$TMP_DIR" == /tmp/neovim-install.* && -d "$TMP_DIR" ]]; then
  echo "Cleaning up temporary directory: $TMP_DIR"
  rm -rf "$TMP_DIR"
else
  echo "Warning: Temporary directory not removed: $TMP_DIR"
fi

# Check installation
command -v nvim
nvim --version | head -n1
