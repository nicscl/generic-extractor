#!/usr/bin/env bash
# Setup Docling sidecar on multiple GCE instances.
# For each zone: start instance, SCP code, run setup, stop.
#
# Prerequisites:
#   - Instances already created (via create-multi-zone.sh)
#   - gcloud CLI authenticated
#   - Run from the generic-extractor repo root
#
# Usage: GCE_PROJECT_ID=your-project bash infra/gce/setup-multi-zone.sh
#
# To setup specific zones only:
#   ZONES="us-central1-a" bash infra/gce/setup-multi-zone.sh

set -euo pipefail

PROJECT="${GCE_PROJECT_ID:?Set GCE_PROJECT_ID}"

# Default: setup all non-primary zones
ZONES="${ZONES:-us-central1-a us-west1-b}"

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

echo "Project:  $PROJECT"
echo "Zones:    $ZONES"
echo "Repo:     $REPO_ROOT"
echo ""

for ZONE in $ZONES; do
    REGION="${ZONE%-*}"
    INSTANCE="docling-gpu-${REGION}"

    echo "================================================================"
    echo "Setting up: $INSTANCE ($ZONE)"
    echo "================================================================"

    # Start the instance
    echo "  ==> Starting instance..."
    gcloud compute instances start "$INSTANCE" \
        --zone="$ZONE" \
        --project="$PROJECT"

    # Wait for SSH to become available
    echo "  ==> Waiting for SSH..."
    for i in $(seq 1 30); do
        if gcloud compute ssh "$INSTANCE" \
            --zone="$ZONE" \
            --project="$PROJECT" \
            --command="echo ok" \
            --ssh-flag="-o ConnectTimeout=5" \
            &>/dev/null; then
            break
        fi
        if [ "$i" -eq 30 ]; then
            echo "  ERROR: SSH not available after 150s, skipping $INSTANCE"
            continue 2
        fi
        sleep 5
    done

    # Copy files
    echo "  ==> Copying sidecar code..."
    gcloud compute scp --recurse \
        "$REPO_ROOT/docling-sidecar/" \
        "$INSTANCE:/tmp/docling-sidecar" \
        --zone="$ZONE" \
        --project="$PROJECT"

    echo "  ==> Copying infra scripts..."
    gcloud compute scp \
        "$SCRIPT_DIR/setup-instance.sh" \
        "$SCRIPT_DIR/docling-gpu.service" \
        "$SCRIPT_DIR/idle-shutdown.sh" \
        "$SCRIPT_DIR/pyproject-gpu.toml" \
        "$INSTANCE:/tmp/" \
        --zone="$ZONE" \
        --project="$PROJECT"

    # Run setup
    echo "  ==> Running setup script on instance..."
    gcloud compute ssh "$INSTANCE" \
        --zone="$ZONE" \
        --project="$PROJECT" \
        --command="sudo bash /tmp/setup-instance.sh"

    # Verify health
    echo "  ==> Verifying Docling health..."
    sleep 10
    if gcloud compute ssh "$INSTANCE" \
        --zone="$ZONE" \
        --project="$PROJECT" \
        --command="curl -s http://localhost:3001/health" 2>/dev/null; then
        echo ""
        echo "  Health check passed!"
    else
        echo "  WARNING: Health check failed (model may still be loading)"
    fi

    # Stop instance (it will auto-start on demand)
    echo "  ==> Stopping instance (will auto-start on demand)..."
    gcloud compute instances stop "$INSTANCE" \
        --zone="$ZONE" \
        --project="$PROJECT" \
        --quiet

    echo "  Done: $INSTANCE"
    echo ""
done

echo "================================================================"
echo "All instances set up!"
echo ""
echo "Update your .env with:"
echo "  GCE_INSTANCES=us-east1-d/docling-gpu$(for Z in $ZONES; do R="${Z%-*}"; echo -n ",$Z/docling-gpu-$R"; done)"
echo ""
echo "Then restart the generic-extractor service."
echo "================================================================"
