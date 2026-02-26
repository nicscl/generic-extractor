#!/usr/bin/env bash
#
# Check Paperspace machine status.
# Reads PAPERSPACE_API_KEY and PAPERSPACE_MACHINE_ID from .env.
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ENV_FILE="$(cd "$SCRIPT_DIR/../.." && pwd)/.env"

if [ -f "$ENV_FILE" ]; then set -a; source "$ENV_FILE"; set +a; fi

API_KEY="${PAPERSPACE_API_KEY:?PAPERSPACE_API_KEY not set}"
MACHINE_ID="${PAPERSPACE_MACHINE_ID:?PAPERSPACE_MACHINE_ID not set}"

INFO=$(curl -sf -H "x-api-key: $API_KEY" \
    "https://api.paperspace.io/machines/getMachinePublic?machineId=$MACHINE_ID" 2>&1) || {
    echo "ERROR: Failed to query machine status"
    exit 1
}

STATE=$(echo "$INFO" | jq -r '.state // "unknown"')
NAME=$(echo "$INFO" | jq -r '.name // "unknown"')
IP=$(echo "$INFO" | jq -r '.publicIpAddress // .dynamicPublicIp // "none"')
GPU=$(echo "$INFO" | jq -r '.machineType // "unknown"')
REGION=$(echo "$INFO" | jq -r '.region // "unknown"')
RATE=$(echo "$INFO" | jq -r '.usageRate // "unknown"')

echo "Machine:  $NAME ($MACHINE_ID)"
echo "State:    $STATE"
echo "GPU:      $GPU"
echo "Region:   $REGION"
echo "IP:       $IP"
echo "Rate:     $RATE"

if [ "$STATE" = "ready" ] && [ "$IP" != "none" ]; then
    echo ""
    echo "Docling health:"
    curl -sf "http://$IP:3001/health" 2>/dev/null && echo " OK" || echo " NOT REACHABLE"
fi
