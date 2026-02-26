//! Docling sidecar OCR provider with instance pool, backpressure, and auto-discovery.
//!
//! Supports multiple Docling sidecar instances running in parallel. Each instance
//! is tracked for health and limited to a configurable number of concurrent OCR jobs
//! (default: 1) to avoid GPU contention.
//!
//! Modes:
//! - **Always-on**: fixed `DOCLING_URL` as seed, optional additional instances discovered via GCE.
//! - **GCE wake-on-demand**: when no healthy instance exists, starts one from the GCE pool.
//! - **Auto-discovery**: background health checker detects manually started/stopped instances.

use super::{OcrInput, OcrPage, OcrProvider, OcrResult};
use crate::gce::{GceConfig, GceInstance};
use crate::paperspace::PaperspaceConfig;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use tracing::{debug, info, warn};

// ---------------------------------------------------------------------------
// Docling sidecar response types (private)
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Instance pool types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstanceState {
    Healthy,
    Unhealthy,
    /// GCE instance just started, waiting for /health to pass.
    Starting,
}

/// A single Docling sidecar instance tracked by the pool.
pub struct InstanceEntry {
    /// Full base URL, e.g. "http://34.56.78.90:3001"
    pub url: String,
    /// GCE identity (if this came from GCE auto-discovery or wake-on-demand).
    pub gce_instance: Option<GceInstance>,
    /// Current health state.
    pub state: RwLock<InstanceState>,
    /// Number of in-flight OCR jobs on this instance (lock-free).
    pub in_flight: AtomicU32,
    /// Semaphore controlling max concurrent jobs. A job must acquire a permit
    /// before dispatching to this instance.
    pub slot_semaphore: tokio::sync::Semaphore,
    /// Epoch seconds of last successful health check.
    pub last_healthy: AtomicU64,
}

/// Pool of Docling sidecar instances with health tracking and job mapping.
pub struct InstancePool {
    /// All known instances.
    pub(crate) instances: RwLock<Vec<Arc<InstanceEntry>>>,
    /// job_id → instance URL mapping (for progress queries).
    job_map: RwLock<HashMap<String, String>>,
    /// Max concurrent OCR jobs per instance.
    max_concurrent: u32,
}

impl InstancePool {
    pub fn new(max_concurrent: u32) -> Self {
        Self {
            instances: RwLock::new(Vec::new()),
            job_map: RwLock::new(HashMap::new()),
            max_concurrent,
        }
    }

    /// Add an instance to the pool. No-op if URL already tracked.
    pub fn add_instance(&self, url: String, gce_instance: Option<GceInstance>, initial_state: InstanceState) {
        let mut instances = self.instances.write().unwrap();
        if instances.iter().any(|e| e.url == url) {
            return;
        }
        info!("Pool: adding instance {} (state={:?})", url, initial_state);
        instances.push(Arc::new(InstanceEntry {
            url,
            gce_instance,
            state: RwLock::new(initial_state),
            in_flight: AtomicU32::new(0),
            slot_semaphore: tokio::sync::Semaphore::new(self.max_concurrent as usize),
            last_healthy: AtomicU64::new(0),
        }));
    }

    /// Remove an instance from the pool by URL.
    pub fn remove_instance(&self, url: &str) {
        let mut instances = self.instances.write().unwrap();
        instances.retain(|e| e.url != url);
        info!("Pool: removed instance {}", url);
    }

    /// Pick the healthy instance with the fewest in-flight jobs.
    pub fn least_loaded(&self) -> Option<Arc<InstanceEntry>> {
        let instances = self.instances.read().unwrap();
        instances
            .iter()
            .filter(|e| *e.state.read().unwrap() == InstanceState::Healthy)
            .min_by_key(|e| e.in_flight.load(Ordering::Relaxed))
            .cloned()
    }

    /// Set instance state by URL.
    pub fn set_instance_state(&self, url: &str, new_state: InstanceState) {
        let instances = self.instances.read().unwrap();
        if let Some(entry) = instances.iter().find(|e| e.url == url) {
            *entry.state.write().unwrap() = new_state;
        }
    }

