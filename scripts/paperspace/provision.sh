#!/usr/bin/env bash
#
# Provision a Paperspace GPU machine for Docling OCR sidecar.
#
# Creates the machine via API, waits for boot, SSHes in to deploy the sidecar,
# then appends PAPERSPACE_MACHINE_ID to .env.
#
# Usage:
#   ./provision.sh [MACHINE_TYPE] [REGION]
#   ./provision.sh P4000 NY1
#   ./provision.sh RTX4000 CA1
#
# Reads PAPERSPACE_API_KEY from ../.env automatically.
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
ENV_FILE="$PROJECT_DIR/.env"

# Source .env
if [ -f "$ENV_FILE" ]; then
    set -a; source "$ENV_FILE"; set +a
fi

API_KEY="${PAPERSPACE_API_KEY:?PAPERSPACE_API_KEY not found in .env}"
API_BASE="https://api.paperspace.io"

MACHINE_TYPE="${1:-P4000}"
# Paperspace region format: "East Coast (NY2)", "West Coast (CA1)", "Europe (AMS1)"
REGION="${2:-East Coast (NY2)}"
MACHINE_NAME="docling-${MACHINE_TYPE,,}-$(date +%s)"
DISK_SIZE=50
TEMPLATE_ID="t0nspur5"  # ML-in-a-Box Ubuntu 22.04

log() { echo "[$(date '+%H:%M:%S')] $*"; }

# ---------------------------------------------------------------------------
# Step 1: Create machine
# ---------------------------------------------------------------------------
log "Creating $MACHINE_TYPE machine '$MACHINE_NAME' in $REGION..."

RESPONSE=$(curl -sf -X POST \
    -H "x-api-key: $API_KEY" \
    -H "Content-Type: application/json" \
    -d "{
        \"region\": \"$REGION\",
        \"machineType\": \"$MACHINE_TYPE\",
        \"size\": $DISK_SIZE,
        \"billingType\": \"hourly\",
        \"machineName\": \"$MACHINE_NAME\",
        \"templateId\": \"$TEMPLATE_ID\",
        \"dynamicPublicIp\": true,
        \"startOnCreate\": true
    }" \
    "$API_BASE/machines/createSingleMachinePublic" 2>&1) || {
    log "ERROR: API request failed. Response:"
    echo "$RESPONSE"
    exit 1
}

MACHINE_ID=$(echo "$RESPONSE" | jq -r '.id // empty')
if [ -z "$MACHINE_ID" ]; then
    log "ERROR: No machine ID in response:"
    echo "$RESPONSE" | jq . 2>/dev/null || echo "$RESPONSE"
    exit 1
fi
log "Machine created: $MACHINE_ID"

# ---------------------------------------------------------------------------
# Step 2: Wait for ready
# ---------------------------------------------------------------------------
log "Waiting for machine to boot (up to 10 min)..."
for i in $(seq 1 120); do
    INFO=$(curl -sf -H "x-api-key: $API_KEY" "$API_BASE/machines/getMachinePublic?machineId=$MACHINE_ID" 2>/dev/null || echo '{}')
    STATE=$(echo "$INFO" | jq -r '.state // "unknown"')
    if [ "$STATE" = "ready" ]; then
        break
    fi
    [ $((i % 6)) -eq 0 ] && log "  State: $STATE..."
    sleep 5
done

if [ "$STATE" != "ready" ]; then
    log "ERROR: Machine did not become ready (state: $STATE)"
    exit 1
fi

PUBLIC_IP=$(echo "$INFO" | jq -r '.publicIpAddress // .dynamicPublicIp // empty')
log "Machine ready at $PUBLIC_IP"

# ---------------------------------------------------------------------------
# Step 3: Deploy sidecar
# ---------------------------------------------------------------------------
log "Deploying sidecar (this calls setup-sidecar.sh)..."
"$SCRIPT_DIR/setup-sidecar.sh" "$PUBLIC_IP"

# ---------------------------------------------------------------------------
# Step 4: Update .env
# ---------------------------------------------------------------------------
if grep -q "PAPERSPACE_MACHINE_ID" "$ENV_FILE" 2>/dev/null; then
    sed -i "s|^PAPERSPACE_MACHINE_ID=.*|PAPERSPACE_MACHINE_ID=$MACHINE_ID|" "$ENV_FILE"
    log "Updated PAPERSPACE_MACHINE_ID in .env"
else
    echo "PAPERSPACE_MACHINE_ID=$MACHINE_ID" >> "$ENV_FILE"
    log "Added PAPERSPACE_MACHINE_ID to .env"
fi

# ---------------------------------------------------------------------------
# Done
# ---------------------------------------------------------------------------
echo ""
echo "=========================================="
echo "  Paperspace Machine Ready!"
echo "=========================================="
echo "  Machine ID:  $MACHINE_ID"
echo "  Public IP:   $PUBLIC_IP"
echo "  GPU:         $MACHINE_TYPE"
echo "  Region:      $REGION"
echo "  Docling URL: http://$PUBLIC_IP:3001"
echo ""
echo "  Restart generic-extractor to activate:"
echo "  sudo systemctl restart generic-extractor"
echo "=========================================="
