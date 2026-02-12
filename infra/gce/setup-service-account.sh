#!/usr/bin/env bash
# Create a minimal service account for starting/stopping the Docling GPU instance.
# Downloads the JSON key to ./docling-starter-key.json.
#
# Usage: bash infra/gce/setup-service-account.sh

set -euo pipefail

PROJECT="${GCE_PROJECT_ID:?Set GCE_PROJECT_ID}"
SA_NAME="docling-starter"
SA_EMAIL="${SA_NAME}@${PROJECT}.iam.gserviceaccount.com"
KEY_FILE="docling-starter-key.json"

echo "==> Creating service account '$SA_NAME'..."
if ! gcloud iam service-accounts describe "$SA_EMAIL" --project="$PROJECT" &>/dev/null; then
    gcloud iam service-accounts create "$SA_NAME" \
        --project="$PROJECT" \
        --display-name="Docling instance starter (minimal permissions)"
else
    echo "    Service account already exists."
fi

echo "==> Granting compute.instanceAdmin.v1 role..."
gcloud projects add-iam-policy-binding "$PROJECT" \
    --member="serviceAccount:${SA_EMAIL}" \
    --role="roles/compute.instanceAdmin.v1" \
    --condition="expression=resource.name == 'projects/${PROJECT}/zones/us-central1-a/instances/docling-gpu',title=docling-gpu-only" \
    --quiet

echo "==> Generating key file: $KEY_FILE"
gcloud iam service-accounts keys create "$KEY_FILE" \
    --iam-account="$SA_EMAIL" \
    --project="$PROJECT"

echo ""
echo "Done. Key saved to: $KEY_FILE"
echo "Copy this to your production server (e.g., /etc/docling-starter-key.json)"
echo "and set GCE_SA_KEY_PATH=/etc/docling-starter-key.json in your .env"
