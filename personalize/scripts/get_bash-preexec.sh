#!/bin/bash
# bash-preexec is a precondition for atuin's bash integration: atuin appends
# its hooks to precmd_functions/preexec_functions, but those arrays are only
# walked by bash-preexec's DEBUG trap. Without it, atuin records nothing in
# bash and Up-arrow / Ctrl-R show empty results.
#
# Pinned to the released tag so installs are reproducible. Bump as needed.
set -e

DST="${HOME}/.bash-preexec.sh"
BP_VERSION="0.5.0"
BP_URL="https://raw.githubusercontent.com/rcaloras/bash-preexec/${BP_VERSION}/bash-preexec.sh"

if [ -f "$DST" ]; then
    echo "bash-preexec already installed at $DST"
    exit 0
fi

TMP_DIR=$(mktemp -d /tmp/bash-preexec-install.XXXXXX)
cleanup() {
    if [[ "$TMP_DIR" == /tmp/bash-preexec-install.* && -d "$TMP_DIR" ]]; then
        rm -rf "$TMP_DIR"
    fi
}
trap cleanup EXIT

echo "Downloading bash-preexec ${BP_VERSION}..."
curl -fsSL "$BP_URL" -o "$TMP_DIR/bash-preexec.sh"

# Sanity check: the file should define __bp_install (the install entrypoint
# that wires up the DEBUG trap and exposes precmd_functions/preexec_functions).
if ! grep -q '__bp_install' "$TMP_DIR/bash-preexec.sh"; then
    echo "Error: downloaded file does not look like bash-preexec" >&2
    exit 1
fi

install -m 0644 "$TMP_DIR/bash-preexec.sh" "$DST"
echo "Installed bash-preexec to $DST"
