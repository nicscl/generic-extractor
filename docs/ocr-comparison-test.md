# OCR Provider Comparison Test

**Date:** 2026-03-10
**Document:** `0097000-48.2025.8.04.1000.pdf` (41 pages, Brazilian legal process)

## Test Setup

Extracted the same PDF using two OCR providers:
- **Gemini OCR** (`gemini-3.1-flash-lite-preview`) - Google AI API
- **Docling** - IBM document understanding library (GPU sidecar)

## Results Comparison

### Node Structure

Both providers extracted the same 5 document sections with **identical page ranges**:

| Document Section | Gemini OCR | Docling |
|------------------|------------|---------|
| Petição Inicial | p3-15 | p3-15 |
| Despacho | p33 | p33 |
| Emenda à Inicial | p36 | p36 |
| Decisão de Saneamento | p39 | p39 |
| Citação | p41 | p41 |

### Minor Differences

| Aspect | Gemini | Docling |
|--------|--------|---------|
| Type naming | `DOCUMENTO` | `DOCUMENT` |
| Label format | "Petição Inicial (Ref. mov. 1.1)" | "Mov. 1.1 - Petição Inicial" |
| Grouping nodes | None | Has `GROUP` node for "Decisões Judiciais" |
| Subtype wording | "Decisão de Saneamento" | "Decisão Saneadora" |

### Entity Extraction

Both extracted similar entities:

| Entity Type | Both |
|-------------|------|
| cpf | Yes |
| cnpj | Yes |
| date_br | Yes |
| email | Yes |
| monetary_brl | Yes |
| phone_br | Yes |
| processo_cnj | Yes |
| pnr | Gemini only (flight booking code) |
| oab | Docling only |

## Content Hash Issue

**Finding:** The `content_hash` field is computed from OCR markdown output, not the original PDF bytes.

```rust
// src/extractor.rs:48
hasher.update(ocr.markdown.as_bytes());
```

This means:
- Same PDF with different OCR providers = **different content_hash**
- Cannot use `content_hash` to deduplicate across OCR providers

**Recommendation:** If true document deduplication is needed, hash the original PDF bytes before OCR processing.

## Conclusion

Both OCR providers produce **conforming extractions** that are stored correctly in Supabase:
- Same page ranges detected
- Same document structure identified
- Compatible node types and subtypes
- No data overwrites (each extraction gets unique `ext_` ID)

The Gemini OCR is a valid alternative to Docling, especially when GPU resources are unavailable.
