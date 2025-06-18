#!/bin/bash
nix-channel --update && \
    nix-env -iA nixpkgs.claude-code
