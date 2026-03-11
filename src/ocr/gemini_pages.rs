//! Gemini page-by-page OCR provider.
//! Converts each PDF page to an image and processes individually for better quality.

use super::{OcrInput, OcrPage, OcrProvider, OcrResult};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use serde::{Deserialize, Serialize};
use std::process::Command;
use tempfile::TempDir;
use tracing::{debug, info, warn};

pub struct GeminiPageByPageProvider {
    api_key: String,
    client: reqwest::Client,
    model: String,
}

impl GeminiPageByPageProvider {
    pub fn from_env(client: reqwest::Client) -> anyhow::Result<Self> {
        let api_key = std::env::var("GEMINI_API_KEY")
            .map_err(|_| anyhow::anyhow!("GEMINI_API_KEY not set"))?;
        let model = std::env::var("GEMINI_OCR_PAGES_MODEL")
            .unwrap_or_else(|_| "gemini-2.5-flash-lite".to_string());
        Ok(Self {
            api_key,
            client,
            model,
        })
    }
}

// ── Single page OCR prompt ──────────────────────────────────────────────────

const SINGLE_PAGE_PROMPT: &str = r#"You are a precise OCR system. TRANSCRIBE ALL TEXT from this single page image.

YOUR TASK:
Carefully examine this page and transcribe EVERY piece of text you can see. Do not skip anything.

TRANSCRIBE ALL OF THE FOLLOWING:
- Main body text (paragraphs, sentences)
- Headers, titles, subtitles
- Page numbers, dates, timestamps
- Document reference numbers, case numbers, protocol numbers
- Names of people, companies, institutions
- Addresses, phone numbers, emails
- Table contents (ALL rows and columns)
- Form field labels AND their filled values
- Handwritten text or signatures (describe if illegible: "[assinatura ilegível]")
- Stamps, seals, watermarks (transcribe visible text)
- Footnotes, annotations, margin notes
- Captions, labels on images/charts
- Legal disclaimers, fine print
- Barcodes/QR codes (note: "[código de barras]" or "[QR code]")

FORMAT RULES:
- Use markdown for structure (## headings, **bold**, tables, lists)
- Preserve the reading order: top to bottom, left to right
- For tables: use markdown table format with | separators
- Keep original line breaks where meaningful
- If text is partially visible or unclear: [texto parcial: "..."]

CRITICAL: Every word, number, date, and symbol matters. Missing information could be critical.
Do NOT add explanations or commentary. Output ONLY the transcribed text."#;

// ── Gemini API types ────────────────────────────────────────────────────────

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
    Text { text: String },
    InlineData { inline_data: InlineData },
}

#[derive(Serialize)]
struct InlineData {
    mime_type: String,
    data: String,
}

#[derive(Serialize)]
struct GenerationConfig {
    temperature: f32,
    max_output_tokens: u32,
}

#[derive(Deserialize)]
struct GenerateContentResponse {
    candidates: Option<Vec<Candidate>>,
    #[serde(rename = "usageMetadata")]
    usage_metadata: Option<UsageMetadata>,
}

#[derive(Deserialize, Debug, Clone)]
struct UsageMetadata {
    #[serde(rename = "promptTokenCount")]
    prompt_token_count: Option<u32>,
    #[serde(rename = "candidatesTokenCount")]
    candidates_token_count: Option<u32>,
    #[serde(rename = "totalTokenCount")]
    total_token_count: Option<u32>,
}

#[derive(Deserialize)]
struct Candidate {
    content: Option<CandidateContent>,
    #[serde(rename = "finishReason", default)]
    finish_reason: Option<String>,
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

#[async_trait::async_trait]
impl OcrProvider for GeminiPageByPageProvider {
    fn name(&self) -> &str {
        "gemini_ocr_pages"
    }

