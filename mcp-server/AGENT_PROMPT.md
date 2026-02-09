# Generic Extractor MCP — Agent Instructions

You have access to the **Generic Extractor** MCP server, which extracts hierarchical structure from PDF documents (legal files, contracts, technical manuals, etc.) using OCR and LLM analysis.

## Available Tools

| Tool | Purpose |
|------|---------|
| `list_configs` | List available extraction configs (e.g. `legal_br`) |
| `list_extractions` | List all existing extractions with IDs, summaries, and metadata |
| `extract_document` | Upload a PDF and run the full extraction pipeline |
| `get_extraction_snapshot` | Get the complete document tree (summaries only, no raw text) |
| `get_node` | Get a specific node by ID |
| `get_content` | Lazy-load the raw text content for a node (paginated) |

## How to Navigate an Extraction

Follow the **summary → structure → drill-down** pattern to minimize token usage:

1. **Check existing**: Call `list_extractions` first to see if the document has already been extracted. If so, use the existing extraction ID.
2. **Extract** (if needed): Call `extract_document` with the PDF path. Save the returned `id`.
3. **Snapshot**: Call `get_extraction_snapshot` with the extraction ID. This gives you the full tree with summaries at every node, a flat `structure_map` for quick navigation, `relationships` between documents, and a `content_index` showing which nodes have loadable content.
3. **Drill down**: When you need the actual text of a specific section, call `get_content` with the node's `content_ref` value. Use `offset` and `limit` for large sections.

## Example Workflow

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

## Key Concepts

- **Nodes** have types like `PETICAO`, `DECISAO`, `RECURSO`, `CERTIDAO`, `DOCUMENTO`, `SECTION`, `GRUPO`.
- **Relationships** connect nodes: `responds_to`, `references`, `decides_on`, `appeals`, `cites`, `amends`.
- **content_ref** values look like `content://node_id`. Pass them to `get_content` to load text.
- **Summaries** exist at every level. Always read summaries before loading full content — most questions can be answered from summaries alone.

## Guidelines

- Always start with `get_extraction_snapshot` after extracting. Never skip straight to `get_content`.
- Use summaries to decide what to drill into. Don't load all content — that defeats the purpose of the hierarchical structure.
- For cross-reference questions ("How did the judge respond to X?"), check the `relationships` array in the snapshot to find connected nodes.
- The `structure_map` in the snapshot is a flat index — useful for quickly locating nodes by label without traversing the tree.
- When content is large (`has_more: true` in the response), paginate with `offset` and `limit` rather than loading everything at once.
