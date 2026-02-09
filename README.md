# Generic Extractor

Config-driven hierarchical document extraction server. Uploads a PDF, runs OCR via [Docling](https://github.com/DS4SD/docling), then uses an LLM (Gemini 3 Flash via OpenRouter) to extract a navigable document tree with summaries, cross-references, and lazy-loadable content.

**Production:** `https://aiapi.sciron.tech`

## Architecture

```
PDF Upload → Docling Sidecar (Python, :3001) → Rust API (:3002) → OpenRouter/Gemini → Structured JSON
                                                      ↓ (optional)
                                                   Supabase
```

- **Docling Sidecar** — Python FastAPI service for PDF-to-text (OCR + markdown). Lazy-loads ML models on first request.
- **Rust API** — Axum server that orchestrates the extraction pipeline and serves results.

## Setup

```bash
make setup
# Edit .env and set OPENROUTER_API_KEY (required)
# Optionally set SUPABASE_URL and SUPABASE_SERVICE_ROLE_KEY for persistence
# Optionally set PORT to change the API port (default: 3002)
```

## Run

```bash
make run
```

This starts:
- Docling sidecar on `http://localhost:3001`
- Rust API on `http://localhost:3002` (configurable via `PORT` env var)

## API

| Endpoint | Method | Description |
|---|---|---|
| `/health` | GET | Health check |
| `/configs` | GET | List available extraction configs |
| `/configs/:name` | GET | Get a specific config |
| `/extract?config=legal_br&upload=true` | POST | Upload PDF (multipart `file` field), run extraction. `upload=true` persists to Supabase. |
| `/extractions` | GET | List all extractions (lightweight summaries with IDs) |
| `/extractions/:id/snapshot` | GET | Full extraction tree in one call (no raw content blobs, optimized for MCP/context loading) |
| `/extractions/:id` | GET | Get extraction by ID |
| `/extractions/:id/node/:node_id` | GET | Get specific node |
| `/content/:ref` | GET | Lazy-load content (supports `?offset=0&limit=4000`) |

### Example

```bash
curl -X POST https://aiapi.sciron.tech/extract \
  -F "file=@document.pdf" \
  -G -d "config=legal_br&upload=true"
```

## Configs

Domain-specific extraction configs live in `configs/*.json`. Each config defines:
- LLM prompt for structure extraction
- Allowed node types and subtypes
- Relationship types
- Metadata schema

Currently available: `legal_br` (Brazilian legal case files).
