# Extraction Guide

How to extract hierarchical structure from a PDF and navigate the results using the Generic Extractor. Covers all access methods: REST API, local MCP (STDIO), and remote MCP (HTTP).

## Overview

```
                     ┌─────────────────────────────────┐
                     │        Your PDF document         │
                     └───────────────┬─────────────────┘
                                     │
               ┌─────────────────────┼─────────────────────┐
               ▼                     ▼                     ▼
        REST API (curl)      MCP Local (STDIO)     MCP Remote (HTTP)
     POST /extract           file_path              file_url
     multipart upload        reads from disk         downloads from URL
                                                     — or —
                                                    file_base64
                                                     sends raw bytes
               │                     │                     │
               └─────────────────────┼─────────────────────┘
                                     ▼
                              Rust API (:3002)
                                     │
                           ┌─────────┼─────────┐
                           ▼                   ▼
                    Docling OCR (:3001)   Gemini 3 Flash
                     page-level text      structure + summaries
                           │                   │
                           └─────────┬─────────┘
                                     ▼
                           Extraction (JSON tree)
                                     │
                              ┌──────┴──────┐
                              ▼             ▼
                          In-memory     Supabase
                           cache        (if upload=true)
```

---

## Method 1: REST API (curl / any HTTP client)

Best for: scripts, CI pipelines, custom integrations.

### Extract

```bash
curl -X POST https://aiapi.sciron.tech/extract \
  -F "file=@/path/to/document.pdf" \
  -G -d "config=legal_br&upload=true"
```

| Parameter | Type | Default | Description |
|---|---|---|---|
| `file` | multipart | *required* | The PDF file (max 100 MB) |
| `config` | query string | `legal_br` | Extraction config name |
| `upload` | query string | `false` | `true` to persist in Supabase |

### Navigate

```bash
# List all extractions
curl https://aiapi.sciron.tech/extractions

# Get full tree (summaries, structure, relationships — no raw text)
curl https://aiapi.sciron.tech/extractions/EXT_ID/snapshot

# Load raw text for a node (paginated)
curl "https://aiapi.sciron.tech/content/NODE_ID?offset=0&limit=4000"

# Get a specific node
curl https://aiapi.sciron.tech/extractions/EXT_ID/node/NODE_ID
```

---

## Method 2: MCP — Local Agent (STDIO)

Best for: Claude Code, local LLM agents with filesystem access.

The agent has direct access to the filesystem, so it can reference PDF files by path.

### Setup

Already configured in `.mcp.json` at the project root:

```json
{
  "mcpServers": {
    "generic-extractor": {
      "command": "node",
      "args": ["mcp-server/dist/index.js"],
      "env": {
        "EXTRACTOR_API_URL": "http://localhost:3002"
      }
    }
  }
}
```

### Extract

```json
extract_document({
  "file_path": "/home/user/documents/case.pdf",
  "config": "legal_br",
  "upload": true
})
```

The MCP server reads the file from disk, uploads it to the API, and returns the full extraction.

### Navigate

```json
list_extractions()

get_extraction_snapshot({ "extraction_id": "ext_abc123" })

get_content({ "ref": "content://doc_1", "offset": 0, "limit": 4000 })

get_node({ "extraction_id": "ext_abc123", "node_id": "doc_1" })
```

---

## Method 3: MCP — Remote Agent (HTTP)

Best for: remote LLM agents, web apps, any client that connects to `https://mcp.sciron.tech/mcp` over Streamable HTTP transport.

Remote agents **do not have filesystem access** on the server. There are two ways to get a PDF to the extractor:

### Option A: PDF from a URL

If your PDF is hosted somewhere accessible (S3, cloud storage, public link):

```json
extract_document({
  "file_url": "https://storage.example.com/docs/contract.pdf",
  "config": "legal_br",
  "upload": true
})
```

The MCP server downloads the file from the URL and forwards it to the API. The filename is derived from the URL path (or you can set `file_name` explicitly).

### Option B: PDF as base64

If you have the raw file bytes (e.g. user uploaded via a web UI, received as an attachment):

```json
extract_document({
  "file_base64": "JVBERi0xLjQKJeLjz9MKMSAwIG9iago8PC...",
  "file_name": "contract.pdf",
  "config": "legal_br",
  "upload": true
})
```