    async fn process(&self, input: &OcrInput) -> anyhow::Result<OcrResult> {
        let (filename, data) = match input {
            OcrInput::Bytes { filename, data } => (filename.clone(), data.clone()),
            OcrInput::Url { filename, url } => {
                info!("GeminiPageByPageProvider: downloading from URL");
                let resp = self.client.get(url).send().await?;
                let bytes = resp.bytes().await?.to_vec();
                (filename.clone(), bytes)
            }
        };

        // Check if it's a PDF
        let is_pdf = filename.to_lowercase().ends_with(".pdf");

        if !is_pdf {
            // For single images, just process directly
            return self.process_single_image(&data, &filename, 1).await;
        }

        // Convert PDF pages to images
        let temp_dir = TempDir::new()?;
        let pdf_path = temp_dir.path().join("input.pdf");
        std::fs::write(&pdf_path, &data)?;

        // Use pdftoppm to convert PDF to images
        let output_prefix = temp_dir.path().join("page");
        let status = Command::new("pdftoppm")
            .args([
                "-png",
                "-r", "200", // 200 DPI for good quality
                pdf_path.to_str().unwrap(),
                output_prefix.to_str().unwrap(),
            ])
            .status();

        match status {
            Ok(s) if s.success() => {}
            Ok(s) => {
                anyhow::bail!("pdftoppm failed with status: {}", s);
            }
            Err(e) => {
                anyhow::bail!(
                    "pdftoppm not found. Install poppler-utils: sudo apt install poppler-utils. Error: {}",
                    e
                );
            }
        }

        // Find all generated page images
        let mut page_files: Vec<_> = std::fs::read_dir(temp_dir.path())?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().map(|e| e == "png").unwrap_or(false))
            .collect();
        page_files.sort();

        if page_files.is_empty() {
            anyhow::bail!("No pages extracted from PDF");
        }

        info!(
            "GeminiPageByPageProvider: processing {} pages from {} (model={})",
            page_files.len(),
            filename,
            self.model
        );

        // Process each page
        let mut pages: Vec<OcrPage> = Vec::new();
        let mut total_prompt_tokens = 0u32;
        let mut total_output_tokens = 0u32;
        let mut all_finish_reasons: Vec<String> = Vec::new();
        let mut full_markdown = String::new();

        for (idx, page_path) in page_files.iter().enumerate() {
            let page_num = (idx + 1) as u32;
            let page_data = std::fs::read(page_path)?;

            info!("  Processing page {}/{}", page_num, page_files.len());

            match self.process_single_page(&page_data, page_num).await {
                Ok((text, prompt_tokens, output_tokens, finish_reason)) => {
                    total_prompt_tokens += prompt_tokens;
                    total_output_tokens += output_tokens;
                    all_finish_reasons.push(finish_reason);

                    // Add to full markdown
                    full_markdown.push_str(&format!("--- Page {} ---\n\n", page_num));
                    full_markdown.push_str(&text);
                    full_markdown.push_str("\n\n");

                    pages.push(OcrPage {
                        page_num,
                        text,
                    });
                }
                Err(e) => {
                    warn!("  Page {} failed: {}", page_num, e);
                    pages.push(OcrPage {
                        page_num,
                        text: format!("[Erro ao processar página: {}]", e),
                    });
                }
            }
        }

        let total_pages = pages.len() as u32;
        let total_tokens = total_prompt_tokens + total_output_tokens;

        // Gemini 2.5 Flash Lite pricing
        let cost_usd = (total_prompt_tokens as f64 * 0.075 / 1_000_000.0)
            + (total_output_tokens as f64 * 0.30 / 1_000_000.0);

        // Check if any page had issues
        let finish_reason = if all_finish_reasons.iter().all(|r| r == "STOP") {
            "STOP".to_string()
        } else {
            format!("MIXED: {:?}", all_finish_reasons)
        };

        info!(
            "GeminiPageByPageProvider: completed {} pages, {} chars (tokens: {} in, {} out | cost: ${:.6})",
            total_pages,
            full_markdown.len(),
            total_prompt_tokens,
            total_output_tokens,
            cost_usd
        );

        Ok(OcrResult {
            markdown: full_markdown,
            pages,
            total_pages,
            metadata: serde_json::json!({
                "model": self.model,
                "provider": "gemini_pages",
                "mode": "page_by_page",
                "finish_reason": finish_reason,
                "token_usage": {
                    "prompt_tokens": total_prompt_tokens,
                    "output_tokens": total_output_tokens,
                    "total_tokens": total_tokens
                },
                "cost_usd": cost_usd
            }),
            ocr_confidence: 0.92,
            provider_name: "gemini_ocr_pages".to_string(),
        })
    }
}

