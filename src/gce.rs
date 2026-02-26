//! GCE Compute Engine API client for on-demand instance management.
//!
//! Supports multi-zone failover: configure multiple instances across zones
//! and the system will try each in order until one starts successfully.
//!
//! Configuration (env vars):
//! - Multi-zone: `GCE_INSTANCES=zone/name,zone/name,...` (preferred)
//! - Single-zone (legacy): `GCE_ZONE` + `GCE_INSTANCE_NAME`
//! - Both require: `GCE_PROJECT_ID` + `GCE_SA_KEY_PATH`

use anyhow::{Context, Result};
use serde::Deserialize;
use std::sync::Mutex;
use tracing::{debug, info, warn};

const COMPUTE_SCOPE: &str = "https://www.googleapis.com/auth/compute";
const TOKEN_URI: &str = "https://oauth2.googleapis.com/token";

/// A single GCE instance (zone + name).
#[derive(Clone, Debug)]
pub struct GceInstance {
    pub zone: String,
    pub instance_name: String,
}

/// Result of successfully starting an instance.
pub struct StartedInstance {
    pub zone: String,
    pub instance_name: String,
    pub external_ip: String,
}

/// Multi-zone GCE configuration. Holds a list of instances to try in order.
#[derive(Clone)]
pub struct GceConfig {
    pub project_id: String,
    pub instances: Vec<GceInstance>,
    sa_key: ServiceAccountKey,
    /// Cached OAuth2 access token.
    token_cache: std::sync::Arc<Mutex<Option<CachedToken>>>,
}

#[derive(Clone)]
struct CachedToken {
    access_token: String,
    expires_at: u64,
}

#[derive(Clone, Deserialize)]
struct ServiceAccountKey {
    client_email: String,
    private_key: String,
    #[allow(dead_code)]
    token_uri: Option<String>,
}

impl GceConfig {
    /// Try to load from env. Returns `None` if required vars are missing (graceful opt-in).
    ///
    /// Supports two formats:
    /// - Multi-zone: `GCE_INSTANCES=us-east1-d/docling-gpu,us-central1-a/docling-gpu-central`
    /// - Legacy single-zone: `GCE_ZONE=us-east1-d` + `GCE_INSTANCE_NAME=docling-gpu`
    pub fn from_env() -> Option<Self> {
        let project_id = std::env::var("GCE_PROJECT_ID").ok()?;
        let key_path = std::env::var("GCE_SA_KEY_PATH").ok()?;

        let key_json = match std::fs::read_to_string(&key_path) {
            Ok(json) => json,
            Err(e) => {
                warn!("GCE_SA_KEY_PATH={} unreadable: {}", key_path, e);
                return None;
            }
        };

        let sa_key: ServiceAccountKey = match serde_json::from_str(&key_json) {
            Ok(k) => k,
            Err(e) => {
                warn!("Failed to parse GCE service account key: {}", e);
                return None;
            }
        };

        // Try multi-zone format first
        let instances = if let Ok(instances_str) = std::env::var("GCE_INSTANCES") {
            let mut list = Vec::new();
            for entry in instances_str.split(',') {
                let entry = entry.trim();
                if entry.is_empty() {
                    continue;
                }
                let parts: Vec<&str> = entry.splitn(2, '/').collect();
                if parts.len() != 2 {
                    warn!("Invalid GCE_INSTANCES entry '{}', expected zone/name", entry);
                    continue;
                }
                list.push(GceInstance {
                    zone: parts[0].to_string(),
                    instance_name: parts[1].to_string(),
                });
            }
            if list.is_empty() {
                warn!("GCE_INSTANCES is set but contains no valid entries");
                return None;
            }
            list
        } else {
            // Legacy single-zone format
            let zone = std::env::var("GCE_ZONE").ok()?;
            let instance_name = std::env::var("GCE_INSTANCE_NAME").ok()?;
            vec![GceInstance { zone, instance_name }]
        };

        Some(Self {
            project_id,
            instances,
            sa_key,
            token_cache: std::sync::Arc::new(Mutex::new(None)),
        })
    }