`file_name` is **required** with `file_base64` since there's no path or URL to derive it from.

### How to base64-encode a PDF

```bash
# macOS / Linux
base64 -i document.pdf -o document.b64

# Or inline
base64 < document.pdf
```

```python
# Python
import base64
with open("document.pdf", "rb") as f:
    encoded = base64.b64encode(f.read()).decode()
```

```javascript
// Node.js
const fs = require("fs");
const encoded = fs.readFileSync("document.pdf").toString("base64");
```

### Navigation (same for all MCP modes)

```json
list_extractions()
get_extraction_snapshot({ "extraction_id": "ext_abc123" })
get_content({ "ref": "content://doc_1", "offset": 0, "limit": 4000 })
```

---

## The Extraction Pipeline

What happens when you call `extract_document` or `POST /extract`:

```
1. PDF received
2. → Docling sidecar (Python) performs OCR
   ← Returns: per-page text + full markdown + page count
3. → Config prompt + document text sent to Gemini 3 Flash (via OpenRouter)
   ← Returns: hierarchical JSON (nodes, summaries, relationships, metadata)
4. OCR text is sliced by page_range and stored per-node as lazy-loadable content
5. Result cached in-memory
6. If upload=true, persisted to Supabase (4 tables)
7. Full Extraction JSON returned to caller
```

The LLM determines the document's hierarchical structure — which sections exist, what type each is, how they relate to each other — while the raw text content comes from OCR, not from the LLM.

---

## Navigating an Extraction

Always follow the **summary → structure → drill-down** pattern:

### Step 1: Check if already extracted

```
list_extractions()
```

Returns lightweight summaries with IDs. If your document is already there, skip to step 3.

### Step 2: Extract

Use one of the three methods above. Save the returned `id`.

### Step 3: Get the snapshot

```
get_extraction_snapshot({ extraction_id: "ext_..." })
```

This is the core response. It contains:

| Field | What it gives you |
|---|---|
| `summary` | 2-4 sentence overview of the entire document |
| `structure_map` | Flat index — every node's ID, label, and children. Use this to jump directly to what you need. |
| `children` | Full nested tree. Each node has `type`, `subtype`, `label`, `page_range`, `summary`, and `content_ref`. |
| `relationships` | Cross-references: `responds_to`, `references`, `decides_on`, `appeals`, `cites`, `amends` |
| `content_index` | Which nodes have loadable text, with character counts |
| `metadata` | Domain-specific data extracted by the config (e.g. case number, parties, court) |

**Read the summaries first.** Most questions can be answered from summaries alone without loading any raw text.

### Step 4: Drill down (only when needed)

```
get_content({ ref: "content://node_id", offset: 0, limit: 4000 })
```

Returns a paginated chunk:

```json
{
  "content": "--- Page 8 ---\nPETICAO INICIAL\n...",
  "offset": 0,
  "limit": 4000,
  "total_chars": 21823,
  "has_more": true
}
```

When `has_more` is `true`, request the next chunk:

```
get_content({ ref: "content://node_id", offset: 4000, limit: 4000 })
```

---

## Reference: All MCP Tool Parameters

### `list_configs`

No parameters. Returns available config names.

### `list_extractions`

No parameters. Returns all extractions (merged from memory + Supabase).

### `extract_document`

| Parameter | Required | Type | Description |
|---|---|---|---|
| `file_path` | one of three | string | Local filesystem path to PDF |
| `file_url` | one of three | string (URL) | URL to download PDF from |
| `file_base64` | one of three | string (base64) | Raw PDF content, base64-encoded |
| `file_name` | with base64 | string | Filename (required with `file_base64`, optional otherwise) |
| `config` | no | string | Config name (default: `legal_br`) |
| `upload` | no | boolean | Persist to Supabase (default: `true`) |

Provide **exactly one** of `file_path`, `file_url`, or `file_base64`.

### `get_extraction_snapshot`

| Parameter | Required | Type | Description |
|---|---|---|---|
| `extraction_id` | yes | string | The extraction ID |

### `get_node`

