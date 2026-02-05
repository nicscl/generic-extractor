//! Document extraction pipeline using LLM.

use crate::content_store::ContentStore;
use crate::openrouter::{Message, OpenRouterClient};
use crate::schema::{
    ConfidenceScores, DocumentNode, DocumentNodeType, EmbeddedReference, Extraction,
    ProcessoMetadata, Relationship, RelationshipType, StructureMapEntry,
};
use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use tracing::{debug, info};

/// System prompt for structure extraction.
const STRUCTURE_PROMPT: &str = r#"You are a legal document analyzer. Analyze the provided document and extract its hierarchical structure.

For Brazilian legal documents (cópia integral), identify:
1. Document type (petição, decisão, recurso, certidão, documento)
2. Sections within each document
3. Page ranges
4. Authors and dates when visible
5. Cross-references between documents

Return a JSON object with this structure:
{
  "summary": "2-4 sentence overview of the entire document",
  "structure_map": [
    {"id": "doc_id", "label": "Human Label", "children": ["child_id1", "child_id2"]}
  ],
  "metadata": {
    "numero": "process number if visible",
    "classe": "process class",
    "orgao_julgador": "court",
    "partes": [{"id": "parte_1", "nome": "Name", "polo": "ATIVO or PASSIVO"}]
  },
  "children": [
    {
      "id": "unique_id",
      "type": "PETIÇÃO|DECISÃO|RECURSO|CERTIDÃO|DOCUMENTO|GRUPO|SECTION",
      "subtype": "Specific type (e.g., Petição Inicial, Contestação)",
      "label": "Display label for sections",
      "page_range": [start, end],
      "date": "YYYY-MM-DD if known",
      "author": "Author name",
      "summary": "2-4 sentence summary of this node",
      "children": []
    }
  ],
  "relationships": [
    {"from": "node_id", "to": "target_id", "type": "responds_to|references|decides_on|appeals"}
  ]
}

