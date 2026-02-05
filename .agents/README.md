# Generic Extractor

## Overview

A hierarchical document extraction system designed for LLM consumption of large documents (initially targeting Brazilian legal documents, extensible to other domains).

## Core Pattern

**Summary → Structure Map → Drill-down**

The system enables progressive disclosure, allowing LLMs to navigate large documents efficiently by:
1. Reading a high-level summary first
2. Consulting a flat structure map for navigation
3. Drilling down into specific nodes only when needed
4. Fetching full content on-demand via lazy-loading

## Key Features

- **Nested tree structure** - Nodes contain children directly, preserving parent context
- **Summaries at every level** - Every node (even leaf sections) has a 2-4 sentence summary
- **Dual cross-references** - Root-level graph for global queries + embedded refs per node
- **Content by reference** - Structure stays lightweight (<10KB); content fetched via `content://{node_id}` URIs
- **Extensible metadata** - Domain-specific fields (legal, technical, contracts, etc.)

## Document Types Supported

### Legal (Primary)
- Petições (Inicial, Contestação, Réplica)
- Decisões (Sentença, Despacho, Acórdão)
- Recursos (Apelação, Agravo, Embargos)
- Certidões e Documentos anexos

### Extensible To
- Technical manuals (chapters → sections → procedures)
- Research papers (abstract → sections → figures/tables)
- Contracts (parties → clauses → amendments)

## Schema Files

| File | Purpose |
|------|---------|
| `plan/initial-schema/extraction_schema.json` | JSON Schema for validation |
| `plan/initial-schema/extraction_schema_draft.yaml` | Example extraction output |
| `plan/initial-schema/design_notes.md` | Architecture decisions and rationale |

## LLM Access Patterns

### Pattern 1: Question Answering
1. LLM receives root summary + structure_map
2. LLM decides which nodes to explore
3. API returns requested nodes with summaries
4. LLM requests content_ref for specific nodes if needed
5. Final answer synthesized

### Pattern 2: Navigation/Exploration
LLM uses structure_map to identify relevant documents, then drills down through children.

### Pattern 3: Cross-Reference Traversal
LLM follows relationship graph (responds_to, decides_on, appeals, etc.) to understand document connections.

## Relationship Types

| Type | Description |
|------|-------------|
| `responds_to` | Document replies to another (e.g., contestação → petição inicial) |
| `references` | Cites or mentions another document |
| `decides_on` | Judicial decision about a petition |
| `appeals` | Recurso challenging a decision |
| `cites` | Legal citation |
| `amends` | Modification of previous document |
| `supersedes` | Replaces previous document |

## Open Questions

1. **Chunking**: Auto-chunk sections >2000 tokens into sub-nodes, or handle at content_ref level?
2. **Version control**: ID stability when re-extracting with better OCR?
3. **Confidence scores**: Include extraction confidence for OCR-uncertain sections?
4. **Multi-language**: Schema is language-agnostic; add `lang` field to summaries?
