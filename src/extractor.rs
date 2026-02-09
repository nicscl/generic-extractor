//! Document extraction pipeline using LLM with Docling OCR.

use crate::config::ExtractionConfig;
use crate::content_store::ContentStore;
use crate::entities::{self, CompiledPatterns};
use crate::openrouter::{Message, OpenRouterClient};
use crate::schema::{
    ConfidenceScores, DocumentNode, EmbeddedReference, Extraction, Relationship, StructureMapEntry,
};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::{debug, info};

/// Page-level OCR content from Docling.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageContent {
    pub page_num: u32,
    pub text: String,
}

/// Response from Docling sidecar.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoclingResponse {
    pub markdown: String,
    pub pages: Vec<PageContent>,
    pub total_pages: u32,
    #[serde(default)]
    pub metadata: serde_json::Value,
}

/// Extraction pipeline orchestrator.
pub struct Extractor {
    client: OpenRouterClient,
    content_store: ContentStore,
}

impl Extractor {
    pub fn new(client: OpenRouterClient, content_store: ContentStore) -> Self {
        Self {
            client,
            content_store,
        }
    }

    /// Extract structure from a document using Docling OCR and LLM.
    /// Uses token-cache-friendly prompt structure: document in system, instructions in user.
    pub async fn extract(
        &self,
        filename: &str,
        docling: &DoclingResponse,
        config: &ExtractionConfig,
    ) -> Result<Extraction> {
        info!(
            "Starting extraction for: {} ({} pages, {} chars) using config: {}",
            filename,
            docling.total_pages,
            docling.markdown.len(),
            config.name
        );

        // Compute content hash from the markdown
        let content_hash = {
            let mut hasher = Sha256::new();
            hasher.update(docling.markdown.as_bytes());
            format!("{:x}", hasher.finalize())
        };

        // Build token-cache-friendly messages:
        // - System message contains config prompt + full document (CACHED PREFIX)
        // - User message contains extraction instructions (VARIABLE SUFFIX)
        let system_prompt = format!(
            "{}\n\n--- DOCUMENT START (pages 1-{}) ---\n\n{}\n\n--- DOCUMENT END ---",
            config.prompts.structure,
            docling.total_pages,
            truncate_for_context(&docling.markdown, 150000) // ~150K chars max
        );

        let user_prompt = r#"Based on the document above, extract its hierarchical structure as JSON. Return ONLY valid JSON with this structure:

{
  "summary": "2-4 sentence overview",
  "structure_map": [{"id": "...", "label": "...", "children": ["id1", "id2"]}],
  "metadata": {...},
  "children": [
    {
      "id": "unique_id",
      "type": "DOCUMENT|PETICAO|DECISAO|RECURSO|SECTION|GROUP",
      "subtype": "Specific type if applicable",
      "label": "Human readable label",
      "page_range": [start_page, end_page],
      "date": "YYYY-MM-DD if known",
      "author": "Author name if known",
      "summary": "2-4 sentence summary",
      "children": []
    }
  ],
  "relationships": [
    {"from": "id1", "to": "id2", "type": "references|responds_to|decides_on|appeals"}
  ]
}"#;

        let messages = vec![Message::system(system_prompt), Message::user(user_prompt)];

        // Call LLM for structure extraction
        debug!("Calling LLM for structure extraction (document cached in system prompt)");
        let response = self.client.chat(messages).await?;

        debug!("Raw LLM response length: {} chars", response.len());

        // Parse the JSON response
        let extracted: ExtractedStructure =
            parse_llm_json(&response).context("Failed to parse LLM structure response")?;

        // Build the Extraction object
        let mut extraction = Extraction::new(filename.to_string(), Some(config.name.clone()));
        extraction.content_hash = Some(content_hash);
        extraction.total_pages = Some(docling.total_pages);
        extraction.summary = extracted.summary;
        extraction.structure_map = extracted.structure_map;

        // Convert relationships
        extraction.relationships = extracted
            .relationships
            .into_iter()
            .map(|r| Relationship {
                from: r.from,
                to: r.to,
                rel_type: r.rel_type,
                citation: r.citation,
            })
            .collect();

        // Store metadata as-is
        extraction.metadata = extracted.metadata.unwrap_or(serde_json::Value::Null);

        // Process children and populate content_ref with page-sliced OCR
        extraction.children = self.process_children(extracted.children, &docling.pages)?;

        // Run regex-based entity extraction if config has patterns
        if !config.entity_patterns.is_empty() {
            let compiled = CompiledPatterns::compile(&config.entity_patterns);
            if !compiled.is_empty() {
                let (node_entity_map, mut ref_index) = entities::extract_entities(
                    &extraction.children,
                    &self.content_store,
                    &compiled,
                );

                // Deduplicate node_ids in the global reference index
                entities::dedup_reference_index(&mut ref_index);

                // Merge regex entities into node metadata under `_entities` key
                // LLM-provided metadata takes precedence (regex goes under `_entities`)
                merge_entities_into_nodes(&mut extraction.children, &node_entity_map);

                // Set extraction-level reference_index
                extraction.reference_index =
                    serde_json::to_value(&ref_index).unwrap_or(serde_json::Value::Null);

                info!(
                    "Entity extraction: {} entity types across {} nodes",
                    ref_index.entities.len(),
                    node_entity_map.len()
                );
            }
        }

        info!(
            "Extraction complete: {} top-level nodes, {} relationships",
            extraction.children.len(),
            extraction.relationships.len()
        );

        Ok(extraction)
    }

