//! Document extraction pipeline using LLM.

use crate::config::ExtractionConfig;
use crate::content_store::ContentStore;
use crate::openrouter::{Message, OpenRouterClient};
use crate::schema::{
    ConfidenceScores, DocumentNode, EmbeddedReference, Extraction,
    Relationship, StructureMapEntry,
};
use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use tracing::{debug, info};

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

    /// Extract structure from a document using the specified config.
    pub async fn extract(
        &self,
        filename: &str,
        text_content: &str,
        images: Option<Vec<Vec<u8>>>,
        config: &ExtractionConfig,
    ) -> Result<Extraction> {
        info!("Starting extraction for: {} using config: {}", filename, config.name);

        // Compute content hash
        let content_hash = {
            let mut hasher = Sha256::new();
            hasher.update(text_content.as_bytes());
            format!("{:x}", hasher.finalize())
        };

        // Build messages using config prompt
        let messages = if let Some(image_data) = images {
            vec![
                Message::system(&config.prompts.structure),
                Message::user_with_images(
                    format!(
                        "Analyze this document:\n\n{}",
                        truncate_for_context(text_content, 50000)
                    ),
                    image_data,
                ),
            ]
        } else {
            vec![
                Message::system(&config.prompts.structure),
                Message::user(format!(
                    "Analyze this document:\n\n{}",
                    truncate_for_context(text_content, 100000)
                )),
            ]
        };

        // Call LLM for structure extraction
        debug!("Calling LLM for structure extraction");
        let response = self.client.chat(messages).await?;
        
        debug!("Raw LLM response length: {} chars", response.len());

        // Parse the JSON response
        let extracted: ExtractedStructure = parse_llm_json(&response)
            .context("Failed to parse LLM structure response")?;

        // Build the Extraction object
        let mut extraction = Extraction::new(filename.to_string(), Some(config.name.clone()));
        extraction.content_hash = Some(content_hash);
        extraction.summary = extracted.summary;
        extraction.structure_map = extracted.structure_map;
        
        // Convert relationships
        extraction.relationships = extracted.relationships
            .into_iter()
            .map(|r| Relationship {
                from: r.from,
                to: r.to,
                rel_type: r.rel_type,
                citation: r.citation,
            })
            .collect();
        
        // Store metadata as-is (dynamic JSON)
        extraction.metadata = extracted.metadata.unwrap_or(serde_json::Value::Null);

        // Process children and store content
        extraction.children = self.process_children(extracted.children, text_content)?;

        info!(
            "Extraction complete: {} top-level nodes, {} relationships",
            extraction.children.len(),
            extraction.relationships.len()
        );

        Ok(extraction)
    }

    /// Process extracted children, storing content and computing refs.
    fn process_children(
        &self,
        nodes: Vec<ExtractedNode>,
        full_text: &str,
    ) -> Result<Vec<DocumentNode>> {
        let mut result = Vec::new();

        for node in nodes {
            let content_ref = if !node.id.is_empty() {
                let content = extract_content_for_range(full_text, node.page_range);
                if !content.is_empty() {
                    Some(self.content_store.store(&node.id, content))
                } else {
                    None
                }
            } else {
                None
            };

            let children = self.process_children(node.children, full_text)?;

            result.push(DocumentNode {
                id: node.id,
                node_type: node.node_type,
                subtype: node.subtype,
                label: node.label,
                page_range: node.page_range,
                date: node.date,
                author: node.author,
                summary: node.summary,
                references: node.references.into_iter().map(|r| EmbeddedReference {
                    node: r.node,
                    ref_type: r.ref_type,
                    citation: r.citation,
                }).collect(),
                referenced_by: Vec::new(),
                content_ref,
                confidence: Some(ConfidenceScores {
                    ocr: None,
                    extraction: Some(0.8),
                    summary: Some(0.85),
                    low_confidence_regions: Vec::new(),
                }),
                metadata: serde_json::Value::Null,
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

fn extract_content_for_range(_full_text: &str, page_range: Option<[u32; 2]>) -> String {
    if page_range.is_some() {
        String::new()
    } else {
        String::new()
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
        response
            .split("```")
            .nth(1)
            .unwrap_or(response)
            .trim()
    } else {
        response.trim()
    };

    // First validate syntax
    let _: serde_json::Value = serde_json::from_str(json_str)
        .context(format!("Invalid JSON syntax: {}", &json_str.chars().take(200).collect::<String>()))?;
    
    // Parse as expected type
    serde_json::from_str(json_str)
        .context(format!("JSON structure mismatch: {}", &json_str.chars().take(200).collect::<String>()))
}
