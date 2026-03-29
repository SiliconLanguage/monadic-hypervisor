#!/usr/bin/env bash

set -euo pipefail

# Pulls .github/ content from the private copilot-customizations repo
# into the local dataplane-emu workspace.
# Run this after making changes directly in the private repo clone.
#
# Environment variables:
#   PRIVATE_AGENT_REPO    - path to local clone of private repo
#   PRIVATE_TARGET_SUBDIR - subdirectory inside private repo to pull from (default: .github)

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEST_DIR="$ROOT_DIR/.github"

if [[ -n "${PRIVATE_AGENT_REPO:-}" ]]; then
  PRIVATE_REPO="$PRIVATE_AGENT_REPO"
elif [[ -d "$HOME/copilot-customizations/.git" ]]; then
  PRIVATE_REPO="$HOME/copilot-customizations"
elif [[ -d "/tmp/copilot-customizations/.git" ]]; then
  PRIVATE_REPO="/tmp/copilot-customizations"
else
  echo "Private repo clone not found. Set PRIVATE_AGENT_REPO or clone to ~/copilot-customizations." >&2
  exit 1
fi

PRIVATE_TARGET_SUBDIR="${PRIVATE_TARGET_SUBDIR:-.github}"
SOURCE_DIR="$PRIVATE_REPO/$PRIVATE_TARGET_SUBDIR"

if [[ ! -d "$SOURCE_DIR" ]]; then
  echo "Source directory not found: $SOURCE_DIR" >&2
  exit 1
fi

# Pull latest from remote first
git -C "$PRIVATE_REPO" pull --ff-only origin main

rsync -a --delete "$SOURCE_DIR/" "$DEST_DIR/"

echo "Pulled .github/ from $PRIVATE_REPO into $DEST_DIR"
echo "Review changes with: git -C $ROOT_DIR diff .github/"