    /// Process extracted children, storing sliced page content.
    fn process_children(
        &self,
        nodes: Vec<ExtractedNode>,
        pages: &[PageContent],
    ) -> Result<Vec<DocumentNode>> {
        let mut result = Vec::new();

        for node in nodes {
            // Extract content for this node's page range from Docling OCR
            let content_ref = if let Some(range) = node.page_range {
                let content = slice_pages(pages, range);
                if !content.is_empty() {
                    Some(self.content_store.store(&node.id, content))
                } else {
                    None
                }
            } else {
                None
            };

            // Recursively process children
            let children = self.process_children(node.children, pages)?;

            result.push(DocumentNode {
                id: node.id,
                node_type: node.node_type,
                subtype: node.subtype,
                label: node.label,
                page_range: node.page_range,
                date: node.date,
                author: node.author,
                summary: node.summary,
                references: node
                    .references
                    .into_iter()
                    .map(|r| EmbeddedReference {
                        node: r.node,
                        ref_type: r.ref_type,
                        citation: r.citation,
                    })
                    .collect(),
                referenced_by: Vec::new(),
                content_ref,
                confidence: Some(ConfidenceScores {
                    ocr: Some(0.95), // Docling has high OCR confidence
                    extraction: Some(0.8),
                    summary: Some(0.85),
                    low_confidence_regions: Vec::new(),
                }),
                metadata: node.metadata.unwrap_or(serde_json::Value::Null),
                children,
            });
        }

        Ok(result)
    }
}

// ============================================================================
// Helper types for LLM response parsing
// ============================================================================

#[derive(Debug, serde::Deserialize)]
struct ExtractedStructure {
    summary: String,
    #[serde(default)]
    structure_map: Vec<StructureMapEntry>,
    #[serde(default)]
    relationships: Vec<ExtractedRelationship>,
    #[serde(default)]
    metadata: Option<serde_json::Value>,
    #[serde(default)]
    children: Vec<ExtractedNode>,
}

#[derive(Debug, serde::Deserialize)]
struct ExtractedNode {
    id: String,
    #[serde(rename = "type")]
    node_type: String,
    #[serde(default)]
    subtype: Option<String>,
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    page_range: Option<[u32; 2]>,
    #[serde(default)]
    date: Option<String>,
    #[serde(default)]
    author: Option<String>,
    #[serde(default)]
    summary: String,
    #[serde(default)]
    metadata: Option<serde_json::Value>,
    #[serde(default)]
    references: Vec<ExtractedRef>,
    #[serde(default)]
    children: Vec<ExtractedNode>,
}

#[derive(Debug, serde::Deserialize)]
struct ExtractedRef {
    node: String,
    #[serde(rename = "type")]
    ref_type: String,
    #[serde(default)]
    citation: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct ExtractedRelationship {
    from: String,
    to: String,
    #[serde(rename = "type")]
    rel_type: String,
    #[serde(default)]
    citation: Option<String>,
}

// ============================================================================
// Helper functions
// ============================================================================

/// Slice pages from Docling response for a given page range.
fn slice_pages(pages: &[PageContent], range: [u32; 2]) -> String {
    pages
        .iter()
        .filter(|p| p.page_num >= range[0] && p.page_num <= range[1])
        .map(|p| format!("--- Page {} ---\n{}", p.page_num, p.text))
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn truncate_for_context(text: &str, max_chars: usize) -> &str {
    if text.len() <= max_chars {
        text
    } else {
        let mut end = max_chars;
        while !text.is_char_boundary(end) && end > 0 {
            end -= 1;
        }
        &text[..end]
    }
}

/// Recursively merge extracted entities into node metadata under `_entities` key.
/// LLM-provided metadata fields are preserved; regex entities are added alongside them.
fn merge_entities_into_nodes(
    nodes: &mut [DocumentNode],
    entity_map: &std::collections::HashMap<String, serde_json::Value>,
) {
    for node in nodes.iter_mut() {
        if let Some(entities) = entity_map.get(&node.id) {
            // Ensure metadata is an object
            if node.metadata.is_null() {
                node.metadata = serde_json::Value::Object(serde_json::Map::new());
            }
            if let Some(obj) = node.metadata.as_object_mut() {
                obj.insert("_entities".to_string(), entities.clone());
            }
        }

        if !node.children.is_empty() {
            merge_entities_into_nodes(&mut node.children, entity_map);
        }
    }
}

fn parse_llm_json<T: serde::de::DeserializeOwned>(response: &str) -> Result<T> {
    // Try to extract JSON from markdown code blocks if present
    let json_str = if response.contains("```json") {
        response
            .split("```json")
            .nth(1)
            .and_then(|s| s.split("```").next())
            .unwrap_or(response)
            .trim()
    } else if response.contains("```") {
        response.split("```").nth(1).unwrap_or(response).trim()
    } else {
        response.trim()
    };

    // First validate syntax
    let _: serde_json::Value = serde_json::from_str(json_str).context(format!(
        "Invalid JSON syntax: {}",
        &json_str.chars().take(200).collect::<String>()
    ))?;

    // Parse as expected type
    serde_json::from_str(json_str).context(format!(
        "JSON structure mismatch: {}",
        &json_str.chars().take(200).collect::<String>()
    ))
}
