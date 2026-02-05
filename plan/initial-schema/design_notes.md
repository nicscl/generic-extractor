# Hierarchical Document Extraction Schema - Design Notes

## Overview

This schema supports progressive disclosure for LLM consumption of large legal documents (and extensible to other document types). The core pattern is **summary → structure map → drill-down**, minimizing token usage while maintaining full fidelity.

---

## Architecture Decisions

### 1. Nested Tree Structure

```
extraction
├── summary (root level)
├── structure_map (flat navigation aid)
├── relationships (cross-reference graph)
├── metadata (processo info)
└── children[]
    └── DocumentNode
        ├── summary
        ├── references[] / referenced_by[]
        ├── content_ref (lazy-load)
        └── children[]
            └── DocumentNode (recursive)
```

**Rationale**: LLMs benefit from seeing parent context when processing a subtree. Nested structure means when you serialize a node, its ancestors' summaries can be included naturally.

**Trade-off**: Updating nodes requires tree traversal. Mitigate by maintaining a flat index alongside (see API patterns below).

---

### 2. Every Node Has a Summary

Even leaf nodes (individual sections) get summaries. This enables:

- **Aggressive pruning**: LLM can decide what to drill into based on summaries alone
- **Multi-resolution answers**: Answer simple questions from summaries, only fetch content for complex ones
- **Token budgeting**: Estimate depth of exploration based on available context window

**Summary generation strategy**:
- Summaries should be 2-4 sentences
- Focus on *what* and *why*, not *how* (the content has the how)
- Include key entities, dates, and outcomes
- For sections: capture the main argument/claim

---

### 3. Dual Cross-Reference Storage

**Root-level graph** (`relationships[]`):
```json
{
  "from": "contestacao",
  "to": "peticao_inicial", 
  "type": "responds_to",
  "citation": null
}
```

**Embedded in nodes** (`references[]` / `referenced_by[]`):
```json
{
  "node": "peticao_inicial",
  "type": "responds_to"
}
```

**Why both?**
- Root graph: efficient for "show me all relationships" queries, building visualizations
- Embedded: when viewing a single node, you immediately see its connections without additional lookups

**Sync strategy**: Root graph is source of truth. Embedded refs are denormalized views generated at extraction time. If graph updates, regenerate embedded refs.

---

### 4. Content by Reference

Nodes store `content_ref` (e.g., `content://peticao_inicial_sec_fatos`) rather than inline text.

**Benefits**:
- Structure map stays lightweight (<10KB even for 200-page documents)
- Content fetched on-demand
- Supports chunked storage for very long sections
- Enables different content representations (raw text, markdown, structured)

**Content store implementation options**:
- Simple: filesystem with `{node_id}.txt`
- Scalable: blob storage (S3, GCS) with content-addressed hashes
- Hybrid: inline small content (<500 tokens), reference large content

---

## LLM Access Patterns

### Pattern 1: Question Answering

```python
# 1. LLM receives root summary + structure_map
# 2. LLM decides which nodes to explore based on question
# 3. API returns requested nodes with their summaries
# 4. LLM may request content_ref for specific nodes
# 5. Final answer synthesized

def answer_question(extraction, question):
    context = {
        "summary": extraction.summary,
        "structure": extraction.structure_map,
        "relationships": extraction.relationships
    }
    
    # LLM tool: get_node(id) -> returns node with summary + children summaries
    # LLM tool: get_content(content_ref) -> returns full text
```

### Pattern 2: Navigation/Exploration

```python
# User: "What did the defendant argue about legitimidade?"

# LLM sees structure_map, identifies "contestacao" as relevant
# Fetches contestacao node -> sees children include "contestacao_sec_preliminar"
# Fetches that section's content
```

### Pattern 3: Cross-Reference Traversal

```python
# User: "How did the judge respond to the defendant's arguments?"

# LLM sees relationship: sentenca -> decides_on -> contestacao
# Fetches sentenca.children, finds fundamentacao section
# Compares with contestacao arguments
```

---

## Extraction Pipeline Notes

### Document Segmentation

For cópia integral, the index page (pg 1-3 in example) provides ground truth for segmentation:
- Document IDs, dates, types, page ranges all present
- Use this as the skeleton, then extract content from each range

For documents without index:
- Visual segmentation: headers, stamps, signatures mark boundaries
- Textual cues: "PETIÇÃO", "SENTENÇA", section numbering
- Provide extraction hints in config (e.g., "expect sections numbered 1., 2., ...")

### Summary Generation

Two approaches:
1. **Bottom-up**: Extract content first, then summarize each node, roll up to parent summaries
2. **Top-down with refinement**: Generate root summary from OCR, then refine with extracted sections

Recommend bottom-up for accuracy, but top-down is faster for initial "what is this document?" questions.

### Cross-Reference Detection

**Explicit citations**: regex for "fls. \d+", "página \d+", "conforme \w+ supra"
**Implicit by type**: contestação always responds_to petição_inicial
**Semantic**: LLM identifies "conforme alegado pela autora" → references petição_inicial

---

## Extension Points

### For Non-Legal Documents

The schema is generic enough for:
- **Technical manuals**: chapters → sections → procedures
- **Research papers**: abstract → sections → figures/tables
- **Contracts**: parties → clauses → amendments

Just adjust:
- `metadata` schema for domain-specific fields
- `type` enum for document types
- Relationship types

### Mandatory Hierarchy Support

User can provide a "template" that extraction must follow:

```yaml
required_structure:
  - id: "executive_summary"
    required: true
  - id: "findings"
    required: true
    children:
      - id: "finding_*"
        min_count: 1
```

Extractor validates output against template, flags missing required nodes.

---

## File Manifest

- `extraction_schema.json` - JSON Schema definition (for validation)
- `extraction_schema_draft.yaml` - Example output for the uploaded cópia integral
- `design_notes.md` - This file

---

## Resolved Design Decisions

### 1. Chunking Strategy: Content-Store Level (Option B)

Large sections (>2000 tokens) are **transparently chunked at the content-store level**, not in the schema.

**How it works:**
- Schema stays clean — no `CHUNK` sub-nodes
- `content_ref` points to the logical content unit
- Content-store API supports pagination:

```python
# API returns paginated content
def get_content(content_ref, offset=0, limit=2000):
    return {
        "content": "...",     # Text chunk
        "offset": 0,
        "limit": 2000,
        "total_tokens": 5000,
        "has_more": True
    }
```

**Trade-off**: LLM loses semantic guidance on chunk boundaries, but the schema remains simple and chunking logic can evolve independently.

---

### 2. Version Control ✅

Added to schema:
- `version`: Integer, increments on re-extraction
- `previous_version_id`: Links to prior extraction
- `content_hash`: Hash of source document for change detection

This enables tracking extraction history and detecting when source documents change.

---

### 3. Confidence Scores ✅

Added `ConfidenceScores` to each `DocumentNode`:

```json
{
  "confidence": {
    "ocr": 0.92,
    "extraction": 0.85,
    "summary": 0.95,
    "low_confidence_regions": [
      { "page": 3, "reason": "handwritten annotation" }
    ]
  }
}
```

Useful for flagging sections that may need human review.

---

### 4. Multi-language ❌ (Dropped)

Not needed — LLMs handle multilingual content natively. Schema remains language-agnostic without explicit `lang` fields.
