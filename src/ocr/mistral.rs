//! Mistral OCR provider (uses Mistral's OCR API).

use super::{OcrInput, OcrPage, OcrProvider, OcrResult};
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

pub struct MistralOcrProvider {
    api_key: String,
    client: reqwest::Client,
}

impl MistralOcrProvider {
    pub fn from_env(client: reqwest::Client) -> anyhow::Result<Self> {
        let api_key = std::env::var("MISTRAL_API_KEY")
            .map_err(|_| anyhow::anyhow!("MISTRAL_API_KEY not set"))?;
        Ok(Self { api_key, client })
    }
}

// ── Mistral API request/response types ──────────────────────────────────────

#[derive(Serialize)]
struct OcrRequest {
    model: String,
    document: DocumentSource,
}

#[derive(Serialize)]
#[serde(tag = "type")]
enum DocumentSource {
    #[serde(rename = "document_url")]
    Url { document_url: String },
    #[serde(rename = "file")]
    File { file_id: String },
}

#[derive(Deserialize)]
struct OcrResponse {
    pages: Vec<MistralPage>,
}

#[derive(Deserialize)]
struct MistralPage {
    index: u32,
    markdown: String,
}

#[derive(Deserialize)]
struct FileUploadResponse {
    id: String,
}

// ── Provider implementation ─────────────────────────────────────────────────

#[async_trait::async_trait]
impl OcrProvider for MistralOcrProvider {
    fn name(&self) -> &str {
        "mistral_ocr"
    }

    async fn process(&self, input: &OcrInput) -> anyhow::Result<OcrResult> {
        let document = match input {
            OcrInput::Url { url, .. } => DocumentSource::Url {
                document_url: url.clone(),
            },
            OcrInput::Bytes { filename, data } => {
                let file_id = self.upload_file(filename, data).await?;
                DocumentSource::File { file_id }
            }
        };

        let body = OcrRequest {
            model: "mistral-ocr-latest".to_string(),
            document,
        };

        info!("MistralOcrProvider: calling OCR API");

        let resp = self
            .client
            .post("https://api.mistral.ai/v1/ocr")
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("Mistral OCR API error ({}): {}", status, text);
        }

        let raw_text = resp.text().await?;
        debug!(
            "MistralOcrProvider: raw response ({} bytes): {}",
            raw_text.len(),
            &raw_text[..raw_text.len().min(500)]
        );
        let ocr: OcrResponse = serde_json::from_str(&raw_text)?;

        let total_pages = ocr.pages.len() as u32;

        // Build full markdown by concatenating per-page markdown
        let full_markdown = ocr
            .pages
            .iter()
            .map(|p| p.markdown.as_str())
            .collect::<Vec<_>>()
            .join("\n\n---\n\n");

        let pages: Vec<OcrPage> = ocr
            .pages
            .into_iter()
            .map(|p| OcrPage {
                page_num: p.index + 1, // Normalize 0-indexed → 1-indexed
                text: p.markdown,
            })
            .collect();

        Ok(OcrResult {
            markdown: full_markdown,
            pages,
            total_pages,
            metadata: serde_json::Value::Null,
            ocr_confidence: 0.92,
            provider_name: "mistral_ocr".to_string(),
        })
    }
}

impl MistralOcrProvider {
    /// Upload raw bytes to Mistral Files API, return the file_id.
    async fn upload_file(&self, filename: &str, data: &[u8]) -> anyhow::Result<String> {
        use reqwest::multipart::{Form, Part};

        info!(
            "MistralOcrProvider: uploading {} ({} bytes) to Files API",
            filename,
            data.len()
        );

        let part = Part::bytes(data.to_vec())
            .file_name(filename.to_string())
            .mime_str("application/pdf")?;

        let form = Form::new()
            .part("file", part)
            .text("purpose", "ocr");

        let resp = self
            .client
            .post("https://api.mistral.ai/v1/files")
            .bearer_auth(&self.api_key)
            .multipart(form)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("Mistral Files API error ({}): {}", status, text);
        }

        let upload: FileUploadResponse = resp.json().await?;
        info!("MistralOcrProvider: uploaded file_id={}", upload.id);
        Ok(upload.id)
    }
}
