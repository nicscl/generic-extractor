#!/usr/bin/env bash
# Create firewall rule to restrict Docling port (3001) to the production server IP only.
#
# Usage: PROD_IP=1.2.3.4 bash infra/gce/setup-firewall.sh

set -euo pipefail

PROJECT="${GCE_PROJECT_ID:?Set GCE_PROJECT_ID}"
PROD_IP="${PROD_IP:?Set PROD_IP to the production server's public IP}"
RULE_NAME="allow-docling-from-prod"

echo "==> Creating firewall rule '$RULE_NAME' (allow tcp:3001 from $PROD_IP)..."

# Delete existing rule if present (idempotent)
if gcloud compute firewall-rules describe "$RULE_NAME" --project="$PROJECT" &>/dev/null; then
    echo "    Rule already exists, updating..."
    gcloud compute firewall-rules update "$RULE_NAME" \
        --project="$PROJECT" \
        --source-ranges="${PROD_IP}/32" \
        --allow=tcp:3001 \
        --target-tags=docling-server
else
    gcloud compute firewall-rules create "$RULE_NAME" \
        --project="$PROJECT" \
        --direction=INGRESS \
        --priority=1000 \
        --network=default \
        --action=ALLOW \
        --rules=tcp:3001 \
        --source-ranges="${PROD_IP}/32" \
        --target-tags=docling-server \
        --description="Allow Docling sidecar access from production server only"
fi

echo "Done. Only $PROD_IP can reach port 3001 on docling-server tagged instances."