Be thorough but concise. Focus on the document structure, not full content extraction."#;

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

    /// Extract structure from a document.
    ///
    /// - `filename`: Original filename
    /// - `text_content`: Extracted text from PDF (or OCR result)
    /// - `images`: Optional page images for vision-based extraction
    pub async fn extract(
        &self,
        filename: &str,
        text_content: &str,
        images: Option<Vec<Vec<u8>>>,
    ) -> Result<Extraction> {
        info!("Starting extraction for: {}", filename);

        // Compute content hash
        let content_hash = {
            let mut hasher = Sha256::new();
            hasher.update(text_content.as_bytes());
            format!("{:x}", hasher.finalize())
        };

        // Build messages for LLM
        let messages = if let Some(image_data) = images {
            vec![
                Message::system(STRUCTURE_PROMPT),
                Message::user_with_images(
                    format!(
                        "Analyze this document. Extracted text:\n\n{}",
                        truncate_for_context(text_content, 50000)
                    ),
                    image_data,
                ),
            ]
        } else {
            vec![
                Message::system(STRUCTURE_PROMPT),
                Message::user(format!(
                    "Analyze this document:\n\n{}",
                    truncate_for_context(text_content, 100000)
                )),
            ]
        };

        // Call LLM for structure extraction
        debug!("Calling LLM for structure extraction");
        let response = self.client.chat(messages).await?;
        
        // Log raw response for debugging
        debug!("Raw LLM response length: {} chars", response.len());
        debug!("Raw LLM response (first 2000 chars): {}", &response.chars().take(2000).collect::<String>());

        // Parse the JSON response
        let extracted: ExtractedStructure = parse_llm_json(&response)
            .context(format!("Failed to parse LLM structure response. First 500 chars: {}", &response.chars().take(500).collect::<String>()))?;

        // Build the Extraction object
        let mut extraction = Extraction::new(filename.to_string());
        extraction.content_hash = Some(content_hash);
        extraction.summary = extracted.summary;
        extraction.structure_map = extracted.structure_map;
        
        // Convert relationships from flexible to strict type
        extraction.relationships = extracted.relationships
            .into_iter()
            .map(Relationship::from)
            .collect();
        
        // Convert metadata if present
        extraction.metadata = extracted.metadata.map(|m| {
            ProcessoMetadata {
                id: format!("meta_{}", uuid::Uuid::new_v4().simple()),
                summary: None,
                numero: m.numero,
                classe: m.classe,
                orgao_julgador: m.orgao_julgador,
                ultima_distribuicao: None,
                valor_causa: None,
                valor_causa_moeda: "BRL".to_string(),
                assuntos: Vec::new(),
                nivel_sigilo: None,
                justica_gratuita: None,
                partes: Vec::new(), // TODO: parse partes from m.partes
                terceiros: Vec::new(),
            }
        });

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
                // Extract content for this node's page range if available
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
                node_type: parse_node_type(&node.node_type),
                subtype: node.subtype,
                label: node.label,
                page_range: node.page_range,
                pdf_page_range: None,
                date: node.date,
                author: node.author,
                summary: node.summary,
                references: node.references.into_iter().map(|r| EmbeddedReference {
                    node: r.node,
                    ref_type: r.ref_type,
                    citation: r.citation,
                }).collect(),
                referenced_by: Vec::new(), // Will be populated from relationships
                content_ref,
                confidence: Some(ConfidenceScores {
                    ocr: None,
                    extraction: Some(0.8), // Default confidence
                    summary: Some(0.85),
                    low_confidence_regions: Vec::new(),
                }),
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
    metadata: Option<ExtractedMetadata>,
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

/// Flexible relationship type for LLM parsing (uses String instead of enum)
#[derive(Debug, serde::Deserialize)]
struct ExtractedRelationship {
    from: String,
    to: String,
    #[serde(rename = "type")]
    rel_type: String,
    #[serde(default)]
    citation: Option<String>,
}

impl From<ExtractedRelationship> for Relationship {
    fn from(r: ExtractedRelationship) -> Self {
        let rel_type = match r.rel_type.to_lowercase().as_str() {
            "responds_to" => RelationshipType::RespondsTo,
            "references" => RelationshipType::References,
            "decides_on" => RelationshipType::DecidesOn,
            "appeals" => RelationshipType::Appeals,
            "cites" => RelationshipType::Cites,
            "amends" => RelationshipType::Amends,
            "supersedes" => RelationshipType::Supersedes,
            _ => RelationshipType::References, // default fallback
        };
        Relationship {
            from: r.from,
            to: r.to,
            rel_type,
            citation: r.citation,
        }
    }
}

/// Flexible metadata for LLM parsing
#[derive(Debug, Default, serde::Deserialize)]
struct ExtractedMetadata {
    #[serde(default)]
    numero: Option<String>,
    #[serde(default)]
    classe: Option<String>,
    #[serde(default)]
    orgao_julgador: Option<String>,
    #[serde(default)]
    partes: Vec<serde_json::Value>, // Accept any format
}

// ============================================================================
// Helper functions
// ============================================================================

fn parse_node_type(s: &str) -> DocumentNodeType {
    match s.to_uppercase().as_str() {
        "PETIÇÃO" | "PETICAO" => DocumentNodeType::Peticao,
        "DECISÃO" | "DECISAO" => DocumentNodeType::Decisao,
        "RECURSO" => DocumentNodeType::Recurso,
        "CERTIDÃO" | "CERTIDAO" => DocumentNodeType::Certidao,
        "DOCUMENTO" => DocumentNodeType::Documento,
        "GRUPO" => DocumentNodeType::Grupo,
        "SECTION" => DocumentNodeType::Section,
        _ => DocumentNodeType::Documento,
    }
}

fn truncate_for_context(text: &str, max_chars: usize) -> &str {
    if text.len() <= max_chars {
        text
    } else {
        // Find a safe UTF-8 boundary
        let mut end = max_chars;
        while !text.is_char_boundary(end) && end > 0 {
            end -= 1;
        }
        &text[..end]
    }
}

fn extract_content_for_range(_full_text: &str, page_range: Option<[u32; 2]>) -> String {
    // For now, we don't have page-level text extraction, so return empty
    // In a real implementation, this would use page markers or PDF structure
    if page_range.is_some() {
        // Placeholder: would extract text for specific pages
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

    // First try parsing as Value to validate JSON syntax
    let _: serde_json::Value = serde_json::from_str(json_str)
        .context(format!("Invalid JSON syntax. First 300 chars: {}", &json_str.chars().take(300).collect::<String>()))?;
    
    // Then parse as the expected type
    serde_json::from_str(json_str)
        .context(format!("JSON structure mismatch. First 300 chars: {}", &json_str.chars().take(300).collect::<String>()))
}