impl GeminiPageByPageProvider {
    async fn process_single_page(
        &self,
        image_data: &[u8],
        page_num: u32,
    ) -> anyhow::Result<(String, u32, u32, String)> {
        let base64_data = BASE64.encode(image_data);

        let request = GenerateContentRequest {
            contents: vec![Content {
                parts: vec![
                    Part::InlineData {
                        inline_data: InlineData {
                            mime_type: "image/png".to_string(),
                            data: base64_data,
                        },
                    },
                    Part::Text {
                        text: format!("Page {}.\n\n{}", page_num, SINGLE_PAGE_PROMPT),
                    },
                ],
            }],
            generation_config: Some(GenerationConfig {
                temperature: 0.1,
                max_output_tokens: 8192,
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
        debug!("Page {} response: {} bytes", page_num, raw_text.len());

        let response: GenerateContentResponse = serde_json::from_str(&raw_text)?;

        let text = response
            .candidates
            .as_ref()
            .and_then(|c| c.first())
            .and_then(|c| c.content.as_ref())
            .and_then(|c| c.parts.first())
            .and_then(|p| p.text.clone())
            .unwrap_or_default();

        let usage = response.usage_metadata.as_ref();
        let prompt_tokens = usage.and_then(|u| u.prompt_token_count).unwrap_or(0);
        let output_tokens = usage.and_then(|u| u.candidates_token_count).unwrap_or(0);
        let finish_reason = response
            .candidates
            .as_ref()
            .and_then(|c| c.first())
            .and_then(|c| c.finish_reason.clone())
            .unwrap_or_else(|| "unknown".to_string());

        Ok((text, prompt_tokens, output_tokens, finish_reason))
    }

    async fn process_single_image(
        &self,
        image_data: &[u8],
        filename: &str,
        page_num: u32,
    ) -> anyhow::Result<OcrResult> {
        let mime_type = if filename.to_lowercase().ends_with(".png") {
            "image/png"
        } else if filename.to_lowercase().ends_with(".jpg")
            || filename.to_lowercase().ends_with(".jpeg")
        {
            "image/jpeg"
        } else {
            "image/png"
        };

        let base64_data = BASE64.encode(image_data);

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
                        text: SINGLE_PAGE_PROMPT.to_string(),
                    },
                ],
            }],
            generation_config: Some(GenerationConfig {
                temperature: 0.1,
                max_output_tokens: 8192,
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

        let response: GenerateContentResponse = serde_json::from_str(&resp.text().await?)?;

        let text = response
            .candidates
            .as_ref()
            .and_then(|c| c.first())
            .and_then(|c| c.content.as_ref())
            .and_then(|c| c.parts.first())
            .and_then(|p| p.text.clone())
            .unwrap_or_default();

        let usage = response.usage_metadata.as_ref();
        let prompt_tokens = usage.and_then(|u| u.prompt_token_count).unwrap_or(0);
        let output_tokens = usage.and_then(|u| u.candidates_token_count).unwrap_or(0);
        let finish_reason = response
            .candidates
            .as_ref()
            .and_then(|c| c.first())
            .and_then(|c| c.finish_reason.clone())
            .unwrap_or_else(|| "unknown".to_string());

        let cost_usd = (prompt_tokens as f64 * 0.075 / 1_000_000.0)
            + (output_tokens as f64 * 0.30 / 1_000_000.0);

        Ok(OcrResult {
            markdown: text.clone(),
            pages: vec![OcrPage {
                page_num,
                text,
            }],
            total_pages: 1,
            metadata: serde_json::json!({
                "model": self.model,
                "provider": "gemini_pages",
                "mode": "single_image",
                "finish_reason": finish_reason,
                "token_usage": {
                    "prompt_tokens": prompt_tokens,
                    "output_tokens": output_tokens,
                    "total_tokens": prompt_tokens + output_tokens
                },
                "cost_usd": cost_usd
            }),
            ocr_confidence: 0.92,
            provider_name: "gemini_ocr_pages".to_string(),
        })
    }
}