    /// Register a job → instance URL mapping.
    pub fn register_job(&self, job_id: String, instance_url: String) {
        self.job_map.write().unwrap().insert(job_id, instance_url);
    }

    /// Remove a job mapping.
    pub fn unregister_job(&self, job_id: &str) {
        self.job_map.write().unwrap().remove(job_id);
    }

    /// Look up which instance URL a job is running on.
    pub fn job_instance_url(&self, job_id: &str) -> Option<String> {
        self.job_map.read().unwrap().get(job_id).cloned()
    }

    /// Snapshot of all instances for debug/status endpoints.
    pub fn snapshot(&self) -> Vec<PoolInstanceInfo> {
        let instances = self.instances.read().unwrap();
        instances
            .iter()
            .map(|e| {
                let state = *e.state.read().unwrap();
                PoolInstanceInfo {
                    url: e.url.clone(),
                    state: format!("{:?}", state),
                    in_flight: e.in_flight.load(Ordering::Relaxed),
                }
            })
            .collect()
    }
}

/// Serializable pool instance info for the debug endpoint.
#[derive(serde::Serialize)]
pub struct PoolInstanceInfo {
    pub url: String,
    pub state: String,
    pub in_flight: u32,
}

// ---------------------------------------------------------------------------
// DoclingProvider
// ---------------------------------------------------------------------------

pub struct DoclingProvider {
    pool: Arc<InstancePool>,
    client: reqwest::Client,
    gce_config: Option<GceConfig>,
    paperspace_config: Option<PaperspaceConfig>,
}

/// The default port our Docling sidecar listens on.
const DOCLING_PORT: u16 = 3001;

impl DoclingProvider {
    pub fn new(
        client: reqwest::Client,
        gce_config: Option<GceConfig>,
        paperspace_config: Option<PaperspaceConfig>,
        initial_url: String,
        max_concurrent_per_instance: u32,
    ) -> Self {
        let pool = Arc::new(InstancePool::new(max_concurrent_per_instance));

        // Seed the pool with the initial URL (DOCLING_URL or default).
        pool.add_instance(initial_url, None, InstanceState::Healthy);

        Self {
            pool,
            client,
            gce_config,
            paperspace_config,
        }
    }

    /// Get a shared reference to the pool (for AppState progress handler).
    pub fn pool(&self) -> Arc<InstancePool> {
        Arc::clone(&self.pool)
    }

    /// Spawn the background health checker. Runs every 30s forever.
    pub fn spawn_health_checker(&self) {
        let pool = Arc::clone(&self.pool);
        let client = self.client.clone();
        let gce_config = self.gce_config.clone();
        let paperspace_config = self.paperspace_config.clone();

        tokio::spawn(async move {
            // Wait 10s before first check to let the system settle.
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
            loop {
                interval.tick().await;
                Self::run_health_check_cycle(
                    &pool,
                    &client,
                    gce_config.as_ref(),
                    paperspace_config.as_ref(),
                )
                .await;
            }
        });
    }

