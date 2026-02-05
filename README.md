# Generic Extractor

Rust server for hierarchical document extraction using OpenRouter's Gemini 3 Flash.

## Setup

```bash
make setup
# Edit .env and set OPENROUTER_API_KEY
```

## Run (single command)

```bash
make run
```

This starts:
- Docling sidecar on `http://localhost:3001`
- Rust API on `http://localhost:3000`

## API

- `POST /extract` - Upload document, returns extraction
- `GET /extractions/:id/snapshot` - Full extraction tree in one call (no raw content blobs)
- `GET /extractions/:id` - Get extraction by ID
- `GET /extractions/:id/node/:node_id` - Get specific node
- `GET /content/:ref` - Lazy-load content (supports `?offset=0&limit=2000`)
