//! Paperspace (DigitalOcean) API client for on-demand GPU instance management.
//!
//! Simpler than GCE: machines have persistent disks and static IPs.
//! Docker auto-starts the Docling container on boot, so we just need
//! start/stop and wait for the service health check.
//!
//! Configuration (env vars):
//! - `PAPERSPACE_API_KEY` — API key for authentication
//! - `PAPERSPACE_MACHINE_ID` — Machine ID (from console or creation API)

use anyhow::{Context, Result};
use serde::Deserialize;
use tracing::{debug, info, warn};

const API_BASE: &str = "https://api.paperspace.io";

/// Paperspace machine configuration.
#[derive(Clone)]
pub struct PaperspaceConfig {
    api_key: String,
    pub machine_id: String,
}

/// Machine info returned by the API.
#[derive(Debug, Clone)]
pub struct MachineInfo {
    pub id: String,
    pub name: String,
    pub state: String,
    pub public_ip: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MachineResponse {
    id: String,
    name: String,
    state: String,
    public_ip_address: Option<String>,
    // Note: dynamicPublicIp is a boolean flag in the API, not an IP string.
    // The actual IP is always in publicIpAddress.
}

impl PaperspaceConfig {
    /// Try to load from env. Returns `None` if required vars are missing.
    pub fn from_env() -> Option<Self> {
        let api_key = std::env::var("PAPERSPACE_API_KEY").ok()?;
        let machine_id = std::env::var("PAPERSPACE_MACHINE_ID").ok()?;

        if api_key.is_empty() || machine_id.is_empty() {
            return None;
        }

        Some(Self {
            api_key,
            machine_id,
        })
    }

    /// Get machine info (state, IP, etc.).
    pub async fn get_machine_info(&self, client: &reqwest::Client) -> Result<MachineInfo> {
        let url = format!(
            "{}/machines/getMachinePublic?machineId={}",
            API_BASE, self.machine_id
        );

        let resp: MachineResponse = client
            .get(&url)
            .header("x-api-key", &self.api_key)
            .send()
            .await
            .context("Failed to query Paperspace machine info")?
            .error_for_status()
            .context("Paperspace machine info query returned error")?
            .json()
            .await
            .context("Failed to parse Paperspace machine info")?;

        let public_ip = resp
            .public_ip_address
            .filter(|ip| !ip.is_empty());

        debug!(
            "Paperspace machine '{}' ({}): state={}, ip={:?}",
            resp.name, resp.id, resp.state, public_ip
        );

        Ok(MachineInfo {
            id: resp.id,
            name: resp.name,
            state: resp.state,
            public_ip,
        })
    }

    /// Start the machine (idempotent — safe if already running).
    pub async fn start_machine(&self, client: &reqwest::Client) -> Result<()> {
        let url = format!("{}/machines/{}/start", API_BASE, self.machine_id);

        let resp = client
            .post(&url)
            .header("x-api-key", &self.api_key)
            .send()
            .await
            .context("Failed to send Paperspace start request")?;

        let status = resp.status();
        if status.is_success() {
            info!("Paperspace start request accepted for '{}'", self.machine_id);
        } else {
            let body = resp.text().await.unwrap_or_default();
            // Machine might already be running
            if body.contains("already") || body.contains("ready") {
                info!("Paperspace machine '{}' is already running", self.machine_id);
            } else {
                anyhow::bail!(
                    "Paperspace start failed for '{}' ({}): {}",
                    self.machine_id,
                    status,
                    body
                );
            }
        }

        Ok(())
    }

    /// Stop the machine.
    pub async fn stop_machine(&self, client: &reqwest::Client) -> Result<()> {
        let url = format!("{}/machines/{}/stop", API_BASE, self.machine_id);

        let resp = client
            .post(&url)
            .header("x-api-key", &self.api_key)
            .send()
            .await
            .context("Failed to send Paperspace stop request")?;

        let status = resp.status();
        if status.is_success() {
            info!("Paperspace stop request accepted for '{}'", self.machine_id);
        } else {
            let body = resp.text().await.unwrap_or_default();
            if body.contains("already") || body.contains("off") {
                info!("Paperspace machine '{}' is already off", self.machine_id);
            } else {
                anyhow::bail!(
                    "Paperspace stop failed for '{}' ({}): {}",
                    self.machine_id,
                    status,
                    body
                );
            }
        }

        Ok(())
    }

    /// Poll until machine reaches "ready" state. Returns the public IP.
    pub async fn wait_until_ready(
        &self,
        client: &reqwest::Client,
        timeout_secs: u64,
    ) -> Result<String> {
        let deadline = now_secs() + timeout_secs;

        loop {
            let info = self.get_machine_info(client).await?;
            match info.state.as_str() {
                "ready" => {
                    let ip = info.public_ip.context(
                        "Paperspace machine is ready but has no public IP",
                    )?;
                    info!(
                        "Paperspace machine '{}' is ready (IP: {})",
                        self.machine_id, ip
                    );
                    return Ok(ip);
                }
                "starting" | "provisioning" | "restarting" | "upgrading" => {
                    debug!("Paperspace machine state: {}... waiting", info.state);
                }
                "off" | "stopping" => {
                    debug!(
                        "Paperspace machine state: {}... waiting for transition",
                        info.state
                    );
                }
                other => {
                    warn!("Unexpected Paperspace machine state: {}", other);
                }
            }

            if now_secs() >= deadline {
                anyhow::bail!(
                    "Timed out waiting for Paperspace machine '{}' to become ready (last state: {})",
                    self.machine_id,
                    info.state
                );
            }

            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
    }

    /// Convenience: start machine and wait for it to become ready.
    /// Returns the public IP address.
    pub async fn start_and_wait(
        &self,
        client: &reqwest::Client,
        timeout_secs: u64,
    ) -> Result<String> {
        // Check if already running first
        let info = self.get_machine_info(client).await?;
        if info.state == "ready" {
            if let Some(ip) = info.public_ip {
                info!(
                    "Paperspace machine '{}' already ready (IP: {})",
                    self.machine_id, ip
                );
                return Ok(ip);
            }
        }

        self.start_machine(client).await?;
        self.wait_until_ready(client, timeout_secs).await
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}