    /// Get a valid OAuth2 access token, refreshing if expired.
    pub async fn get_access_token(&self, client: &reqwest::Client) -> Result<String> {
        // Check cache
        {
            let cache = self.token_cache.lock().unwrap();
            if let Some(ref cached) = *cache {
                let now = now_secs();
                if now < cached.expires_at.saturating_sub(60) {
                    return Ok(cached.access_token.clone());
                }
            }
        }

        // Mint a new JWT
        let now = now_secs();
        let claims = serde_json::json!({
            "iss": self.sa_key.client_email,
            "scope": COMPUTE_SCOPE,
            "aud": TOKEN_URI,
            "iat": now,
            "exp": now + 3600,
        });

        let header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
        let encoding_key =
            jsonwebtoken::EncodingKey::from_rsa_pem(self.sa_key.private_key.as_bytes())
                .context("Invalid RSA private key in service account JSON")?;

        let jwt = jsonwebtoken::encode(&header, &claims, &encoding_key)
            .context("Failed to encode JWT")?;

        // Exchange JWT for access token
        #[derive(Deserialize)]
        struct TokenResponse {
            access_token: String,
            expires_in: u64,
        }

        let resp: TokenResponse = client
            .post(TOKEN_URI)
            .form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
                ("assertion", &jwt),
            ])
            .send()
            .await
            .context("Token exchange request failed")?
            .error_for_status()
            .context("Token exchange returned error")?
            .json()
            .await
            .context("Failed to parse token response")?;

        let token = resp.access_token.clone();
        {
            let mut cache = self.token_cache.lock().unwrap();
            *cache = Some(CachedToken {
                access_token: resp.access_token,
                expires_at: now + resp.expires_in,
            });
        }

        Ok(token)
    }

    fn instance_url(&self, instance: &GceInstance) -> String {
        format!(
            "https://compute.googleapis.com/compute/v1/projects/{}/zones/{}/instances/{}",
            self.project_id, instance.zone, instance.instance_name
        )
    }

    /// Get the instance status (RUNNING, TERMINATED, STAGING, STOPPING, etc.)
    pub async fn get_instance_status(
        &self,
        client: &reqwest::Client,
        instance: &GceInstance,
    ) -> Result<String> {
        let token = self.get_access_token(client).await?;

        #[derive(Deserialize)]
        struct InstanceInfo {
            status: String,
        }

        let info: InstanceInfo = client
            .get(&self.instance_url(instance))
            .bearer_auth(&token)
            .send()
            .await
            .context("Failed to query instance status")?
            .error_for_status()
            .context("Instance status query returned error")?
            .json()
            .await
            .context("Failed to parse instance info")?;

        debug!(
            "GCE instance '{}/{}' status: {}",
            instance.zone, instance.instance_name, info.status
        );
        Ok(info.status)
    }

    /// Get the external IP of a running instance from its network interfaces.
    pub async fn get_instance_ip(
        &self,
        client: &reqwest::Client,
        instance: &GceInstance,
    ) -> Result<String> {
        let token = self.get_access_token(client).await?;

        let resp = client
            .get(&self.instance_url(instance))
            .bearer_auth(&token)
            .send()
            .await
            .context("Failed to query instance info")?
            .error_for_status()
            .context("Instance info query returned error")?;

        let body: serde_json::Value = resp.json().await.context("Failed to parse instance info")?;

        // Navigate: networkInterfaces[0].accessConfigs[0].natIP
        let ip = body
            .get("networkInterfaces")
            .and_then(|ni| ni.as_array())
            .and_then(|arr| arr.first())
            .and_then(|iface| iface.get("accessConfigs"))
            .and_then(|ac| ac.as_array())
            .and_then(|arr| arr.first())
            .and_then(|config| config.get("natIP"))
            .and_then(|ip| ip.as_str())
            .map(|s| s.to_string())
            .context("No external IP found on instance (is it running with an access config?)")?;

        Ok(ip)
    }

    /// Start the instance (idempotent — safe to call if already running).
    pub async fn start_instance(
        &self,
        client: &reqwest::Client,
        instance: &GceInstance,
    ) -> Result<()> {
        let token = self.get_access_token(client).await?;
        let url = format!("{}/start", self.instance_url(instance));

        let resp = client
            .post(&url)
            .bearer_auth(&token)
            .send()
            .await
            .context("Failed to send start request")?;

        let status = resp.status();
        if status.is_success() {
            info!(
                "GCE start request accepted for '{}/{}'",
                instance.zone, instance.instance_name
            );
        } else {
            let body = resp.text().await.unwrap_or_default();
            // 409 = already running, which is fine
            if status.as_u16() == 409 {
                info!(
                    "GCE instance '{}/{}' is already running",
                    instance.zone, instance.instance_name
                );
            } else {
                anyhow::bail!(
                    "GCE start failed for '{}/{}' ({}): {}",
                    instance.zone,
                    instance.instance_name,
                    status,
                    body
                );
            }
        }

        Ok(())
    }

    /// Poll until instance reaches RUNNING state. Timeout in seconds.
    pub async fn wait_until_running(
        &self,
        client: &reqwest::Client,
        instance: &GceInstance,
        timeout_secs: u64,
    ) -> Result<()> {
        let deadline = now_secs() + timeout_secs;

        loop {
            let status = self.get_instance_status(client, instance).await?;
            match status.as_str() {
                "RUNNING" => {
                    info!(
                        "GCE instance '{}/{}' is RUNNING",
                        instance.zone, instance.instance_name
                    );
                    return Ok(());
                }
                "STAGING" | "PROVISIONING" => {
                    debug!("Instance is {}... waiting", status);
                }
                "TERMINATED" | "STOPPED" | "SUSPENDED" => {
                    debug!("Instance still {}... waiting for transition", status);
                }
                other => {
                    warn!("Unexpected instance status: {}", other);
                }
            }

            if now_secs() >= deadline {
                anyhow::bail!(
                    "Timed out waiting for instance '{}/{}' to reach RUNNING (last status: {})",
                    instance.zone,
                    instance.instance_name,
                    status
                );
            }

            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
    }

    /// Try to start any instance from the pool. Returns the first one that starts
    /// successfully along with its external IP. Tries instances in order.
    pub async fn try_start_any(
        &self,
        client: &reqwest::Client,
    ) -> Result<StartedInstance> {
        let mut last_error = None;

        for instance in &self.instances {
            info!(
                "Trying to start GCE instance '{}/{}'...",
                instance.zone, instance.instance_name
            );

            // Check if already running
            match self.get_instance_status(client, instance).await {
                Ok(status) if status == "RUNNING" => {
                    info!(
                        "Instance '{}/{}' is already RUNNING, getting IP...",
                        instance.zone, instance.instance_name
                    );
                    match self.get_instance_ip(client, instance).await {
                        Ok(ip) => {
                            return Ok(StartedInstance {
                                zone: instance.zone.clone(),
                                instance_name: instance.instance_name.clone(),
                                external_ip: ip,
                            });
                        }
                        Err(e) => {
                            warn!(
                                "Instance '{}/{}' is RUNNING but failed to get IP: {}",
                                instance.zone, instance.instance_name, e
                            );
                            last_error = Some(e);
                            continue;
                        }
                    }
                }
                Ok(_status) => {
                    // Not running — try to start it
                }
                Err(e) => {
                    warn!(
                        "Failed to check status of '{}/{}': {}",
                        instance.zone, instance.instance_name, e
                    );
                    last_error = Some(e);
                    continue;
                }
            }

            // Try to start
            match self.start_instance(client, instance).await {
                Ok(()) => {}
                Err(e) => {
                    warn!(
                        "Failed to start '{}/{}': {}",
                        instance.zone, instance.instance_name, e
                    );
                    last_error = Some(e);
                    continue;
                }
            }

            // Wait for RUNNING
            match self.wait_until_running(client, instance, 120).await {
                Ok(()) => {}
                Err(e) => {
                    warn!(
                        "Instance '{}/{}' did not reach RUNNING: {}",
                        instance.zone, instance.instance_name, e
                    );
                    last_error = Some(e);
                    continue;
                }
            }

            // Get external IP
            match self.get_instance_ip(client, instance).await {
                Ok(ip) => {
                    return Ok(StartedInstance {
                        zone: instance.zone.clone(),
                        instance_name: instance.instance_name.clone(),
                        external_ip: ip,
                    });
                }
                Err(e) => {
                    warn!(
                        "Instance '{}/{}' started but failed to get IP: {}",
                        instance.zone, instance.instance_name, e
                    );
                    last_error = Some(e);
                    continue;
                }
            }
        }

        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("No GCE instances configured")))
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}
