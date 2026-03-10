//! Gemini 3.1 Flash Lite OCR provider (uses Google AI API for PDF processing).

use super::{OcrInput, OcrPage, OcrProvider, OcrResult};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

pub struct GeminiOcrProvider {
    api_key: String,
    client: reqwest::Client,
    model: String,
}

impl GeminiOcrProvider {
    pub fn from_env(client: reqwest::Client) -> anyhow::Result<Self> {
        let api_key = std::env::var("GEMINI_API_KEY")
            .map_err(|_| anyhow::anyhow!("GEMINI_API_KEY not set"))?;
        let model =
            std::env::var("GEMINI_OCR_MODEL").unwrap_or_else(|_| "gemini-3.1-flash-lite-preview".to_string());
        Ok(Self {
            api_key,
            client,
            model,
        })
    }
}

// ── Gemini API request/response types ──────────────────────────────────────

#[derive(Serialize)]
struct GenerateContentRequest {
    contents: Vec<Content>,
    generation_config: Option<GenerationConfig>,
}

#[derive(Serialize)]
struct Content {
    parts: Vec<Part>,
}

#[derive(Serialize)]
#[serde(untagged)]
enum Part {
    Text {
        text: String,
    },
    InlineData {
        inline_data: InlineData,
    },
}

#[derive(Serialize)]
struct InlineData {
    mime_type: String,
    data: String, // base64 encoded
}

#[derive(Serialize)]
struct GenerationConfig {
    temperature: f32,
    max_output_tokens: u32,
}

#[derive(Deserialize)]
struct GenerateContentResponse {
    candidates: Vec<Candidate>,
}

#[derive(Deserialize)]
struct Candidate {
    content: CandidateContent,
}

#[derive(Deserialize)]
struct CandidateContent {
    parts: Vec<ResponsePart>,
}

#[derive(Deserialize)]
struct ResponsePart {
    text: Option<String>,
}

// ── Provider implementation ─────────────────────────────────────────────────

const OCR_PROMPT: &str = r#"You are a document OCR system. Extract ALL text from this PDF document.

Output the text in clean markdown format with:
- Preserve document structure (headings, paragraphs, lists)
- Mark page breaks with "--- Page N ---" where N is the page number
- Preserve tables as markdown tables when possible
- Include all text content, do not summarize or omit anything
- For scanned/image-based pages, extract all visible text

Return ONLY the extracted text in markdown format, no explanations."#;

#[async_trait::async_trait]
impl OcrProvider for GeminiOcrProvider {
    fn name(&self) -> &str {
        "gemini_ocr"
    }

    async fn process(&self, input: &OcrInput) -> anyhow::Result<OcrResult> {
        let (filename, data) = match input {
            OcrInput::Bytes { filename, data } => (filename.clone(), data.clone()),
            OcrInput::Url { filename, url } => {
                info!("GeminiOcrProvider: downloading from URL");
                let resp = self.client.get(url).send().await?;
                let bytes = resp.bytes().await?.to_vec();
                (filename.clone(), bytes)
            }
        };

        // Determine MIME type
        let mime_type = if filename.to_lowercase().ends_with(".pdf") {
            "application/pdf"
        } else if filename.to_lowercase().ends_with(".png") {
            "image/png"
        } else if filename.to_lowercase().ends_with(".jpg")
            || filename.to_lowercase().ends_with(".jpeg")
        {
            "image/jpeg"
        } else {
            "application/pdf" // default
        };

        let base64_data = BASE64.encode(&data);

        info!(
            "GeminiOcrProvider: processing {} ({} bytes, model={})",
            filename,
            data.len(),
            self.model
        );

        let request = GenerateContentRequest {
            contents: vec![Content {
                parts: vec![
                    Part::InlineData {
                        inline_data: InlineData {
                            mime_type: mime_type.to_string(),
                            data: base64_data,
                        },
                    },
                    Part::Text {
                        text: OCR_PROMPT.to_string(),
                    },
                ],
            }],
            generation_config: Some(GenerationConfig {
                temperature: 0.1,
                max_output_tokens: 65536,
            }),
        };

        let url = format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent?key={}",
            self.model, self.api_key
        );

        let resp = self.client.post(&url).json(&request).send().await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("Gemini API error ({}): {}", status, text);
        }

        let raw_text = resp.text().await?;
        debug!(
            "GeminiOcrProvider: raw response ({} bytes)",
            raw_text.len()
        );

        let response: GenerateContentResponse = serde_json::from_str(&raw_text)?;

        let markdown = response
            .candidates
            .first()
            .and_then(|c| c.content.parts.first())
            .and_then(|p| p.text.clone())
            .unwrap_or_default();

        // Parse pages from markdown (look for "--- Page N ---" markers)
        let pages = parse_pages_from_markdown(&markdown);
        let total_pages = if pages.is_empty() { 1 } else { pages.len() as u32 };

        info!(
            "GeminiOcrProvider: extracted {} pages, {} chars",
            total_pages,
            markdown.len()
        );

        Ok(OcrResult {
            markdown: markdown.clone(),
            pages,
            total_pages,
            metadata: serde_json::json!({
                "model": self.model,
                "provider": "gemini"
            }),
            ocr_confidence: 0.90,
            provider_name: "gemini_ocr".to_string(),
        })
    }
}

/// Parse page markers from the markdown output.
fn parse_pages_from_markdown(markdown: &str) -> Vec<OcrPage> {
    let mut pages = Vec::new();
    let mut current_page = 1u32;
    let mut current_text = String::new();

    for line in markdown.lines() {
        // Check for page marker like "--- Page 1 ---" or "--- Page 2 ---"
        if line.starts_with("--- Page ") && line.ends_with(" ---") {
            // Save previous page if we have content
            if !current_text.trim().is_empty() {
                pages.push(OcrPage {
                    page_num: current_page,
                    text: current_text.trim().to_string(),
                });
            }

            // Parse new page number
            if let Some(num_str) = line
                .strip_prefix("--- Page ")
                .and_then(|s| s.strip_suffix(" ---"))
            {
                if let Ok(num) = num_str.parse::<u32>() {
                    current_page = num;
                }
            }
            current_text.clear();
        } else {
            current_text.push_str(line);
            current_text.push('\n');
        }
    }

    // Don't forget the last page
    if !current_text.trim().is_empty() {
        pages.push(OcrPage {
            page_num: current_page,
            text: current_text.trim().to_string(),
        });
    }

    // If no page markers were found, treat the whole thing as page 1
    if pages.is_empty() && !markdown.trim().is_empty() {
        pages.push(OcrPage {
            page_num: 1,
            text: markdown.trim().to_string(),
        });
    }

    pages
}
