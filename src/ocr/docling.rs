//! Docling sidecar OCR provider.
//!
//! Supports two modes depending on whether GCE config is present:
//! - **Always-on**: fail immediately on connection error (current behavior).
//! - **Wake-on-demand**: on connection error, start the GCE instance, wait
//!   for Docling to become healthy, then retry the request.

use super::{OcrInput, OcrPage, OcrProvider, OcrResult};
use crate::gce::GceConfig;
use serde::Deserialize;
use tracing::{info, warn};

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
    gce_config: Option<GceConfig>,
}

impl DoclingProvider {
    pub fn new(client: reqwest::Client, gce_config: Option<GceConfig>) -> Self {
        let url =
            std::env::var("DOCLING_URL").unwrap_or_else(|_| "http://localhost:3001".to_string());
        Self {
            url,
            client,
            gce_config,
        }
    }

    /// Attempt to convert a document via the Docling sidecar.
    async fn try_convert(&self, input: &OcrInput) -> anyhow::Result<OcrResult> {
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

    /// Quick health check against the sidecar (5s timeout).
    async fn health_check(&self) -> bool {
        let url = format!("{}/health", self.url);
        let result = self
            .client
            .get(&url)
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await;
        matches!(result, Ok(r) if r.status().is_success())
    }

    /// Ensure the Docling sidecar is reachable, starting the GCE instance if needed.
    /// Only called when `gce_config` is `Some`.
    async fn ensure_docling_ready(&self, gce: &GceConfig) -> anyhow::Result<()> {
        // Quick check — maybe it's already up
        if self.health_check().await {
            return Ok(());
        }

        info!("Docling sidecar unreachable, checking GCE instance status...");

        let status = gce.get_instance_status(&self.client).await?;
        if status != "RUNNING" {
            info!("GCE instance is '{}', starting...", status);
            gce.start_instance(&self.client).await?;
            gce.wait_until_running(&self.client, 120).await?;
        }

        // Instance is RUNNING, but Docling may still be loading models.
        // Poll health endpoint for up to 3 minutes.
        info!("Waiting for Docling sidecar to become healthy...");
        let deadline =
            tokio::time::Instant::now() + std::time::Duration::from_secs(180);

        loop {
            if self.health_check().await {
                info!("Docling sidecar is healthy");
                return Ok(());
            }

            if tokio::time::Instant::now() >= deadline {
                anyhow::bail!(
                    "Docling sidecar did not become healthy within 3 minutes after instance start"
                );
            }

            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
    }
}

/// Returns true if the error looks like a connection failure (refused, timeout, DNS).
fn is_connection_error(err: &anyhow::Error) -> bool {
    let msg = format!("{:#}", err);
    msg.contains("connection refused")
        || msg.contains("Connection refused")
        || msg.contains("tcp connect error")
        || msg.contains("dns error")
        || msg.contains("timed out")
        || msg.contains("error trying to connect")
}

#[async_trait::async_trait]
impl OcrProvider for DoclingProvider {
    fn name(&self) -> &str {
        "docling"
    }

    async fn process(&self, input: &OcrInput) -> anyhow::Result<OcrResult> {
        // First attempt
        match self.try_convert(input).await {
            Ok(result) => return Ok(result),
            Err(err) => {
                // If it's a connection error and we have GCE config, try to wake the instance
                if is_connection_error(&err) {
                    if let Some(ref gce) = self.gce_config {
                        warn!("Docling connection failed, attempting GCE wake-on-demand: {}", err);
                        self.ensure_docling_ready(gce).await?;
                        // Retry after waking
                        return self.try_convert(input).await;
                    }
                }
                // No GCE config or not a connection error — fail as before
                return Err(err);
            }
        }
    }
}
