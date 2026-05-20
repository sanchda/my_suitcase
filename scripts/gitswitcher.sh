#!/usr/bin/env bash
# Usage: ./git-switch-remote.sh [personal|work]

if [ $# -ne 1 ]; then
  echo "Usage: $0 [personal|work]"
  exit 1
fi

MODE=$1

if [ "$MODE" != "personal" ] && [ "$MODE" != "work" ]; then
  echo "Error: Mode must be either 'personal' or 'work'"
  exit 1
fi

# Get current remote URL and check
CURRENT_URL=$(git config --get remote.origin.url)
if [ -z "$CURRENT_URL" ]; then
  echo "Error: No origin remote found"
  exit 1
fi

# Extract the repository path (org/repo.git)
if [[ $CURRENT_URL == *"@"* ]]; then
  REPO_PATH=$(echo $CURRENT_URL | sed 's/.*:\(.*\)/\1/')
else
  REPO_PATH=$(echo $CURRENT_URL | sed 's/https:\/\/github.com\/\(.*\)/\1/')
fi

# Update
NEW_URL="git@github.com-$MODE:$REPO_PATH"
git remote set-url origin "$NEW_URL"

echo "Remote origin updated to use $MODE configuration:"
echo "  $NEW_URL"
