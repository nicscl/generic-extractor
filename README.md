# Generic Extractor

Rust server for hierarchical document extraction using OpenRouter's Gemini 3 Flash.

## Setup

```bash
cp .env.example .env
# Edit .env with your OpenRouter API key
cargo run
```

## API

- `POST /extract` - Upload document, returns extraction
- `GET /extractions/:id` - Get extraction by ID
- `GET /extractions/:id/node/:node_id` - Get specific node
- `GET /content/:ref` - Lazy-load content (supports `?offset=0&limit=2000`)
