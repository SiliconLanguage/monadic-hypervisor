#!/bin/bash
set -euo pipefail

# Match existing AWS workflow that reads variables from repo-root .env.
REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
ENV_FILE="$REPO_ROOT/.env"

if [ ! -f "$ENV_FILE" ]; then
    echo "Missing $ENV_FILE"
    echo "Create it with INSTANCE_ID and AWS_REGION (same variables used by start-graviton.ps1)."
    exit 1
fi

# shellcheck disable=SC1090
source "$ENV_FILE"

if [ -z "${INSTANCE_ID:-}" ] || [ -z "${AWS_REGION:-}" ]; then
    echo "INSTANCE_ID and AWS_REGION must be set in $ENV_FILE"
    exit 1
fi

echo "Stopping AWS instance $INSTANCE_ID in region $AWS_REGION..."
aws ec2 stop-instances --instance-ids "$INSTANCE_ID" --region "$AWS_REGION" >/dev/null
aws ec2 wait instance-stopped --instance-ids "$INSTANCE_ID" --region "$AWS_REGION"

echo "Instance stopped: $INSTANCE_ID"
