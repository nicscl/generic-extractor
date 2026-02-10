//! Docling sidecar OCR provider.

use super::{OcrInput, OcrPage, OcrProvider, OcrResult};
use serde::Deserialize;
use tracing::info;

/// Docling sidecar response (private deserialization types).
#[derive(Debug, Deserialize)]
struct DoclingResponse {
    markdown: String,
    pages: Vec<DoclingPageContent>,
    total_pages: u32,
    #[serde(default)]
    metadata: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct DoclingPageContent {
    page_num: u32,
    text: String,
}

pub struct DoclingProvider {
    url: String,
    client: reqwest::Client,
}

impl DoclingProvider {
    pub fn new(client: reqwest::Client) -> Self {
        let url =
            std::env::var("DOCLING_URL").unwrap_or_else(|_| "http://localhost:3001".to_string());
        Self { url, client }
    }
}

#[async_trait::async_trait]
impl OcrProvider for DoclingProvider {
    fn name(&self) -> &str {
        "docling"
    }

    async fn process(&self, input: &OcrInput) -> anyhow::Result<OcrResult> {
        use reqwest::multipart::{Form, Part};

        let (filename, file_data) = match input {
            OcrInput::Bytes { filename, data } => (filename.clone(), data.clone()),
            OcrInput::Url { filename, url } => {
                // Docling sidecar only accepts multipart — download first
                info!("DoclingProvider: downloading {} for sidecar", url);
                let resp = self.client.get(url).send().await?;
                if !resp.status().is_success() {
                    let status = resp.status();
                    let text = resp.text().await.unwrap_or_default();
                    anyhow::bail!("Failed to download file for Docling ({}): {}", status, text);
                }
                (filename.clone(), resp.bytes().await?.to_vec())
            }
        };

        let part = Part::bytes(file_data)
            .file_name(filename)
            .mime_str("application/pdf")?;

        let form = Form::new().part("file", part);

        let response = self
            .client
            .post(format!("{}/convert", self.url))
            .multipart(form)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            anyhow::bail!("Docling sidecar error ({}): {}", status, error_text);
        }

        let docling: DoclingResponse = response.json().await?;

        Ok(OcrResult {
            markdown: docling.markdown,
            pages: docling
                .pages
                .into_iter()
                .map(|p| OcrPage {
                    page_num: p.page_num,
                    text: p.text,
                })
                .collect(),
            total_pages: docling.total_pages,
            metadata: docling.metadata,
            ocr_confidence: 0.95,
            provider_name: "docling".to_string(),
        })
    }
}
