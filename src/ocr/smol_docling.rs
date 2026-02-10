//! SmolDocling sidecar OCR provider.

use super::{OcrInput, OcrPage, OcrProvider, OcrResult};
use serde::Deserialize;
use tracing::info;

/// SmolDocling sidecar response (same schema as docling sidecar).
#[derive(Debug, Deserialize)]
struct SmolDoclingResponse {
    markdown: String,
    pages: Vec<SmolDoclingPageContent>,
    total_pages: u32,
    #[serde(default)]
    metadata: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct SmolDoclingPageContent {
    page_num: u32,
    text: String,
}

pub struct SmolDoclingProvider {
    url: String,
    client: reqwest::Client,
}

impl SmolDoclingProvider {
    /// Only create the provider if `SMOL_DOCLING_URL` is explicitly set.
    pub fn from_env(client: reqwest::Client) -> Option<Self> {
        std::env::var("SMOL_DOCLING_URL").ok().map(|url| Self {
            url,
            client,
        })
    }
}

#[async_trait::async_trait]
impl OcrProvider for SmolDoclingProvider {
    fn name(&self) -> &str {
        "smol_docling"
    }

    async fn process(&self, input: &OcrInput) -> anyhow::Result<OcrResult> {
        use reqwest::multipart::{Form, Part};

        let (filename, file_data) = match input {
            OcrInput::Bytes { filename, data } => (filename.clone(), data.clone()),
            OcrInput::Url { filename, url } => {
                info!("SmolDoclingProvider: downloading {} for sidecar", url);
                let resp = self.client.get(url).send().await?;
                if !resp.status().is_success() {
                    let status = resp.status();
                    let text = resp.text().await.unwrap_or_default();
                    anyhow::bail!(
                        "Failed to download file for SmolDocling ({}): {}",
                        status,
                        text
                    );
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
            anyhow::bail!("SmolDocling sidecar error ({}): {}", status, error_text);
        }

        let result: SmolDoclingResponse = response.json().await?;

        Ok(OcrResult {
            markdown: result.markdown,
            pages: result
                .pages
                .into_iter()
                .map(|p| OcrPage {
                    page_num: p.page_num,
                    text: p.text,
                })
                .collect(),
            total_pages: result.total_pages,
            metadata: result.metadata,
            ocr_confidence: 0.85,
            provider_name: "smol_docling".to_string(),
        })
    }
}
