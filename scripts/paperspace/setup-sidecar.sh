#!/usr/bin/env bash
#
# Deploy the Docling sidecar to a Paperspace machine via SSH.
#
# Usage:
#   ./setup-sidecar.sh <IP_ADDRESS> [SSH_USER]
#
set -euo pipefail

IP="${1:?Usage: setup-sidecar.sh <IP_ADDRESS> [SSH_USER]}"
SSH_USER="${2:-paperspace}"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SIDECAR_DIR="$(cd "$SCRIPT_DIR/../../docling-sidecar" && pwd)"

SSH_OPTS="-o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o ConnectTimeout=10 -o LogLevel=ERROR -i ~/.ssh/azure_devops_rsa"
SSH_CMD="ssh $SSH_OPTS $SSH_USER@$IP"
SCP_CMD="scp $SSH_OPTS"

log() { echo "[$(date '+%H:%M:%S')] $*"; }

# ---------------------------------------------------------------------------
# Step 1: Wait for SSH
# ---------------------------------------------------------------------------
log "Waiting for SSH on $IP..."
for i in $(seq 1 60); do
    if $SSH_CMD "echo ok" >/dev/null 2>&1; then
        log "SSH ready"
        break
    fi
    [ $((i % 6)) -eq 0 ] && log "  Still waiting for SSH..."
    sleep 5
done

if ! $SSH_CMD "echo ok" >/dev/null 2>&1; then
    log "ERROR: SSH not available after 5 minutes"
    exit 1
fi

# ---------------------------------------------------------------------------
# Step 2: Install system dependencies and uv
# ---------------------------------------------------------------------------
log "Installing system dependencies (libGL, libglib2)..."
$SSH_CMD bash <<'EOF'
sudo apt-get update -qq && sudo apt-get install -y -qq libgl1-mesa-glx libglib2.0-0 2>/dev/null || \
    echo "WARN: Could not install system deps (no sudo?). Docling may fail if libGL is missing."
if [ -f "$HOME/.local/bin/uv" ]; then
    echo "uv already installed"
else
    curl -LsSf https://astral.sh/uv/install.sh | sh
fi
EOF

# ---------------------------------------------------------------------------
# Step 3: Copy sidecar files
# ---------------------------------------------------------------------------
log "Copying sidecar files..."
$SCP_CMD "$SIDECAR_DIR/server.py" "$SSH_USER@$IP:/tmp/server.py"
$SCP_CMD "$SIDECAR_DIR/pyproject.toml" "$SSH_USER@$IP:/tmp/pyproject.toml"
$SCP_CMD "$SIDECAR_DIR/uv.lock" "$SSH_USER@$IP:/tmp/uv.lock"

# ---------------------------------------------------------------------------
# Step 4: Install into user home (no sudo needed)
# ---------------------------------------------------------------------------
log "Setting up sidecar..."
$SSH_CMD bash <<'EOF'
set -e

mkdir -p ~/docling-sidecar
cp /tmp/server.py ~/docling-sidecar/
cp /tmp/pyproject.toml ~/docling-sidecar/
cp /tmp/uv.lock ~/docling-sidecar/

cd ~/docling-sidecar
~/.local/bin/uv sync 2>&1

# Create a user-level systemd service (no sudo)
mkdir -p ~/.config/systemd/user

cat > ~/.config/systemd/user/docling-sidecar.service <<'SERVICE'
[Unit]
Description=Docling OCR Sidecar (GPU)
After=network-online.target

[Service]
Type=simple
WorkingDirectory=%h/docling-sidecar
ExecStart=%h/.local/bin/uv run --project %h/docling-sidecar uvicorn server:app --host 0.0.0.0 --port 3001
Restart=on-failure
RestartSec=5
Environment=PYTORCH_CUDA_ALLOC_CONF=expandable_segments:True

[Install]
WantedBy=default.target
SERVICE

systemctl --user daemon-reload
systemctl --user enable docling-sidecar
systemctl --user restart docling-sidecar

# Enable linger so the user-level service survives reboots
sudo loginctl enable-linger $(whoami) 2>/dev/null || echo "WARN: Could not enable linger (no sudo?)"
echo "Service started"
EOF

# ---------------------------------------------------------------------------
# Step 5: Wait for health check
# ---------------------------------------------------------------------------
log "Waiting for Docling health check on http://$IP:3001/health ..."
for i in $(seq 1 90); do
    if curl -sf "http://$IP:3001/health" >/dev/null 2>&1; then
        log "Docling sidecar is healthy!"
        exit 0
    fi
    [ $((i % 12)) -eq 0 ] && log "  Still loading models ($((i*5/60))min)..."
    sleep 5
done

log "WARNING: Health check not passing after 7.5 minutes."
log "Models may still be downloading. Check with:"
log "  ssh -i ~/.ssh/azure_devops_rsa $SSH_USER@$IP 'systemctl --user status docling-sidecar'"
