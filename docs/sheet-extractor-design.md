# Sheet Extractor — Design Document

> Brainstorm session: 2026-02-11

## Objective

Add a **tabular data extraction pipeline** to the existing generic-extractor codebase. It takes spreadsheets, CSVs, or scanned PDFs containing tables, identifies the schema(s) within, normalizes the data, and stores it in a queryable format.

This complements the existing PDF document extractor (which produces hierarchical document trees) with a data-focused extractor that produces **typed, structured datasets**.

---

## Input Types

| Format | Parsing Strategy |
|--------|-----------------|
| CSV | Direct parsing in Rust (no OCR needed) |
| Excel (.xlsx, .xls) | Direct parsing in Rust via `calamine` crate (no OCR needed) |
| Scanned PDF with tables | Docling OCR sidecar → markdown tables → parse |

- For already-structured files (CSV/Excel), bypass OCR entirely and parse directly.
- The LLM agent still runs on all inputs to describe, schematize, and normalize the data.

---

## Pipeline Architecture

```
Input (CSV / Excel / scanned PDF with tables)
  │
  ├── CSV/Excel ──→ Direct Rust parsing ──→ Raw rows + column headers
  │
  ├── Scanned PDF ──→ Docling OCR ──→ Markdown tables ──→ Parse tables
  │
  ▼
Raw tabular content (rows, columns, headers, annotations)
  │
  ▼
Agent loop (multi-turn LLM):
  │  Prompt structure:
  │    - System: generic strategy prompt (non-business-specific)
  │    - User/config: business-specific context from config
  │
  │  Turn 1: "Here are the first N rows from page/sheet 1 — what schemas do you see?"
  │  Turn 2: "Here's page/sheet 3 — same schema or new one?"
  │  Turn N: "Finalize schemas, define transforms, classify all rows"
  │
  ▼
Schema definitions + column transforms + classified rows
  │
  ▼
Transform phase (Rust, declarative):
  │  Apply typed transforms: parse_date_br, parse_currency_brl, etc.
  │
  ▼
Structured output: typed datasets with relationships
  │
  ▼
Storage:
  ├── JSONB in `extraction.datasets` table (initial)
  └── Optional "upgrade" to real normalized Supabase tables (future)
```

---

## Key Design Decisions

### 1. Per-page uniformity assumption

Each page of a scanned PDF is expected to have a uniform table layout. Different pages may have different schemas (e.g., pages 1-3 are transactions, pages 4-5 are a payment schedule with different columns).

### 2. Non-tabular content becomes metadata columns

Headers, stamps, annotations, and other non-tabular content on a page are NOT discarded. Instead, they are captured as additional columns on the data rows they relate to.

**Example:** If a page header says "Financial Data — May 2025", the agent should add a `periodo` column with value `"May 2025"` to every row extracted from that page. This preserves context that would otherwise be lost.

### 3. Two-part prompt architecture

Same pattern as the existing PDF extractor:
- **System prompt (generic):** Specifies the general strategy, output structure, task description. Shared across all configs. Contains the document/data for token caching.
- **User prompt (config-driven):** Business-specific context from the config file. Expected columns, domain classification rules, data format hints, required vs optional fields.

### 4. Declarative transforms (not Python scripts)

Data cleaning/normalization uses a finite set of **built-in transforms** applied in Rust:

```
parse_date_br       — DD/MM/YYYY → ISO8601
parse_date_us       — MM/DD/YYYY → ISO8601
parse_currency_brl  — "1.234,56" → 1234.56
parse_currency_usd  — "1,234.56" → 1234.56
normalize_cpf       — "123.456.789-00" → "12345678900"
normalize_cnpj      — strip punctuation
strip_whitespace    — trim + collapse internal whitespace
to_uppercase        — uppercase
to_lowercase        — lowercase
to_number           — parse as float
to_integer          — parse as int
```

The LLM agent picks from this menu when defining column transforms. This is:
- Fast (native Rust)
- Sandboxed (no code execution)
- Deterministic (same input → same output)
- Easy to serialize alongside schema definitions

If edge cases arise later that need custom logic, a Python escape hatch can be added.

