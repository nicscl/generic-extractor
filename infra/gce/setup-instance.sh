#!/usr/bin/env bash
# One-time setup to run ON the GCE instance via SSH.
# Installs uv, copies sidecar code, installs GPU deps, sets up systemd + idle-shutdown.
#
# Usage (from your machine):
#   gcloud compute instances start docling-gpu --zone=us-central1-a
#   gcloud compute scp --recurse docling-sidecar/ docling-gpu:/tmp/docling-sidecar --zone=us-central1-a
#   gcloud compute scp infra/gce/setup-instance.sh infra/gce/docling-gpu.service infra/gce/idle-shutdown.sh infra/gce/pyproject-gpu.toml docling-gpu:/tmp/ --zone=us-central1-a
#   gcloud compute ssh docling-gpu --zone=us-central1-a -- 'sudo bash /tmp/setup-instance.sh'

set -euo pipefail

SIDECAR_DIR="/opt/docling-sidecar"

echo "==> Installing uv..."
curl -LsSf https://astral.sh/uv/install.sh | sh
export PATH="$HOME/.local/bin:$PATH"

echo "==> Setting up sidecar directory..."
mkdir -p "$SIDECAR_DIR"
cp /tmp/docling-sidecar/server.py "$SIDECAR_DIR/"

# Use GPU-variant pyproject.toml (no CPU-only overrides)
cp /tmp/pyproject-gpu.toml "$SIDECAR_DIR/pyproject.toml"

echo "==> Installing Python dependencies with CUDA support..."
cd "$SIDECAR_DIR"
uv sync

echo "==> Verifying GPU is accessible..."
uv run python -c "import torch; print(f'CUDA available: {torch.cuda.is_available()}, Device: {torch.cuda.get_device_name(0) if torch.cuda.is_available() else \"N/A\"}')"

echo "==> Installing systemd service..."
cp /tmp/docling-gpu.service /etc/systemd/system/docling-gpu.service
systemctl daemon-reload
systemctl enable docling-gpu.service
systemctl start docling-gpu.service

echo "==> Installing idle-shutdown cron..."
cp /tmp/idle-shutdown.sh /opt/idle-shutdown.sh
chmod +x /opt/idle-shutdown.sh

# Touch activity file so the instance doesn't shut down immediately
touch /tmp/docling_last_activity

# Run every 5 minutes
CRON_LINE="*/5 * * * * /opt/idle-shutdown.sh >> /var/log/idle-shutdown.log 2>&1"
(crontab -l 2>/dev/null | grep -v idle-shutdown; echo "$CRON_LINE") | crontab -

echo ""
echo "==> Setup complete!"
echo "    Docling service: systemctl status docling-gpu"
echo "    Health check:    curl http://localhost:3001/health"
echo "    Idle shutdown:   15 min inactivity → auto stop"
