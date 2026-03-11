//! Lightweight document section detection.
//!
//! This module provides a fast, cheap way to detect document sections
//! with their page ranges without running full extraction.
//!
//! Supports two modes:
//! - `full`: Analyze entire document at once (default)
//! - `page`: Analyze page-by-page with Gemini for finer granularity

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

/// Detection mode for section analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DetectionMode {
    /// Analyze entire document at once (faster, cheaper)
    #[default]
    Full,
    /// Analyze page-by-page with Gemini (finer granularity)
    Page,
}

impl DetectionMode {
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "page" | "page-by-page" | "pages" => DetectionMode::Page,
            _ => DetectionMode::Full,
        }
    }
}

#[derive(Debug, Deserialize)]
struct ExtractedSections {
    sections: Vec<DocumentSection>,
}

/// Page-level section info detected by Gemini.
#[derive(Debug, Clone, Deserialize)]
struct PageInfo {
    page: u32,
    section_name: Option<String>,
    section_type: Option<String>,
    is_new_section: bool,
}

/// Detect sections page-by-page using Gemini API directly.
/// This provides finer granularity by analyzing each page individually.
pub async fn detect_sections_page_by_page(
    http_client: &reqwest::Client,
    ocr: &OcrResult,
    model: Option<&str>,
) -> Result<SectionsResult> {
    let api_key = std::env::var("GEMINI_API_KEY")
        .map_err(|_| anyhow::anyhow!("GEMINI_API_KEY not set for page-by-page mode"))?;

    let model = model.unwrap_or("gemini-2.5-flash-lite-preview");

    info!(
        "Page-by-page section detection ({} pages, model={})",
        ocr.total_pages, model
    );

    let mut page_infos: Vec<PageInfo> = Vec::new();

    for page in &ocr.pages {
        let page_info = analyze_single_page(
            http_client,
            &api_key,
            model,
            page.page_num,
            &page.text,
            page_infos.last().and_then(|p| p.section_name.as_deref()),
        ).await?;

        info!("Page {}: {} (new={})",
            page.page_num,
            page_info.section_name.as_deref().unwrap_or("?"),
            page_info.is_new_section
        );

        page_infos.push(page_info);
    }

    // Aggregate page infos into sections
    let sections = aggregate_page_infos(&page_infos);

    info!("Aggregated {} sections from {} pages", sections.len(), ocr.total_pages);

    Ok(SectionsResult {
        total_pages: ocr.total_pages,
        sections,
        ocr_metadata: if ocr.metadata.is_null() {
            None
        } else {
            Some(ocr.metadata.clone())
        },
    })
}

async fn analyze_single_page(
    client: &reqwest::Client,
    api_key: &str,
    model: &str,
    page_num: u32,
    page_text: &str,
    previous_section: Option<&str>,
) -> Result<PageInfo> {
    let context = if let Some(prev) = previous_section {
        format!("The previous page was part of section: \"{}\"", prev)
    } else {
        "This is the first page of the document.".to_string()
    };

    let prompt = format!(
        r#"Analyze this page from a legal/business document and identify its section.

Page {page_num}:
```
{page_text}
```

{context}

Determine:
1. What section/document this page belongs to
2. The section type (Petition, Decision, Contract, Invoice, Receipt, Certificate, etc.)
3. Whether this is the START of a new section or a continuation

Return ONLY valid JSON:
{{"page": {page_num}, "section_name": "Section Title", "section_type": "Type", "is_new_section": true/false}}"#,
        page_num = page_num,
        page_text = truncate_for_context(page_text, 8000),
        context = context
    );

    let request = serde_json::json!({
        "contents": [{
            "parts": [{"text": prompt}]
        }],
        "generationConfig": {
            "temperature": 0.1,
            "maxOutputTokens": 256
        }
    });

    let url = format!(
        "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent?key={}",
        model, api_key
    );

    let resp = client.post(&url).json(&request).send().await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        anyhow::bail!("Gemini API error ({}): {}", status, text);
    }

    let body: serde_json::Value = resp.json().await?;

    let response_text = body["candidates"][0]["content"]["parts"][0]["text"]
        .as_str()
        .unwrap_or("{}");

    Ok(parse_llm_json::<PageInfo>(response_text).unwrap_or(PageInfo {
        page: page_num,
        section_name: None,
        section_type: None,
        is_new_section: false,
    }))
}

fn aggregate_page_infos(pages: &[PageInfo]) -> Vec<DocumentSection> {
    let mut sections: Vec<DocumentSection> = Vec::new();

    for page in pages {
        if page.is_new_section || sections.is_empty() {
            // Start a new section
            sections.push(DocumentSection {
                name: page.section_name.clone().unwrap_or_else(|| format!("Section {}", sections.len() + 1)),
                section_type: page.section_type.clone(),
                page_start: page.page,
                page_end: page.page,
            });
        } else if let Some(current) = sections.last_mut() {
            // Extend the current section
            current.page_end = page.page;
            // Update section info if we got better data
            if current.name.starts_with("Section ") {
                if let Some(ref name) = page.section_name {
                    current.name = name.clone();
                }
            }
            if current.section_type.is_none() {
                current.section_type = page.section_type.clone();
            }
        }
    }

    sections
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
