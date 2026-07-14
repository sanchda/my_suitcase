#!/bin/bash
set -e

# Check if atuin is already installed
if command -v atuin &> /dev/null; then
    echo "atuin is already installed!"
    atuin --version | head -n1
    exit 0
fi

# Variables
ATUIN_VERSION="18.16.0"

# Pick the cargo-dist target triple matching this host. Note: atuin upstream
# does NOT publish an x86_64-apple-darwin (Intel Mac) build — fall through to
# brew there.
os=$(uname -s)
arch=$(uname -m)
case "$os/$arch" in
    Linux/x86_64)              ATUIN_TARGET="x86_64-unknown-linux-musl" ;;
    Linux/aarch64|Linux/arm64) ATUIN_TARGET="aarch64-unknown-linux-musl" ;;
    Darwin/arm64)              ATUIN_TARGET="aarch64-apple-darwin" ;;
    Darwin/x86_64)
        echo "atuin upstream does not publish an Intel-Mac binary." >&2
        echo "Install via Homebrew instead: brew install atuin" >&2
        exit 1
        ;;
    *)
        echo "Unsupported OS/arch for atuin install: $os/$arch" >&2
        exit 1
        ;;
esac

ATUIN_ARCHIVE="atuin-${ATUIN_TARGET}.tar.gz"
ATUIN_URL="https://github.com/atuinsh/atuin/releases/download/v${ATUIN_VERSION}/${ATUIN_ARCHIVE}"
EXTRACT_DIR="atuin-${ATUIN_TARGET}"

# Create temporary directory
TMP_DIR=$(mktemp -d /tmp/atuin-install.XXXXXX)
echo "Using temporary directory: $TMP_DIR"
cd "$TMP_DIR"

echo "Downloading atuin v${ATUIN_VERSION}..."
curl -LO "$ATUIN_URL"

echo "Extracting archive..."
tar xzf "$ATUIN_ARCHIVE"

echo "Installing atuin to /usr/local..."

sudo mkdir -p /usr/local/bin

# Install binary (cargo-dist layout: binary at root of extracted dir)
sudo cp "$EXTRACT_DIR/atuin" /usr/local/bin/

# Clean up temporary directory
cd
if [[ "$TMP_DIR" == /tmp/atuin-install.* && -d "$TMP_DIR" ]]; then
    echo "Cleaning up temporary directory: $TMP_DIR"
    rm -rf "$TMP_DIR"
else
    echo "Warning: Temporary directory not removed: $TMP_DIR"
fi

# Check installation
which atuin
atuin --version | head -n1

cat <<'EOF'

------------------------------------------------------------
atuin binary installed. To finish setup:

  1. Add shell init to your bashrc/zshrc (e.g. in suitcase):
       command -v atuin >/dev/null && eval "$(atuin init bash)"
       command -v atuin >/dev/null && eval "$(atuin init zsh)"

  2. Initialize the local DB:
       atuin login    # only if using sync; otherwise skip
       atuin import auto

  3. (Optional) Stop sharing HISTFILE between bash and zsh in
     your bashrc — atuin is now the source of truth.
------------------------------------------------------------
EOF