### 5. JSONB storage with Supabase persistence

**Status: Implemented.** Datasets are stored in two Supabase tables within the `extraction` schema:

```sql
CREATE TABLE extraction.datasets (
    id TEXT PRIMARY KEY,
    source_file TEXT NOT NULL,
    config_name TEXT,
    extracted_at TIMESTAMPTZ DEFAULT NOW(),
    summary TEXT,
    schemas JSONB NOT NULL,         -- array of schema definitions (columns only, no rows)
    relationships JSONB,            -- cross-schema relationships
    status TEXT DEFAULT 'processing'
);

CREATE TABLE extraction.dataset_rows (
    id TEXT PRIMARY KEY,
    dataset_id TEXT REFERENCES extraction.datasets(id),
    schema_name TEXT NOT NULL,       -- which schema this row belongs to
    row_data JSONB NOT NULL,         -- the actual row as key-value pairs
    page_num INTEGER,                -- source page (for scanned PDFs)
    row_index INTEGER                -- original row position
);

CREATE INDEX idx_dataset_rows_dataset ON extraction.dataset_rows(dataset_id);
CREATE INDEX idx_dataset_rows_schema ON extraction.dataset_rows(dataset_id, schema_name);
```

Migration file: `migrations/003_datasets.sql`

**Persistence strategy:**
- File-based backup: datasets always saved to `data/datasets/{id}.json`
- Supabase upload: when `upload=true` (default), rows batch-inserted 100 at a time
- Hydration: on cache miss, `get_or_hydrate_dataset()` fetches from Supabase and caches in memory
- Listing: merges in-memory + Supabase datasets (dedup by ID)

This supports:
- JSONB querying via Supabase (`row_data->>'valor'`, filters, etc.)
- Reasonable performance for tens of thousands of rows
- No dynamic DDL
- Survival across server restarts

**Future upgrade path:** An optional operation that reads a dataset's schema, creates a real normalized Supabase table with proper columns/types, and migrates the JSONB rows into it. Triggered manually (not automatic).

### 6. Multi-turn agent loop

Schema discovery is iterative:
1. Agent receives a sample of the data (first N rows, or first page)
2. Hypothesizes schema(s): column names, types, table boundaries
3. Receives more data (next page/chunk) to validate or refine
4. Identifies relationships between schemas (e.g., foreign key patterns)
5. Outputs final schema definitions with transforms

This avoids single-shot failures on complex multi-schema documents.

### 7. Cross-schema relationships

The agent should identify relationships between discovered schemas:

```json
{
  "from": "transacoes.categoria_id",
  "to": "categorias.id",
  "type": "references"
}
```

Non-tabular content (headers, section titles) helps the agent understand these relationships.

### 8. Config system

Reuses the existing `configs/*.json` system. A sheet extraction config includes:

```json
{
    "name": "financial_br",
    "description": "Brazilian financial spreadsheets",
    "prompts": {
        "structure": "Generic strategy prompt for tabular extraction..."
    },
    "expected_columns": [
        {
            "name": "data",
            "type": "date",
            "format": "DD/MM/YYYY",
            "required": true
        },
        {
            "name": "valor",
            "type": "currency_brl",
            "required": true
        },
        {
            "name": "descricao",
            "type": "string",
            "required": false
        }
    ],
    "classification_hints": "Valores monetários estão em BRL. Datas seguem DD/MM/YYYY...",
    "entity_patterns": [...]
}
```

- **`expected_columns`**: Defines what the agent should look for. Required columns cause failure if not found. Optional columns are extracted if present.
- **`classification_hints`**: Business-specific context injected into the LLM prompt.

---

## Output Shape

