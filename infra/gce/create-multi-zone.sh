#!/usr/bin/env bash
# Create Docling GPU instances across multiple zones for failover.
# Skips zones where the instance already exists.
#
# Prerequisites: gcloud CLI authenticated, project set.
#
# Usage: GCE_PROJECT_ID=your-project bash infra/gce/create-multi-zone.sh
#
# To create in specific zones only:
#   ZONES="us-central1-a us-west1-b" bash infra/gce/create-multi-zone.sh

set -euo pipefail

PROJECT="${GCE_PROJECT_ID:?Set GCE_PROJECT_ID}"

# Default zones to create in (skip us-east1-d — already exists)
ZONES="${ZONES:-us-central1-a us-west1-b}"

MACHINE="n1-standard-4"
GPU="type=nvidia-tesla-t4,count=1"
DISK_SIZE="30GB"
IMAGE_FAMILY="common-cu128-ubuntu-2204-nvidia-570"
IMAGE_PROJECT="deeplearning-platform-release"

echo "Project: $PROJECT"
echo "Zones:   $ZONES"
echo ""

for ZONE in $ZONES; do
    REGION="${ZONE%-*}"
    # Instance name: docling-gpu-<region> (e.g., docling-gpu-us-central1)
    INSTANCE="docling-gpu-${REGION}"
    IP_NAME="docling-ip-${REGION}"

    echo "================================================================"
    echo "Zone: $ZONE  Instance: $INSTANCE"
    echo "================================================================"

    # Check if instance already exists
    if gcloud compute instances describe "$INSTANCE" --zone="$ZONE" --project="$PROJECT" &>/dev/null; then
        echo "  Instance '$INSTANCE' already exists in $ZONE, skipping."
        echo ""
        continue
    fi

    # Reserve static IP (if not already reserved)
    echo "  ==> Reserving static IP '$IP_NAME' in $REGION..."
    if ! gcloud compute addresses describe "$IP_NAME" --region="$REGION" --project="$PROJECT" &>/dev/null; then
        gcloud compute addresses create "$IP_NAME" \
            --region="$REGION" \
            --project="$PROJECT"
    fi
    STATIC_IP=$(gcloud compute addresses describe "$IP_NAME" \
        --region="$REGION" --project="$PROJECT" \
        --format='value(address)')
    echo "      Static IP: $STATIC_IP"

    # Create instance (stopped)
    echo "  ==> Creating instance '$INSTANCE' in $ZONE..."
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
        --no-restart-on-failure

    echo "  ==> Stopping instance..."
    gcloud compute instances stop "$INSTANCE" \
        --project="$PROJECT" \
        --zone="$ZONE" \
        --quiet

    echo "  Done: $INSTANCE ($ZONE) → $STATIC_IP"
    echo ""
done

echo "================================================================"
echo "All instances created. Next steps:"
echo ""
echo "  1. Update service account permissions:"
echo "     bash infra/gce/setup-service-account.sh"
echo ""
echo "  2. Setup each instance (install deps, systemd, etc.):"
echo "     bash infra/gce/setup-multi-zone.sh"
echo ""
echo "  3. Update .env on your API server:"
echo "     GCE_INSTANCES=us-east1-d/docling-gpu,<zone>/<name>,..."
echo "================================================================"
