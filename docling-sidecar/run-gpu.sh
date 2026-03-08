#!/bin/bash
# GPU variant of run.sh — uses venv python directly instead of `uv run`
# to avoid uv syncing the lockfile and overwriting CUDA torch with CPU torch.
set -u

PORT="${PORT:-3001}"
RESTART_DELAY=2
VENV="/root/generic-extractor/docling-sidecar/.venv"

while true; do
    echo "[$(date)] Starting Docling sidecar (GPU) on :${PORT}..."
    "$VENV/bin/python3" -m uvicorn server:app \
        --app-dir /root/generic-extractor/docling-sidecar \
        --host 0.0.0.0 \
        --port "$PORT"
    EXIT_CODE=$?
    echo "[$(date)] Sidecar exited (code=$EXIT_CODE), restarting in ${RESTART_DELAY}s..."
    sleep "$RESTART_DELAY"
done
