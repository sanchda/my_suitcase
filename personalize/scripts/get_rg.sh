#!/bin/bash
set -euo pipefail

# Check if ripgrep is already installed
if command -v rg &> /dev/null; then
    echo "ripgrep is already installed!"
    rg --version | head -n1
    exit 0
fi

# -----------------------
# Config (overrideable)
# -----------------------
RG_VERSION="${RG_VERSION:-14.1.1}"
PREFIX="${PREFIX:-/usr/local}"

# -----------------------
# Detect OS / ARCH
# -----------------------
OS="$(uname -s)"
MACHINE="$(uname -m)"

case "$OS" in
    Linux)  PLATFORM="linux" ;;
    Darwin) PLATFORM="macos" ;;
    *)
        echo "Unsupported OS: $OS"
        exit 1
        ;;
esac

case "$MACHINE" in
    x86_64|amd64)
        ARCH="x86_64"
        ;;
    arm64|aarch64)
        ARCH="aarch64"
        ;;
    *)
        echo "Unsupported architecture: $MACHINE"
        exit 1
        ;;
esac

# -----------------------
# Choose asset target
# -----------------------
TARGET=""
CANDIDATES=()

if [[ "$PLATFORM" == "linux" ]]; then
    # Prefer musl, fall back to gnu
    CANDIDATES=(
        "${ARCH}-unknown-linux-musl"
        "${ARCH}-unknown-linux-gnu"
    )
elif [[ "$PLATFORM" == "macos" ]]; then
    CANDIDATES=(
        "${ARCH}-apple-darwin"
    )
fi

# -----------------------
# Temp dir + cleanup
# -----------------------
TMP_DIR="$(mktemp -d 2>/dev/null || mktemp -d -t ripgrep-install)"
cleanup() {
    if [[ -n "${TMP_DIR:-}" && -d "$TMP_DIR" ]]; then
        rm -rf "$TMP_DIR"
    fi
}
trap cleanup EXIT

echo "Using temporary directory: $TMP_DIR"
cd "$TMP_DIR"

# -----------------------
# Download the right archive
# -----------------------
download_ok=false
for candidate in "${CANDIDATES[@]}"; do
    RG_ARCHIVE="ripgrep-${RG_VERSION}-${candidate}.tar.gz"
    RG_URL="https://github.com/BurntSushi/ripgrep/releases/download/${RG_VERSION}/${RG_ARCHIVE}"

    echo "Trying to download: $RG_ARCHIVE"
    if curl -fL -o "$RG_ARCHIVE" "$RG_URL"; then
        TARGET="$candidate"
        download_ok=true
        break
    fi
done

if [[ "$download_ok" != true ]]; then
    echo "Failed to download a compatible ripgrep archive for:"
    echo "  OS:   $OS"
    echo "  ARCH: $MACHINE"
    echo "  Version: $RG_VERSION"
    exit 1
fi

EXTRACT_DIR="ripgrep-${RG_VERSION}-${TARGET}"

echo "Extracting archive..."
tar xzf "$RG_ARCHIVE"

# -----------------------
# Install
# -----------------------
echo "Installing ripgrep to $PREFIX..."

sudo mkdir -p "$PREFIX/bin"
sudo mkdir -p "$PREFIX/share/man/man1"
sudo mkdir -p "$PREFIX/share/doc/ripgrep"
sudo mkdir -p "$PREFIX/share/bash-completion/completions"

# Install binary
sudo cp "$EXTRACT_DIR/rg" "$PREFIX/bin/"

# Install documentation
if [[ -d "$EXTRACT_DIR/doc" ]]; then
    sudo cp "$EXTRACT_DIR/doc/rg.1" "$PREFIX/share/man/man1/" || true
    sudo cp "$EXTRACT_DIR/doc/"*.md "$PREFIX/share/doc/ripgrep/" 2>/dev/null || true
fi
sudo cp "$EXTRACT_DIR/"*.md "$PREFIX/share/doc/ripgrep/" 2>/dev/null || true
sudo cp "$EXTRACT_DIR/COPYING" "$PREFIX/share/doc/ripgrep/" 2>/dev/null || true
sudo cp "$EXTRACT_DIR/UNLICENSE" "$PREFIX/share/doc/ripgrep/" 2>/dev/null || true
sudo cp "$EXTRACT_DIR/LICENSE-MIT" "$PREFIX/share/doc/ripgrep/" 2>/dev/null || true

# Install bash completion (if present in this build)
if [[ -f "$EXTRACT_DIR/complete/rg.bash" ]]; then
    sudo cp "$EXTRACT_DIR/complete/rg.bash" \
        "$PREFIX/share/bash-completion/completions/rg"
fi

# -----------------------
# Verify
# -----------------------
command -v rg
rg --version | head -n1