    /// One health check cycle: check existing → GC dead → discover GCE → discover Paperspace.
    async fn run_health_check_cycle(
        pool: &InstancePool,
        client: &reqwest::Client,
        gce_config: Option<&GceConfig>,
        paperspace_config: Option<&PaperspaceConfig>,
    ) {
        let entries: Vec<Arc<InstanceEntry>> = pool.instances.read().unwrap().clone();

        // Phase 1: Health-check all known instances.
        for entry in &entries {
            let ok = health_check(client, &entry.url).await;
            let mut state = entry.state.write().unwrap();
            if ok {
                if *state != InstanceState::Healthy {
                    info!("Pool: instance {} is now Healthy", entry.url);
                }
                *state = InstanceState::Healthy;
                entry.last_healthy.store(now_secs(), Ordering::Relaxed);
            } else if *state == InstanceState::Healthy {
                warn!("Pool: instance {} failed health check → Unhealthy", entry.url);
                *state = InstanceState::Unhealthy;
            }
            // Starting instances are left alone — the wake flow handles them.
        }

        // Phase 2: Remove instances unhealthy > 5 min with 0 in-flight.
        let now = now_secs();
        {
            let mut instances = pool.instances.write().unwrap();
            instances.retain(|e| {
                let state = *e.state.read().unwrap();
                if state == InstanceState::Unhealthy
                    && e.in_flight.load(Ordering::Relaxed) == 0
                {
                    let last = e.last_healthy.load(Ordering::Relaxed);
                    if last > 0 && now.saturating_sub(last) > 300 {
                        info!("Pool: removing dead instance {} (unhealthy >5min)", e.url);
                        return false;
                    }
                }
                true
            });
        }

        // Phase 3: Auto-discover RUNNING GCE instances not yet in pool.
        if let Some(gce) = gce_config {
            for gce_inst in &gce.instances {
                let status = match gce.get_instance_status(client, gce_inst).await {
                    Ok(s) => s,
                    Err(_) => continue,
                };
                if status != "RUNNING" {
                    continue;
                }
                let ip = match gce.get_instance_ip(client, gce_inst).await {
                    Ok(ip) => ip,
                    Err(e) => {
                        debug!(
                            "Could not get IP for '{}/{}': {}",
                            gce_inst.zone, gce_inst.instance_name, e
                        );
                        continue;
                    }
                };
                let url = format!("http://{}:3001", ip);
                let already_known = pool
                    .instances
                    .read()
                    .unwrap()
                    .iter()
                    .any(|e| e.url == url);
                if already_known {
                    continue;
                }
                // Health check before adding.
                if health_check(client, &url).await {
                    info!(
                        "Pool: auto-discovered '{}/{}' at {}",
                        gce_inst.zone, gce_inst.instance_name, url
                    );
                    pool.add_instance(url, Some(gce_inst.clone()), InstanceState::Healthy);
                } else {
                    debug!(
                        "Discovered instance at {} not healthy yet, skipping",
                        url
                    );
                }
            }
        }

        // Phase 4: Auto-discover Paperspace machine if running.
        if let Some(ps) = paperspace_config {
            match ps.get_machine_info(client).await {
                Ok(info) if info.state == "ready" => {
                    if let Some(ip) = info.public_ip {
                        let url = format!("http://{}:{}", ip, DOCLING_PORT);
                        let already_known = pool
                            .instances
                            .read()
                            .unwrap()
                            .iter()
                            .any(|e| e.url == url);
                        if !already_known {
                            if health_check(client, &url).await {
                                info!(
                                    "Pool: auto-discovered Paperspace machine '{}' at {}",
                                    info.name, url
                                );
                                pool.add_instance(url, None, InstanceState::Healthy);
                            } else {
                                debug!(
                                    "Paperspace machine at {} not healthy yet, skipping",
                                    url
                                );
                            }
                        }
                    }
                }
                Ok(info) => {
                    debug!("Paperspace machine state: {}", info.state);
                }
                Err(e) => {
                    debug!("Could not check Paperspace machine: {}", e);
                }
            }
        }
    }

