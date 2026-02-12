//! GCE Compute Engine API client for on-demand instance management.
//!
//! Enables starting a stopped GCE instance (e.g., Docling GPU sidecar) via
//! the Compute Engine REST API using service account JWT authentication.
//! All env vars are optional — if any are missing, GCE on-demand is disabled.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::sync::Mutex;
use tracing::{debug, info, warn};

const COMPUTE_SCOPE: &str = "https://www.googleapis.com/auth/compute";
const TOKEN_URI: &str = "https://oauth2.googleapis.com/token";

/// Configuration loaded from environment. All four vars must be set.
#[derive(Clone)]
pub struct GceConfig {
    pub project_id: String,
    pub zone: String,
    pub instance_name: String,
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
    /// Try to load from env. Returns `None` if any variable is missing (graceful opt-in).
    pub fn from_env() -> Option<Self> {
        let project_id = std::env::var("GCE_PROJECT_ID").ok()?;
        let zone = std::env::var("GCE_ZONE").ok()?;
        let instance_name = std::env::var("GCE_INSTANCE_NAME").ok()?;
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

        Some(Self {
            project_id,
            zone,
            instance_name,
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

    fn instance_url(&self) -> String {
        format!(
            "https://compute.googleapis.com/compute/v1/projects/{}/zones/{}/instances/{}",
            self.project_id, self.zone, self.instance_name
        )
    }

    /// Get the instance status (RUNNING, TERMINATED, STAGING, STOPPING, etc.)
    pub async fn get_instance_status(&self, client: &reqwest::Client) -> Result<String> {
        let token = self.get_access_token(client).await?;

        #[derive(Deserialize)]
        struct InstanceInfo {
            status: String,
        }

        let info: InstanceInfo = client
            .get(&self.instance_url())
            .bearer_auth(&token)
            .send()
            .await
            .context("Failed to query instance status")?
            .error_for_status()
            .context("Instance status query returned error")?
            .json()
            .await
            .context("Failed to parse instance info")?;

        debug!("GCE instance '{}' status: {}", self.instance_name, info.status);
        Ok(info.status)
    }

    /// Start the instance (idempotent — safe to call if already running).
    pub async fn start_instance(&self, client: &reqwest::Client) -> Result<()> {
        let token = self.get_access_token(client).await?;
        let url = format!("{}/start", self.instance_url());

        let resp = client
            .post(&url)
            .bearer_auth(&token)
            .send()
            .await
            .context("Failed to send start request")?;

        let status = resp.status();
        if status.is_success() {
            info!("GCE start request accepted for '{}'", self.instance_name);
        } else {
            let body = resp.text().await.unwrap_or_default();
            // 409 = already running, which is fine
            if status.as_u16() == 409 {
                info!("GCE instance '{}' is already running", self.instance_name);
            } else {
                anyhow::bail!("GCE start failed ({}): {}", status, body);
            }
        }

        Ok(())
    }

    /// Poll until instance reaches RUNNING state. Timeout in seconds.
    pub async fn wait_until_running(
        &self,
        client: &reqwest::Client,
        timeout_secs: u64,
    ) -> Result<()> {
        let deadline = now_secs() + timeout_secs;

        loop {
            let status = self.get_instance_status(client).await?;
            match status.as_str() {
                "RUNNING" => {
                    info!("GCE instance '{}' is RUNNING", self.instance_name);
                    return Ok(());
                }
                "STAGING" | "PROVISIONING" => {
                    debug!("Instance is {}... waiting", status);
                }
                "TERMINATED" | "STOPPED" | "SUSPENDED" => {
                    // Might need a moment after start_instance() call
                    debug!("Instance still {}... waiting for transition", status);
                }
                other => {
                    warn!("Unexpected instance status: {}", other);
                }
            }

            if now_secs() >= deadline {
                anyhow::bail!(
                    "Timed out waiting for instance '{}' to reach RUNNING (last status: {})",
                    self.instance_name,
                    status
                );
            }

            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}
