#!/usr/bin/env bash
#
# Start the Paperspace machine.
# Reads PAPERSPACE_API_KEY and PAPERSPACE_MACHINE_ID from .env.
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ENV_FILE="$(cd "$SCRIPT_DIR/../.." && pwd)/.env"

if [ -f "$ENV_FILE" ]; then set -a; source "$ENV_FILE"; set +a; fi

API_KEY="${PAPERSPACE_API_KEY:?PAPERSPACE_API_KEY not set}"
MACHINE_ID="${PAPERSPACE_MACHINE_ID:?PAPERSPACE_MACHINE_ID not set}"

echo "Starting machine $MACHINE_ID..."
curl -sf -X POST \
    -H "x-api-key: $API_KEY" \
    "https://api.paperspace.io/machines/$MACHINE_ID/start" >/dev/null

echo "Waiting for ready..."
for i in $(seq 1 60); do
    STATE=$(curl -sf -H "x-api-key: $API_KEY" \
        "https://api.paperspace.io/machines/getMachinePublic?machineId=$MACHINE_ID" \
        | jq -r '.state // "unknown"')
    if [ "$STATE" = "ready" ]; then
        IP=$(curl -sf -H "x-api-key: $API_KEY" \
            "https://api.paperspace.io/machines/getMachinePublic?machineId=$MACHINE_ID" \
            | jq -r '.publicIpAddress // .dynamicPublicIp // "unknown"')
        echo "Machine ready at $IP (Docling: http://$IP:3001)"
        exit 0
    fi
    [ $((i % 6)) -eq 0 ] && echo "  State: $STATE..."
    sleep 5
done
echo "ERROR: Timeout waiting for machine to start"
exit 1
