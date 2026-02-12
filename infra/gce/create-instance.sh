#!/usr/bin/env bash
# One-time setup: create GCE instance with T4 GPU for Docling OCR sidecar.
# Prerequisites: gcloud CLI authenticated, project set.
#
# Usage: bash infra/gce/create-instance.sh

set -euo pipefail

PROJECT="${GCE_PROJECT_ID:?Set GCE_PROJECT_ID}"
ZONE="us-central1-a"
INSTANCE="docling-gpu"
MACHINE="n1-standard-4"
GPU="type=nvidia-tesla-t4,count=1"
DISK_SIZE="80GB"
IMAGE_FAMILY="common-cu124-debian-11-py310"   # Deep Learning VM with CUDA 12.4
IMAGE_PROJECT="deeplearning-platform-release"
STATIC_IP_NAME="docling-ip"
REGION="${ZONE%-*}"                             # us-central1

echo "==> Reserving static IP '$STATIC_IP_NAME' in $REGION..."
if ! gcloud compute addresses describe "$STATIC_IP_NAME" --region="$REGION" --project="$PROJECT" &>/dev/null; then
    gcloud compute addresses create "$STATIC_IP_NAME" \
        --region="$REGION" \
        --project="$PROJECT"
fi
STATIC_IP=$(gcloud compute addresses describe "$STATIC_IP_NAME" \
    --region="$REGION" --project="$PROJECT" \
    --format='value(address)')
echo "    Static IP: $STATIC_IP"

echo "==> Creating instance '$INSTANCE' (stopped)..."
gcloud compute instances create "$INSTANCE" \
    --project="$PROJECT" \
    --zone="$ZONE" \
    --machine-type="$MACHINE" \
    --accelerator="$GPU" \
    --maintenance-policy=TERMINATE \
    --boot-disk-size="$DISK_SIZE" \
    --boot-disk-type=pd-ssd \
    --image-family="$IMAGE_FAMILY" \
    --image-project="$IMAGE_PROJECT" \
    --address="$STATIC_IP" \
    --tags=docling-server \
    --metadata=install-nvidia-driver=True \
    --scopes=default \
    --no-restart-on-failure \
    --no-start-on-create

echo ""
echo "Done. Instance '$INSTANCE' created (TERMINATED)."
echo "Static IP: $STATIC_IP"
echo ""
echo "Next steps:"
echo "  1. Run: bash infra/gce/setup-firewall.sh"
echo "  2. Start instance: gcloud compute instances start $INSTANCE --zone=$ZONE --project=$PROJECT"
echo "  3. SSH in and run: bash infra/gce/setup-instance.sh"