    /// Try to start a GCE instance and add it to the pool.
    async fn wake_and_register_instance(&self, gce: &GceConfig) -> anyhow::Result<String> {
        let started = gce.try_start_any(&self.client).await?;
        let url = format!("http://{}:3001", started.external_ip);

        info!(
            "GCE instance '{}/{}' started (IP: {}), waiting for Docling health...",
            started.zone, started.instance_name, started.external_ip
        );

        // Add to pool as Starting.
        let gce_inst = GceInstance {
            zone: started.zone,
            instance_name: started.instance_name,
        };
        self.pool
            .add_instance(url.clone(), Some(gce_inst), InstanceState::Starting);

        // Poll health for up to 3 minutes.
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(180);
        loop {
            if health_check(&self.client, &url).await {
                info!("Docling sidecar healthy at {}", url);
                self.pool
                    .set_instance_state(&url, InstanceState::Healthy);
                return Ok(url);
            }
            if tokio::time::Instant::now() >= deadline {
                self.pool.remove_instance(&url);
                anyhow::bail!(
                    "Docling at {} did not become healthy within 3 minutes",
                    url
                );
            }
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
    }

    /// Try to start a Paperspace machine and add it to the pool.
    async fn wake_paperspace_instance(
        &self,
        ps: &PaperspaceConfig,
    ) -> anyhow::Result<String> {
        info!("Starting Paperspace machine '{}'...", ps.machine_id);

        let ip = ps.start_and_wait(&self.client, 180).await?;
        let url = format!("http://{}:{}", ip, DOCLING_PORT);

        self.pool
            .add_instance(url.clone(), None, InstanceState::Starting);

        // Wait for the Docling service to become healthy (Docker auto-starts on boot).
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(300);
        loop {
            if health_check(&self.client, &url).await {
                info!("Docling sidecar healthy at {} (Paperspace)", url);
                self.pool
                    .set_instance_state(&url, InstanceState::Healthy);
                return Ok(url);
            }
            if tokio::time::Instant::now() >= deadline {
                self.pool.remove_instance(&url);
                anyhow::bail!(
                    "Docling at {} (Paperspace) did not become healthy within 5 minutes",
                    url
                );
            }
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
    }

    /// Try to wake any available cloud instance (Paperspace first, then GCE).
    async fn wake_any_instance(&self) -> anyhow::Result<()> {
        // Try Paperspace first (faster boot, no quota issues).
        if let Some(ref ps) = self.paperspace_config {
            match self.wake_paperspace_instance(ps).await {
                Ok(url) => {
                    info!("Woke Paperspace instance at {}", url);
                    return Ok(());
                }
                Err(e) => {
                    warn!("Paperspace wake failed: {}", e);
                }
            }
        }

        // Fall back to GCE.
        if let Some(ref gce) = self.gce_config {
            match self.wake_and_register_instance(gce).await {
                Ok(url) => {
                    info!("Woke GCE instance at {}", url);
                    return Ok(());
                }
                Err(e) => {
                    warn!("GCE wake failed: {}", e);
                    return Err(e);
                }
            }
        }

        anyhow::bail!("No cloud providers configured (set PAPERSPACE_* or GCE_* env vars)")
    }

    /// Attempt to convert a document via a specific Docling sidecar instance.
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
                info!("DoclingProvider: downloading {} for sidecar", url);
                let resp = self.client.get(url).send().await?;
                if !resp.status().is_success() {
                    let status = resp.status();
                    let text = resp.text().await.unwrap_or_default();
                    anyhow::bail!(
                        "Failed to download file for Docling ({}): {}",
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
}

// ---------------------------------------------------------------------------
// OcrProvider implementation
// ---------------------------------------------------------------------------

/// Returns true if the error looks like a connection failure.
fn is_connection_error(err: &anyhow::Error) -> bool {
    let msg = format!("{:#}", err);
    msg.contains("connection refused")
        || msg.contains("Connection refused")
        || msg.contains("tcp connect error")
        || msg.contains("dns error")
        || msg.contains("timed out")
        || msg.contains("error trying to connect")
}

/// Quick health check against a Docling URL (5s timeout).
async fn health_check(client: &reqwest::Client, url: &str) -> bool {
    let health_url = format!("{}/health", url);
    let result = client
        .get(&health_url)
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await;
    matches!(result, Ok(r) if r.status().is_success())
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
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

        // Step 1: Pick least-loaded healthy instance.
        let instance = match self.pool.least_loaded() {
            Some(inst) => inst,
            None => {
                // No healthy instances — try to wake any cloud provider.
                warn!("No healthy Docling instances, attempting wake-on-demand");
                self.wake_any_instance().await?;
                self.pool.least_loaded().ok_or_else(|| {
                    anyhow::anyhow!("No healthy Docling instances after cloud wake")
                })?
            }
        };

        // Step 2: Acquire a slot (backpressure).
        let _permit = instance
            .slot_semaphore
            .acquire()
            .await
            .map_err(|_| anyhow::anyhow!("Instance semaphore closed"))?;

        // Step 3: Track in-flight and register job mapping.
        instance.in_flight.fetch_add(1, Ordering::Relaxed);
        if let Some(jid_str) = jid {
            self.pool
                .register_job(jid_str.to_string(), instance.url.clone());
        }

        info!(
            "Routing job {} to {} (in_flight: {})",
            jid.unwrap_or("?"),
            instance.url,
            instance.in_flight.load(Ordering::Relaxed)
        );

        // Step 4: Send OCR request.
        let result = self.try_convert(input, jid, &instance.url).await;

        // Step 5: Handle connection errors with fallback.
        let final_result = match result {
            Ok(ocr_result) => Ok(ocr_result),
            Err(err) if is_connection_error(&err) => {
                warn!(
                    "Instance {} failed mid-job: {}. Marking unhealthy.",
                    instance.url, err
                );
                *instance.state.write().unwrap() = InstanceState::Unhealthy;

                // Try fallback: another healthy instance or wake one.
                match self.try_fallback(input, jid).await {
                    Some(fallback_result) => fallback_result,
                    None => Err(err),
                }
            }
            Err(err) => Err(err),
        };

        // Step 6: Cleanup — always runs.
        instance.in_flight.fetch_sub(1, Ordering::Relaxed);
        if let Some(jid_str) = jid {
            self.pool.unregister_job(jid_str);
        }
        // _permit drops here, releasing the semaphore slot.

        final_result
    }
}

impl DoclingProvider {
    /// Try to dispatch a job to a fallback instance (existing or newly woken).
    async fn try_fallback(
        &self,
        input: &OcrInput,
        jid: Option<&str>,
    ) -> Option<anyhow::Result<OcrResult>> {
        // Try existing healthy instance first.
        if let Some(fallback) = self.pool.least_loaded() {
            return Some(self.run_on_instance(input, jid, &fallback).await);
        }

        // No healthy instance — try to wake any cloud provider.
        match self.wake_any_instance().await {
            Ok(()) => {
                if let Some(fallback) = self.pool.least_loaded() {
                    Some(self.run_on_instance(input, jid, &fallback).await)
                } else {
                    None
                }
            }
            Err(e) => {
                warn!("Cloud wake fallback failed: {}", e);
                None
            }
        }
    }

    /// Run a job on a specific instance with proper tracking.
    async fn run_on_instance(
        &self,
        input: &OcrInput,
        jid: Option<&str>,
        instance: &Arc<InstanceEntry>,
    ) -> anyhow::Result<OcrResult> {
        // Acquire slot with timeout.
        let _permit = tokio::time::timeout(
            std::time::Duration::from_secs(300),
            instance.slot_semaphore.acquire(),
        )
        .await
        .map_err(|_| anyhow::anyhow!("Timeout waiting for slot on {}", instance.url))?
        .map_err(|_| anyhow::anyhow!("Semaphore closed on {}", instance.url))?;

        instance.in_flight.fetch_add(1, Ordering::Relaxed);
        if let Some(jid_str) = jid {
            self.pool
                .register_job(jid_str.to_string(), instance.url.clone());
        }

        info!(
            "Fallback: routing job {} to {} (in_flight: {})",
            jid.unwrap_or("?"),
            instance.url,
            instance.in_flight.load(Ordering::Relaxed)
        );

        let result = self.try_convert(input, jid, &instance.url).await;

        instance.in_flight.fetch_sub(1, Ordering::Relaxed);
        if let Some(jid_str) = jid {
            self.pool.unregister_job(jid_str);
        }

        result
    }
}
