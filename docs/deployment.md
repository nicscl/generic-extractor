# Deployment Guide

How to install, configure, and run the Generic Extractor. Covers single-machine dev, split-machine production, and all service components.

## Components

| Component | Language | Port | RAM | Purpose |
|---|---|---|---|---|
| **Rust API** | Rust/Axum | 3002 | ~50 MB | Orchestrates extraction, serves results |
| **Docling sidecar** | Python/FastAPI | 3001 | **4-8 GB** | PDF OCR + text extraction (heavy) |
| **MCP server** | Node.js | 3003 | ~35 MB | MCP protocol bridge for LLM agents |

Docling is the heavy component. It loads PyTorch ML models for OCR and layout analysis. Everything else is lightweight.

---

## Requirements

| Tool | Version | Install |
|---|---|---|
| Rust + Cargo | stable | `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \| sh` |
| uv (Python) | latest | `curl -LsSf https://astral.sh/uv/install.sh \| sh` |
| Node.js | 20+ | `apt install nodejs npm` or nvm |
| Python | 3.10+ | Managed by uv |

---

## Quick Start (single machine, dev)

```bash
git clone git@github.com:nicscl/generic-extractor.git
cd generic-extractor
make setup          # installs deps, creates .env
vi .env             # set OPENROUTER_API_KEY (required)
make run            # starts Docling on :3001, Rust API on :3002
```

Requires **8+ GB RAM** for Docling OCR on real documents.

---

## Environment Variables

Set these in `.env` at the project root:

```bash
# Required
OPENROUTER_API_KEY=sk-or-your-key-here

# Optional: Supabase persistence
SUPABASE_URL=https://your-project.supabase.co
SUPABASE_SERVICE_ROLE_KEY=your-service-role-key

# Optional: Ports and URLs
PORT=3002                              # Rust API port (default: 3002)
DOCLING_URL=http://localhost:3001      # Docling sidecar URL (default: http://localhost:3001)
```

---

## Running Each Component

### Docling Sidecar

```bash
uv run --project docling-sidecar uvicorn server:app \
  --app-dir docling-sidecar \
  --host 0.0.0.0 \
  --port 3001
```

First request is slow (~30s) — it lazy-loads ML models into memory. Subsequent requests are fast.

Verify: `curl http://localhost:3001/health`

### Rust API

```bash
# Dev
cargo run

# Production (build release first)
cargo build --release
./target/release/generic-extractor
```

Reads `.env` automatically. Verify: `curl http://localhost:3002/health`

### MCP Server (STDIO — for Claude Code)

Configured in `.mcp.json`, starts automatically when Claude Code opens the project.

### MCP Server (HTTP — for remote agents)

```bash
cd mcp-server
npm install && npm run build
MCP_TRANSPORT=http MCP_PORT=3003 EXTRACTOR_API_URL=http://localhost:3002 node dist/index.js
```

Verify: `curl http://localhost:3003/health`

---

## Split-Machine Deployment

For production, run Docling on a machine with enough RAM and everything else on a smaller machine.

### Machine A — Docling (8+ GB RAM)

```bash
git clone git@github.com:nicscl/generic-extractor.git
cd generic-extractor
uv sync --project docling-sidecar
uv run --project docling-sidecar uvicorn server:app \
  --app-dir docling-sidecar \
  --host 0.0.0.0 \
  --port 3001
```

### Machine B — API + MCP (2 GB RAM is fine)

```bash
git clone git@github.com:nicscl/generic-extractor.git
cd generic-extractor

# .env
OPENROUTER_API_KEY=sk-or-...
SUPABASE_URL=https://...
SUPABASE_SERVICE_ROLE_KEY=...
DOCLING_URL=http://machine-a-ip:3001    # <-- point to Machine A

# Build and run
cargo build --release
./target/release/generic-extractor

# MCP server (optional)
cd mcp-server && npm install && npm run build
MCP_TRANSPORT=http MCP_PORT=3003 node dist/index.js
```

---

## Production with systemd

### Rust API

```ini
# /etc/systemd/system/generic-extractor.service
[Unit]
Description=Generic Extractor API
After=network.target

[Service]
Type=simple
User=root
WorkingDirectory=/root/generic-extractor
EnvironmentFile=/root/generic-extractor/.env
ExecStart=/root/generic-extractor/target/release/generic-extractor
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
```

### MCP HTTP Server

```ini
# /etc/systemd/system/mcp-extractor.service
[Unit]
Description=Generic Extractor MCP HTTP Server
After=network.target

[Service]
Type=simple
User=root
WorkingDirectory=/root/generic-extractor/mcp-server
Environment=MCP_TRANSPORT=http
Environment=MCP_PORT=3003
Environment=EXTRACTOR_API_URL=http://localhost:3002
ExecStart=/usr/bin/node /root/generic-extractor/mcp-server/dist/index.js
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
```

### Docling Sidecar

```ini
# /etc/systemd/system/docling-sidecar.service
[Unit]
Description=Docling PDF OCR Sidecar
After=network.target

[Service]
Type=simple
User=root
WorkingDirectory=/root/generic-extractor
ExecStart=/root/.local/bin/uv run --project docling-sidecar uvicorn server:app --app-dir docling-sidecar --host 0.0.0.0 --port 3001
Restart=on-failure
RestartSec=10

[Install]
WantedBy=multi-user.target
```

Enable and start:

```bash
systemctl daemon-reload
systemctl enable --now generic-extractor mcp-extractor docling-sidecar
```

---

## Nginx Reverse Proxy (with SSL)

### API — `aiapi.sciron.tech`

```nginx
server {
    server_name aiapi.sciron.tech;

    location / {
        proxy_pass http://127.0.0.1:3002;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
        proxy_read_timeout 300s;    # extraction can take minutes on CPU
        proxy_send_timeout 300s;
        client_max_body_size 100M;
    }

    # SSL managed by certbot
}
```

### MCP — `mcp.sciron.tech`

```nginx
server {
    server_name mcp.sciron.tech;

    location / {
        proxy_pass http://127.0.0.1:3003;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
        proxy_http_version 1.1;
        proxy_set_header Connection '';
        proxy_buffering off;           # required for SSE
        proxy_cache off;
        proxy_read_timeout 86400s;     # long-lived SSE connections
    }

    # SSL managed by certbot
}
```

Set up SSL:

```bash
apt install certbot python3-certbot-nginx
certbot --nginx -d aiapi.sciron.tech -d mcp.sciron.tech
```

---

## Verify Everything Works

```bash
# Health checks
curl https://aiapi.sciron.tech/health          # → ok
curl http://localhost:3001/health               # → {"status":"ok"}
curl https://mcp.sciron.tech/health             # → {"status":"ok"}

# List configs
curl https://aiapi.sciron.tech/configs          # → ["legal_br"]

# List existing extractions
curl https://aiapi.sciron.tech/extractions

# Extract via file upload
curl -X POST https://aiapi.sciron.tech/extract \
  -F "file=@document.pdf" \
  -G -d "config=legal_br&upload=true"

# Extract via URL
curl -X POST "https://aiapi.sciron.tech/extract?file_url=https://example.com/doc.pdf&config=legal_br&upload=true"
```
