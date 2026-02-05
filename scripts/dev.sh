#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SIDECAR_PID=""

require_cmd() {
    local cmd="$1"
    if ! command -v "$cmd" >/dev/null 2>&1; then
        echo "Missing required command: $cmd"
        exit 1
    fi
}

cleanup() {
    if [[ -n "$SIDECAR_PID" ]] && kill -0 "$SIDECAR_PID" 2>/dev/null; then
        kill "$SIDECAR_PID" 2>/dev/null || true
        wait "$SIDECAR_PID" 2>/dev/null || true
    fi
}
trap cleanup EXIT INT TERM

cd "$ROOT_DIR"

require_cmd uv
require_cmd cargo
require_cmd curl

if [[ ! -f ".env" ]]; then
    echo "Missing .env file. Run: make setup"
    exit 1
fi

echo "Starting Docling sidecar on :3001..."
uv run --project docling-sidecar uvicorn server:app --app-dir docling-sidecar --host 0.0.0.0 --port 3001 &
SIDECAR_PID=$!

for _ in {1..30}; do
    if curl -fsS "http://127.0.0.1:3001/health" >/dev/null; then
        echo "Docling sidecar is ready."
        break
    fi

    if ! kill -0 "$SIDECAR_PID" 2>/dev/null; then
        echo "Docling sidecar failed to start."
        exit 1
    fi

    sleep 1
done

if ! curl -fsS "http://127.0.0.1:3001/health" >/dev/null; then
    echo "Timed out waiting for Docling sidecar."
    exit 1
fi

echo "Starting Rust API on :3000..."
cargo run
