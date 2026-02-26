//! Docling sidecar OCR provider.
//!
//! Supports three modes depending on configuration:
//! - **Always-on**: fixed `DOCLING_URL`, fail immediately on connection error.
//! - **Single-zone wake**: one GCE instance, start on connection error.
//! - **Multi-zone failover**: try each GCE instance in order until one starts.
//!
//! When using GCE wake-on-demand, the active Docling URL is resolved dynamically
//! from the instance's external IP and stored for subsequent requests.

use super::{OcrInput, OcrPage, OcrProvider, OcrResult};
use crate::gce::GceConfig;
use serde::Deserialize;
use std::sync::{Arc, RwLock};
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
    /// Shared active URL — also referenced by AppState for the progress handler.
    /// Initialized with DOCLING_URL env var (or localhost:3001), updated dynamically
    /// when a GCE instance starts.
    active_url: Arc<RwLock<String>>,
    client: reqwest::Client,
    gce_config: Option<GceConfig>,
}

impl DoclingProvider {
    pub fn new(
        client: reqwest::Client,
        gce_config: Option<GceConfig>,
        shared_url: Arc<RwLock<String>>,
    ) -> Self {
        Self {
            active_url: shared_url,
            client,
            gce_config,
        }
    }

    /// Get the current Docling URL to use.
    pub fn current_url(&self) -> String {
        self.active_url.read().unwrap().clone()
    }

    /// Set the active URL (called after a GCE instance starts).
    fn set_active_url(&self, ip: &str) {
        let url = format!("http://{}:3001", ip);
        info!("Docling active URL set to: {}", url);
        *self.active_url.write().unwrap() = url;
    }

    /// Attempt to convert a document via the Docling sidecar.
    async fn try_convert(
        &self,
        input: &OcrInput,
        job_id: Option<&str>,
        url: &str,
    ) -> anyhow::Result<OcrResult> {
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

        let convert_url = if let Some(jid) = job_id {
            format!("{}/convert?job_id={}", url, jid)
        } else {
            format!("{}/convert", url)
        };

        let response = self.client.post(&convert_url).multipart(form).send().await?;

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

    /// Quick health check against a specific URL (5s timeout).
    async fn health_check_url(&self, url: &str) -> bool {
        let health_url = format!("{}/health", url);
        let result = self
            .client
            .get(&health_url)
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await;
        matches!(result, Ok(r) if r.status().is_success())
    }

    /// Ensure the Docling sidecar is reachable, starting a GCE instance if needed.
    /// Returns the URL to use for this request.
    async fn ensure_docling_ready(&self, gce: &GceConfig) -> anyhow::Result<String> {
        let current = self.current_url();

        // Quick check — maybe it's already up
        if self.health_check_url(&current).await {
            return Ok(current);
        }

        info!(
            "Docling sidecar unreachable at {}, trying GCE instances...",
            current
        );

        // Try to start any available instance
        let started = gce.try_start_any(&self.client).await?;

        let url = format!("http://{}:3001", started.external_ip);
        info!(
            "GCE instance '{}/{}' started (IP: {}), waiting for Docling to become healthy...",
            started.zone, started.instance_name, started.external_ip
        );

        // Update active URL for future requests
        self.set_active_url(&started.external_ip);

        // Poll health endpoint for up to 3 minutes (model loading time)
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(180);

        loop {
            if self.health_check_url(&url).await {
                info!("Docling sidecar is healthy at {}", url);
                return Ok(url);
            }

            if tokio::time::Instant::now() >= deadline {
                anyhow::bail!(
                    "Docling sidecar did not become healthy at {} within 3 minutes after instance start",
                    url
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
        self.process_with_job_id(input, "").await
    }

    async fn process_with_job_id(
        &self,
        input: &OcrInput,
        job_id: &str,
    ) -> anyhow::Result<OcrResult> {
        let jid = if job_id.is_empty() {
            None
        } else {
            Some(job_id)
        };
        let url = self.current_url();

        // First attempt
        match self.try_convert(input, jid, &url).await {
            Ok(result) => return Ok(result),
            Err(err) => {
                // If it's a connection error and we have GCE config, try to wake an instance
                if is_connection_error(&err) {
                    if let Some(ref gce) = self.gce_config {
                        warn!(
                            "Docling connection failed at {}, attempting GCE wake-on-demand: {}",
                            url, err
                        );
                        let new_url = self.ensure_docling_ready(gce).await?;
                        // Retry with the new URL
                        return self.try_convert(input, jid, &new_url).await;
                    }
                }
                // No GCE config or not a connection error — fail as before
                return Err(err);
            }
        }
    }
}