```json
{
    "id": "ds_abc123",
    "extraction_id": "ext_...",
    "source_file": "financeiro.xlsx",
    "summary": "Planilha financeira com 342 transações de maio/2025 e resumo mensal por categoria.",
    "schemas": [
        {
            "name": "transacoes_financeiras",
            "description": "Transações financeiras de maio 2025",
            "source_pages": [1, 2, 3, 4, 5],
            "columns": [
                {"name": "data", "type": "date", "format": "DD/MM/YYYY", "transform": "parse_date_br"},
                {"name": "descricao", "type": "string", "transform": "strip_whitespace"},
                {"name": "valor", "type": "currency_brl", "transform": "parse_currency_brl"},
                {"name": "categoria_id", "type": "string"},
                {"name": "periodo", "type": "string", "source": "header", "description": "Extracted from page header"}
            ],
            "row_count": 342,
            "rows": [
                {"data": "2025-05-01", "descricao": "Pagamento fornecedor", "valor": 1234.56, "categoria_id": "cat_01", "periodo": "Maio 2025"},
                ...
            ]
        },
        {
            "name": "resumo_mensal",
            "description": "Resumo consolidado por categoria",
            "source_pages": [6],
            "columns": [
                {"name": "id", "type": "string"},
                {"name": "categoria", "type": "string"},
                {"name": "total", "type": "currency_brl", "transform": "parse_currency_brl"}
            ],
            "row_count": 8,
            "rows": [...]
        }
    ],
    "relationships": [
        {
            "from": "transacoes_financeiras.categoria_id",
            "to": "resumo_mensal.id",
            "type": "references"
        }
    ]
}
```

---

## API Endpoints

### `POST /extract-sheet`

**Query params** (same pattern as `/extract`):
- `config` — config name (default: `financial_br`)
- `upload` — persist to Supabase (default: `true`)
- `ocr_provider` — only relevant for scanned PDFs (default: `docling`)

**Input:** Multipart file upload

**Response:** Returns immediately with dataset ID + `"processing"` status (async, same pattern as `/extract`).

### `GET /datasets`

List all datasets (merges in-memory + Supabase). Returns lightweight summaries: id, status, source_file, config_name, extracted_at, summary, schema_count, total_rows.

### `GET /datasets/:id`

Get full dataset by ID. Hydrates from Supabase on cache miss.

### `GET /datasets/:id/rows`

Paginated row query for a specific schema within a dataset.

**Query params:**
- `schema_name` — required, the schema to query (e.g. `card_transactions`)
- `offset` — row offset (default: `0`)
- `limit` — max rows to return (default: `100`)

**Response:** `Vec<Value>` — just the row_data objects.

### MCP Tools

Four MCP tools expose the dataset pipeline to agents:

| Tool | Description |
|------|-------------|
| `extract_sheet` | Upload CSV/Excel/PDF and extract tabular data |
| `list_datasets` | List all datasets with summaries |
| `get_dataset` | Get complete dataset with schemas and rows |
| `query_dataset_rows` | Paginated row access for a specific schema |

---

## Codebase Integration

- **Same Rust server** (`src/main.rs`) — new route + handler
- **Same config system** (`src/config.rs`, `configs/`) — extended for sheet configs
- **Same OCR providers** (`src/ocr/`) — used only for scanned PDF inputs
- **Same Supabase client** (`src/supabase.rs`) — extended with `upload_dataset`, `list_datasets`, `fetch_dataset`, `query_dataset_rows`
- **Same OpenRouter client** (`src/openrouter.rs`) — used for multi-turn agent
- **MCP server** (`mcp-server/src/index.ts`) — 4 new tools: `extract_sheet`, `list_datasets`, `get_dataset`, `query_dataset_rows`
- **New modules:**
  - `src/sheet_extractor.rs` — multi-turn agent loop, schema discovery, transform application
  - `src/sheet_parser.rs` — CSV/Excel direct parsing, table extraction from markdown
  - `src/sheet_schema.rs` — `SheetExtraction`, `DataSchema`, `ColumnDef`, `SchemaRelationship` types

---

## Future Work

- **Omni-extractor:** A meta-endpoint that inspects input content and routes to the appropriate extractor (document tree vs tabular data vs mixed). Could handle PDFs with both narrative text and embedded tables.
- **Table upgrade:** Promote a JSONB dataset to a real Supabase table with typed columns.
- **Python transform escape hatch:** For edge cases not covered by built-in transforms.
- **Schema versioning:** Handle files with evolving schemas (same source, slightly different columns over time).
- **Streaming extraction:** For very large files, process and emit rows incrementally.
