# Generic Extractor MCP — Agent Instructions

You have access to the **Generic Extractor** MCP server, which extracts hierarchical structure from PDF documents (legal files, contracts, technical manuals, etc.) using OCR and LLM analysis.

## Available Tools

| Tool | Purpose |
|------|---------|
| `list_configs` | List available extraction configs (e.g. `legal_br`) |
| `list_extractions` | List/search extractions by `readable_id` (case number, invoice ID, etc.) |
| `extract_document` | Upload a PDF and run the full extraction pipeline |
| `get_extraction_snapshot` | Get the complete document tree (summaries only, no raw text) |
| `get_node` | Get a specific node by ID |
| `get_content` | Lazy-load the raw text content for a node (paginated) |

## Uploading a PDF for Extraction

`extract_document` accepts a PDF via **one** of three methods:

| Parameter | When to use | Example |
|-----------|-------------|---------|
| `file_path` | Local agent with filesystem access (STDIO mode) | `"/home/user/case.pdf"` |
| `file_url` | PDF is hosted at a URL (S3, public link, etc.) | `"https://bucket.s3.amazonaws.com/case.pdf"` |
| `file_base64` | No URL, no filesystem — send raw content. Requires `file_name`. | `"JVBERi0xLjQK..."` |

Choose **exactly one**. Examples:

```json
// Local file
{ "file_path": "/path/to/document.pdf", "config": "legal_br", "upload": true }

// URL download
{ "file_url": "https://example.com/document.pdf", "config": "legal_br", "upload": true }

// Base64 (must include file_name)
{ "file_base64": "JVBERi0xLjQK...", "file_name": "document.pdf", "config": "legal_br", "upload": true }
```

Set `upload: true` (default) to persist the extraction in Supabase so it survives server restarts.

## How to Navigate an Extraction

Follow the **summary → structure → drill-down** pattern to minimize token usage:

1. **Check existing**: Call `list_extractions` first to see if the document has already been extracted. If so, use the existing extraction ID.
2. **Extract** (if needed): Call `extract_document` with the PDF. Save the returned `id`.
3. **Snapshot**: Call `get_extraction_snapshot` with the extraction ID. This gives you the full tree with summaries at every node, a flat `structure_map` for quick navigation, `relationships` between documents, and a `content_index` showing which nodes have loadable content.
4. **Drill down**: When you need the actual text of a specific section, call `get_content` with the node's `content_ref` value. Use `offset` and `limit` for large sections.

## Example Workflows

### Local agent (STDIO)

```
User: "What are the defendant's main arguments in this case?"

1. extract_document({ file_path: "/path/to/case.pdf", config: "legal_br" })
   → Returns extraction with id: "ext_abc123"

2. get_extraction_snapshot({ extraction_id: "ext_abc123" })
   → See the tree. Find the "contestacao" (defendant's response) node.
   → Read its summary first — it may be enough to answer.

3. get_content({ ref: "content://contestacao_sec_merito", offset: 0, limit: 4000 })
   → Only if the summary wasn't detailed enough, load the actual text.
```

### Remote agent (HTTP) — PDF from a URL

```
User: "Analyze this contract: https://storage.example.com/contracts/2026/NDA-acme.pdf"

1. extract_document({ file_url: "https://storage.example.com/contracts/2026/NDA-acme.pdf", config: "legal_br" })
   → Server downloads the PDF and extracts it. Returns id: "ext_def456"

2. get_extraction_snapshot({ extraction_id: "ext_def456" })
   → Navigate the tree using summaries.
```

### Remote agent (HTTP) — PDF from user upload

```
User uploads a file via your UI → your app base64-encodes it

1. extract_document({ file_base64: "<base64 string>", file_name: "contract.pdf", config: "legal_br" })
   → Returns extraction with id: "ext_ghi789"

2. get_extraction_snapshot({ extraction_id: "ext_ghi789" })
   → Navigate as usual.
```

## Key Concepts

- **`readable_id`** — Human-readable document identifier extracted by the LLM (e.g. CNJ process number `0266175-44.2023.8.06.0001`, invoice number, contract ID). Present on each extraction. Use `list_extractions({ readable_id: "0266175" })` to search.
- **`reference_index`** — Global entity cross-reference index at the extraction level. Maps entity types (CPF, CNPJ, PNR, flight numbers, monetary values, etc.) to their occurrences with node IDs. Use this to answer "which nodes mention CPF X?" without loading content.
- **`metadata`** — Extraction-level structured metadata (e.g. case class, court, parties for legal docs). Node-level metadata also exists under each node's `metadata` field, with regex-extracted entities under `metadata._entities`.
- **Nodes** have types like `PETICAO`, `DECISAO`, `RECURSO`, `CERTIDAO`, `DOCUMENTO`, `SECTION`, `GRUPO`.
- **Relationships** connect nodes: `responds_to`, `references`, `decides_on`, `appeals`, `cites`, `amends`.
- **content_ref** values look like `content://node_id`. Pass them to `get_content` to load text.
- **Summaries** exist at every level. Always read summaries before loading full content — most questions can be answered from summaries alone.

## Guidelines

- **Search first**: If you know a document identifier (case number, invoice ID), use `list_extractions({ readable_id: "..." })` to find existing extractions before uploading.
- Always start with `get_extraction_snapshot` after extracting. Never skip straight to `get_content`.
- **Use `reference_index`** to find which nodes contain a specific entity (CPF, CNPJ, PNR, etc.) without loading content.
- Use summaries to decide what to drill into. Don't load all content — that defeats the purpose of the hierarchical structure.
- For cross-reference questions ("How did the judge respond to X?"), check the `relationships` array in the snapshot to find connected nodes.
- The `structure_map` in the snapshot is a flat index — useful for quickly locating nodes by label without traversing the tree.
- When content is large (`has_more: true` in the response), paginate with `offset` and `limit` rather than loading everything at once.
