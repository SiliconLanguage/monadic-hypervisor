#!/usr/bin/env bash

set -euo pipefail

# Mirrors everything under .github/ into the private copilot-customizations repo.
# Never pushes to the main dataplane-emu repository.
#
# Environment variables:
#   PRIVATE_AGENT_REPO    - path to local clone of private repo (auto-cloned if missing)
#   PRIVATE_AGENT_REMOTE  - remote URL for private repo (used when auto-cloning)
#   PRIVATE_TARGET_SUBDIR - subdirectory inside private repo to mirror into (default: .github)

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SOURCE_DIR="$ROOT_DIR/.github"

if [[ ! -d "$SOURCE_DIR" ]]; then
  echo "Missing source directory: $SOURCE_DIR" >&2
  exit 1
fi

# ── Mirror .github/ into the private copilot-customizations repo ──────────────
if [[ -n "${PRIVATE_AGENT_REPO:-}" ]]; then
  PRIVATE_REPO="$PRIVATE_AGENT_REPO"
elif [[ -d "/tmp/copilot-customizations/.git" ]]; then
  PRIVATE_REPO="/tmp/copilot-customizations"
else
  PRIVATE_REPO="$HOME/copilot-customizations"
fi

PRIVATE_AGENT_REMOTE="${PRIVATE_AGENT_REMOTE:-https://github.com/ping-long-github/copilot-customizations.git}"
PRIVATE_TARGET_SUBDIR="${PRIVATE_TARGET_SUBDIR:-.github}"
PRIVATE_DEST_DIR="$PRIVATE_REPO/$PRIVATE_TARGET_SUBDIR"

if [[ ! -d "$PRIVATE_REPO/.git" ]]; then
  if [[ -e "$PRIVATE_REPO" && ! -d "$PRIVATE_REPO/.git" ]]; then
    echo "Path exists but is not a git repo: $PRIVATE_REPO" >&2
    echo "Set PRIVATE_AGENT_REPO to a clean clone path." >&2
    exit 1
  fi
  echo "Private repo missing. Cloning $PRIVATE_AGENT_REMOTE into $PRIVATE_REPO"
  git clone "$PRIVATE_AGENT_REMOTE" "$PRIVATE_REPO"
fi

mkdir -p "$PRIVATE_DEST_DIR"
rsync -a --delete "$SOURCE_DIR/" "$PRIVATE_DEST_DIR/"

# Also sync the two helper scripts themselves
cp "$ROOT_DIR/push-private-configs.sh" "$PRIVATE_REPO/push-private-configs.sh"
cp "$ROOT_DIR/pull-private-configs.sh" "$PRIVATE_REPO/pull-private-configs.sh"

cd "$PRIVATE_REPO"
git add "$PRIVATE_TARGET_SUBDIR" push-private-configs.sh pull-private-configs.sh

if git diff --cached --quiet; then
  echo "No changes. Private repo already up to date."
  exit 0
fi

CHANGED_FILES=$(git diff --cached --name-only | wc -l)
git commit -m "chore: sync .github customizations and scripts ($CHANGED_FILES file(s) changed)"
git push origin main

echo "Published $CHANGED_FILES file(s) to $PRIVATE_REPO"
