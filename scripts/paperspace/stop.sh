#!/usr/bin/env bash
#
# Stop the Paperspace machine (stops billing).
# Reads PAPERSPACE_API_KEY and PAPERSPACE_MACHINE_ID from .env.
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ENV_FILE="$(cd "$SCRIPT_DIR/../.." && pwd)/.env"

if [ -f "$ENV_FILE" ]; then set -a; source "$ENV_FILE"; set +a; fi

API_KEY="${PAPERSPACE_API_KEY:?PAPERSPACE_API_KEY not set}"
MACHINE_ID="${PAPERSPACE_MACHINE_ID:?PAPERSPACE_MACHINE_ID not set}"

echo "Stopping machine $MACHINE_ID..."
curl -sf -X POST \
    -H "x-api-key: $API_KEY" \
    "https://api.paperspace.io/machines/$MACHINE_ID/stop" >/dev/null

echo "Machine stop requested. Billing will cease once state is 'off'."
