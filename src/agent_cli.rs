//! Agent-CLI-API LLM backend — wraps `claude -p` via HTTP.
//!
//! Calls `POST /run` on the agent-cli-api service to get LLM completions.

use crate::openrouter::{LlmClient, Message, MessageContent, Role};
use anyhow::{Context, Result};
use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;
use std::env;
use std::time::Duration;
use tracing::{debug, info};

const DEFAULT_URL: &str = "http://localhost:8002";
const DEFAULT_TIMEOUT_SECS: u64 = 300;

pub struct AgentCliClient {
    client: Client,
    base_url: String,
    model: Option<String>,
    timeout_secs: u64,
}

impl AgentCliClient {
    pub fn from_env() -> Result<Self> {
        let base_url = env::var("AGENT_CLI_URL").unwrap_or_else(|_| DEFAULT_URL.into());
        let model = env::var("AGENT_CLI_MODEL").ok();
        let timeout_secs: u64 = env::var("AGENT_CLI_TIMEOUT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_TIMEOUT_SECS);

        // HTTP client timeout = agent timeout + 30s buffer
        let client = Client::builder()
            .timeout(Duration::from_secs(timeout_secs + 30))
            .build()
            .context("Failed to build HTTP client for agent-cli-api")?;

        info!(
            "AgentCliClient: url={}, model={}, timeout={}s",
            base_url,
            model.as_deref().unwrap_or("(default)"),
            timeout_secs
        );

        Ok(Self {
            client,
            base_url,
            model,
            timeout_secs,
        })
    }
}

#[async_trait]
impl LlmClient for AgentCliClient {
    async fn chat(&self, messages: Vec<Message>) -> Result<String> {
        let prompt = flatten_messages(&messages).replace('\0', "");
        debug!(
            "Sending prompt to agent-cli-api ({} chars)",
            prompt.len()
        );

        let mut body = serde_json::Map::new();
        body.insert("prompt".into(), serde_json::Value::String(prompt));
        body.insert(
            "output_format".into(),
            serde_json::Value::String("json".into()),
        );
        body.insert(
            "timeout".into(),
            serde_json::Value::Number(self.timeout_secs.into()),
        );
        if let Some(ref model) = self.model {
            body.insert("model".into(), serde_json::Value::String(model.clone()));
        }

        let url = format!("{}/run", self.base_url);
        let response = self
            .client
            .post(&url)
            .json(&serde_json::Value::Object(body))
            .send()
            .await
            .context("Failed to send request to agent-cli-api")?;

        let status = response.status();
        if !status.is_success() {
            let error_text = response.text().await.unwrap_or_default();
            anyhow::bail!("agent-cli-api error ({}): {}", status, error_text);
        }

        let resp: AgentCliResponse = response
            .json()
            .await
            .context("Failed to parse agent-cli-api response")?;

        if resp.success == Some(false) {
            let err_msg = resp.error.unwrap_or_else(|| "unknown error".into());
            anyhow::bail!("agent-cli-api returned error: {}", err_msg);
        }

        let result = resp
            .data
            .and_then(|d| d.result)
            .unwrap_or_default();

        info!("agent-cli-api response: {} chars", result.len());
        Ok(result)
    }
}

/// Flatten chat messages into a single prompt string.
///
/// System messages → `<system>...</system>` tags, user/assistant → plain text.
/// Image content is silently dropped.
fn flatten_messages(messages: &[Message]) -> String {
    let mut parts = Vec::new();

    for msg in messages {
        let text = extract_text(&msg.content);
        if text.is_empty() {
            continue;
        }

        match msg.role {
            Role::System => parts.push(format!("<system>\n{}\n</system>", text)),
            Role::User => parts.push(text),
            Role::Assistant => parts.push(format!("<assistant>\n{}\n</assistant>", text)),
        }
    }

    parts.join("\n\n")
}

/// Extract text content from a message, dropping images.
fn extract_text(content: &MessageContent) -> String {
    match content {
        MessageContent::Text(s) => s.clone(),
        MessageContent::Parts(parts) => {
            let texts: Vec<&str> = parts
                .iter()
                .filter_map(|p| match p {
                    crate::openrouter::ContentPart::Text { text } => Some(text.as_str()),
                    _ => None, // drop images
                })
                .collect();
            texts.join("\n")
        }
    }
}

#[derive(Debug, Deserialize)]
struct AgentCliResponse {
    success: Option<bool>,
    data: Option<AgentCliData>,
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AgentCliData {
    result: Option<String>,
}