| Parameter | Required | Type | Description |
|---|---|---|---|
| `extraction_id` | yes | string | The extraction ID |
| `node_id` | yes | string | The node ID |

### `get_content`

| Parameter | Required | Type | Description |
|---|---|---|---|
| `ref` | yes | string | Content reference (`content://node_id` or just `node_id`) |
| `offset` | no | integer | Character offset (default: 0) |
| `limit` | no | integer | Max characters (default: 4000) |

---

## Reference: REST API Endpoints

| Endpoint | Method | Description |
|---|---|---|
| `/health` | GET | Health check |
| `/configs` | GET | List available extraction configs |
| `/configs/:name` | GET | Get a specific config |
| `/extract?config=legal_br&upload=true` | POST | Upload PDF (multipart), run extraction |
| `/extractions` | GET | List all extractions |
| `/extractions/:id/snapshot` | GET | Full tree (no raw content) |
| `/extractions/:id` | GET | Full extraction by ID |
| `/extractions/:id/node/:node_id` | GET | Get specific node |
| `/content/:ref` | GET | Lazy-load content (`?offset=0&limit=4000`) |

**Production base URL:** `https://aiapi.sciron.tech`
**MCP HTTP endpoint:** `https://mcp.sciron.tech/mcp`

---

## Configs

Domain-specific extraction configs live in `configs/*.json`. Each config defines:

- **`prompts.structure`** — The system prompt that tells the LLM how to analyze the document and what hierarchical structure to extract.
- **`node_types`** — Allowed node types with subtypes (e.g. `PETICAO` with subtypes `Inicial`, `Contestacao`).
- **`relationship_types`** — Valid cross-reference types (e.g. `responds_to`, `decides_on`).
- **`metadata_schema`** — Domain-specific metadata the LLM should extract (e.g. case number, parties, court).

Currently available:

| Config | Domain | Node Types |
|---|---|---|
| `legal_br` | Brazilian legal case files | `PETICAO`, `DECISAO`, `RECURSO`, `CERTIDAO`, `DOCUMENTO`, `GRUPO`, `SECTION` |

To add a new config, create `configs/my_domain.json` with the same structure and restart the API. See the existing `configs/legal_br.json` as a template.

---

## Supabase Persistence

When `upload=true`, the extraction is persisted across four tables in the `extraction` schema:

| Table | Contents |
|---|---|
| `extractions` | Main record (id, summary, metadata, structure_map) |
| `extraction_nodes` | Flat list of nodes (id, parent_id, type, summary, page range) |
| `node_content` | Raw OCR text per node |
| `extraction_relationships` | Cross-references between nodes |

All read endpoints (list, get, snapshot, node, content) check the in-memory cache first and fall back to Supabase automatically. Extractions survive server restarts.

### Required env vars

```
SUPABASE_URL=https://your-project.supabase.co
SUPABASE_SERVICE_ROLE_KEY=your-service-role-key
```

### Schema SQL

```sql
create schema if not exists extraction;

create table extraction.extractions (
  id text primary key,
  config_name text,
  source_file text not null,
  content_hash text,
  total_pages integer,
  summary text not null default '',
  structure_map jsonb default '[]'::jsonb,
  metadata jsonb,
  extracted_at text not null,
  extractor_version text
);

create table extraction.extraction_nodes (
  id text not null,
  extraction_id text not null references extraction.extractions(id) on delete cascade,
  parent_id text,
  type text not null,
  subtype text,
  label text,
  page_start integer,
  page_end integer,
  date text,
  author text,
  summary text not null default '',
  confidence jsonb,
  primary key (extraction_id, id)
);

create table extraction.node_content (
  extraction_id text not null,
  node_id text not null,
  content text not null,
  char_count integer,
  primary key (extraction_id, node_id),
  foreign key (extraction_id, node_id)
    references extraction.extraction_nodes(extraction_id, id) on delete cascade
);

create table extraction.extraction_relationships (
  id bigint generated always as identity primary key,
  extraction_id text not null references extraction.extractions(id) on delete cascade,
  from_node text not null,
  to_node text not null,
  relationship_type text not null
);
```

Expose the `extraction` schema through Supabase Dashboard > Settings > API > Exposed schemas.
