//! Lightweight document section detection.
//!
//! This module provides a fast, cheap way to detect document sections
//! with their page ranges without running full extraction.

use crate::ocr::OcrResult;
use crate::openrouter::{LlmClient, Message};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::info;

/// A detected document section with page range.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentSection {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub section_type: Option<String>,
    pub page_start: u32,
    pub page_end: u32,
}

/// Result of section detection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SectionsResult {
    pub total_pages: u32,
    pub sections: Vec<DocumentSection>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ocr_metadata: Option<serde_json::Value>,
}

impl SectionsResult {
    /// Convert to markdown table format.
    pub fn to_markdown(&self) -> String {
        let mut md = String::new();
        md.push_str("# Document Sections\n\n");
        md.push_str(&format!("**Total Pages:** {}\n\n", self.total_pages));
        md.push_str("| Section | Type | Pages |\n");
        md.push_str("|---------|------|-------|\n");

        for section in &self.sections {
            let section_type = section.section_type.as_deref().unwrap_or("-");
            let pages = if section.page_start == section.page_end {
                format!("p{}", section.page_start)
            } else {
                format!("p{}-{}", section.page_start, section.page_end)
            };
            md.push_str(&format!("| {} | {} | {} |\n", section.name, section_type, pages));
        }

        md
    }
}

/// Detect document sections from OCR output using a lightweight LLM prompt.
pub async fn detect_sections(
    client: Arc<dyn LlmClient>,
    ocr: &OcrResult,
) -> Result<SectionsResult> {
    info!(
        "Detecting sections for document ({} pages, {} chars)",
        ocr.total_pages,
        ocr.markdown.len()
    );

    let system_prompt = format!(
        r#"You are a document structure analyzer. Your task is to identify the main sections/documents within a larger document.

--- DOCUMENT START (pages 1-{}) ---

{}

--- DOCUMENT END ---"#,
        ocr.total_pages,
        truncate_for_context(&ocr.markdown, 100000)
    );

    let user_prompt = r#"Analyze this document and identify all distinct sections or sub-documents within it.

For each section, determine:
1. The section name/title
2. The section type (e.g., "Petition", "Decision", "Amendment", "Citation", "Contract", "Invoice", etc.)
3. The page range (start and end page)

Return ONLY valid JSON in this format:
{
  "sections": [
    {"name": "Section Title", "section_type": "Type", "page_start": 1, "page_end": 5},
    {"name": "Another Section", "section_type": "Type", "page_start": 6, "page_end": 10}
  ]
}

Important:
- Look for clear section boundaries (headers, page breaks, different document types)
- Include all major sections, even single-page ones
- Use the actual titles/names from the document when available
- Page numbers should match the "--- Page N ---" markers in the text"#;

    let messages = vec![Message::system(system_prompt), Message::user(user_prompt)];

    let response = client.chat(messages).await?;

    // Parse the JSON response
    let extracted: ExtractedSections =
        parse_llm_json(&response).context("Failed to parse sections response")?;

    info!("Detected {} sections", extracted.sections.len());

    Ok(SectionsResult {
        total_pages: ocr.total_pages,
        sections: extracted.sections,
        ocr_metadata: if ocr.metadata.is_null() {
            None
        } else {
            Some(ocr.metadata.clone())
        },
    })
}

#[derive(Debug, Deserialize)]
struct ExtractedSections {
    sections: Vec<DocumentSection>,
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

fn parse_llm_json<T: serde::de::DeserializeOwned>(response: &str) -> Result<T> {
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

    serde_json::from_str(json_str).context(format!(
        "JSON parse error: {}",
        &json_str.chars().take(200).collect::<String>()
    ))
}
