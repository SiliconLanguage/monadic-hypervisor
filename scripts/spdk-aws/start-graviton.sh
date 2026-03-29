#!/usr/bin/env bash
set -euo pipefail

# ============================================================================
# start-graviton.sh — Start an existing stopped Graviton instance and SSH in
# ============================================================================
# Bash equivalent of start-graviton.ps1.
# Loads .env from repo root, starts the instance, waits for running state,
# optionally updates Cloudflare DNS, and opens an SSH session.
#
# Usage:
#   bash scripts/spdk-aws/start-graviton.sh [--no-ssh]
# ============================================================================

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
ENV_FILE="$REPO_ROOT/.env"

# ---------------------------------------------------------------------------
# 0. Load .env
# ---------------------------------------------------------------------------
if [[ ! -f "$ENV_FILE" ]]; then
    echo "Missing $ENV_FILE"
    echo "Create it from the template: cp scripts/spdk-aws/.env.template .env"
    exit 1
fi

set -a
# shellcheck disable=SC1090
source "$ENV_FILE"
set +a

# Required variables
for var in INSTANCE_ID AWS_REGION; do
    if [[ -z "${!var:-}" ]]; then
        echo "ERROR: $var must be set in $ENV_FILE" >&2
        exit 1
    fi
done

# Optional variables (for Cloudflare DNS + SSH)
DEV_SSH_KEY="${DEV_SSH_KEY:-}"
DEV_USER="${DEV_USER:-ec2-user}"
DEV_DOMAIN="${DEV_DOMAIN:-}"
CF_API_TOKEN="${CF_API_TOKEN:-}"
CF_ZONE_ID="${CF_ZONE_ID:-}"

NO_SSH=0
if [[ "${1:-}" == "--no-ssh" ]]; then
    NO_SSH=1
fi

# ---------------------------------------------------------------------------
# 1. Start the instance
# ---------------------------------------------------------------------------
echo "Starting instance $INSTANCE_ID in $AWS_REGION..."
aws ec2 start-instances \
    --instance-ids "$INSTANCE_ID" \
    --region "$AWS_REGION" >/dev/null

echo "Waiting for instance to reach 'running' state..."
aws ec2 wait instance-running \
    --instance-ids "$INSTANCE_ID" \
    --region "$AWS_REGION"

# ---------------------------------------------------------------------------
# 2. Fetch public IP
# ---------------------------------------------------------------------------
IP=$(aws ec2 describe-instances \
    --instance-ids "$INSTANCE_ID" \
    --region "$AWS_REGION" \
    --query 'Reservations[0].Instances[0].PublicIpAddress' \
    --output text)

if [[ -z "$IP" || "$IP" == "None" ]]; then
    echo "ERROR: Could not get a public IP. Is the instance fully running?" >&2
    exit 1
fi

echo "Instance running. Public IP: $IP"

# ---------------------------------------------------------------------------
# 3. Update Cloudflare DNS (optional — skipped if tokens not set)
# ---------------------------------------------------------------------------
if [[ -n "$CF_API_TOKEN" && -n "$CF_ZONE_ID" && -n "$DEV_DOMAIN" ]]; then
    echo "Updating Cloudflare DNS: $DEV_DOMAIN → $IP"

    RECORD_ID=$(curl -sf \
        -H "Authorization: Bearer $CF_API_TOKEN" \
        -H "Content-Type: application/json" \
        "https://api.cloudflare.com/client/v4/zones/$CF_ZONE_ID/dns_records?name=$DEV_DOMAIN" \
        | python3 -c "import sys,json; r=json.load(sys.stdin)['result']; print(r[0]['id'] if r else '')" 2>/dev/null || true)

    if [[ -n "$RECORD_ID" ]]; then
        curl -sf -X PUT \
            -H "Authorization: Bearer $CF_API_TOKEN" \
            -H "Content-Type: application/json" \
            -d "{\"type\":\"A\",\"name\":\"$DEV_DOMAIN\",\"content\":\"$IP\",\"ttl\":60,\"proxied\":false}" \
            "https://api.cloudflare.com/client/v4/zones/$CF_ZONE_ID/dns_records/$RECORD_ID" >/dev/null \
            && echo "DNS updated: $DEV_DOMAIN → $IP" \
            || echo "WARNING: Cloudflare DNS update failed (non-fatal)" >&2
    else
        echo "WARNING: Could not find DNS record for $DEV_DOMAIN (skipping)" >&2
    fi
else
    echo "(Cloudflare DNS update skipped — set CF_API_TOKEN, CF_ZONE_ID, DEV_DOMAIN in .env to enable)"
fi

# ---------------------------------------------------------------------------
# 4. SSH into the instance
# ---------------------------------------------------------------------------
if [[ "$NO_SSH" -eq 1 ]]; then
    echo "Done. Connect manually: ssh ${DEV_SSH_KEY:+-i $DEV_SSH_KEY }${DEV_USER}@${IP}"
    exit 0
fi

SSH_ARGS=(-o StrictHostKeyChecking=accept-new -o ServerAliveInterval=60 -o ServerAliveCountMax=3)
if [[ -n "$DEV_SSH_KEY" ]]; then
    SSH_ARGS+=(-i "$DEV_SSH_KEY")
fi

echo "Connecting: ssh ${DEV_USER}@${IP}..."
exec ssh "${SSH_ARGS[@]}" "${DEV_USER}@${IP}"
