#!/bin/bash
# Auto-restart wrapper for the Docling sidecar.
# Used by /cancel-all endpoint which does os._exit(0) to kill stuck conversions.
set -u

PORT="${PORT:-3001}"
RESTART_DELAY=2

while true; do
    echo "[$(date)] Starting Docling sidecar on :${PORT}..."
    uv run --project docling-sidecar uvicorn server:app \
        --app-dir docling-sidecar \
        --host 0.0.0.0 \
        --port "$PORT"
    EXIT_CODE=$?
    echo "[$(date)] Sidecar exited (code=$EXIT_CODE), restarting in ${RESTART_DELAY}s..."
    sleep "$RESTART_DELAY"
done
