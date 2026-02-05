#![allow(dead_code)]
//! OpenRouter API client for LLM interactions.

use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::env;
use tracing::{debug, info};

const OPENROUTER_API_URL: &str = "https://openrouter.ai/api/v1/chat/completions";
const DEFAULT_MODEL: &str = "google/gemini-3-flash-preview";

/// OpenRouter client for chat completions.
#[derive(Clone)]
pub struct OpenRouterClient {
    client: Client,
    api_key: String,
    model: String,
}

impl OpenRouterClient {
    /// Create a new client, reading API key from OPENROUTER_API_KEY env var.
    pub fn from_env() -> Result<Self> {
        let api_key = env::var("OPENROUTER_API_KEY")
            .context("OPENROUTER_API_KEY environment variable not set")?;

        Ok(Self {
            client: Client::new(),
            api_key,
            model: DEFAULT_MODEL.to_string(),
        })
    }

    /// Create a client with a specific model.
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    /// Send a chat completion request with text only.
    pub async fn chat(&self, messages: Vec<Message>) -> Result<String> {
        let request = ChatCompletionRequest {
            model: self.model.clone(),
            messages,
            max_tokens: Some(16384),
            response_format: None,
            // Lock to Google for cache consistency
            provider: Some(ProviderRouting {
                only: Some(vec!["Google".to_string()]),
                allow_fallbacks: Some(false),
            }),
        };

        self.send_request(request).await
    }

    /// Send a chat completion request with JSON schema response format.
    pub async fn chat_json<T: for<'de> Deserialize<'de>>(
        &self,
        messages: Vec<Message>,
        schema_name: &str,
        schema: serde_json::Value,
    ) -> Result<T> {
        let request = ChatCompletionRequest {
            model: self.model.clone(),
            messages,
            max_tokens: Some(16384),
            response_format: Some(ResponseFormat::JsonSchema {
                json_schema: JsonSchemaFormat {
                    name: schema_name.to_string(),
                    schema,
                },
            }),
            // Lock to Google for cache consistency
            provider: Some(ProviderRouting {
                only: Some(vec!["Google".to_string()]),
                allow_fallbacks: Some(false),
            }),
        };

        let response = self.send_request(request).await?;
        let parsed: T =
            serde_json::from_str(&response).context("Failed to parse LLM response as JSON")?;
        Ok(parsed)
    }

    async fn send_request(&self, request: ChatCompletionRequest) -> Result<String> {
        debug!("Sending request to OpenRouter: model={}", request.model);

        let response = self
            .client
            .post(OPENROUTER_API_URL)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await
            .context("Failed to send request to OpenRouter")?;

        let status = response.status();
        if !status.is_success() {
            let error_text = response.text().await.unwrap_or_default();
            anyhow::bail!("OpenRouter API error ({}): {}", status, error_text);
        }

        let response: ChatCompletionResponse = response
            .json()
            .await
            .context("Failed to parse OpenRouter response")?;

        let content = response
            .choices
            .into_iter()
            .next()
            .and_then(|c| c.message.content)
            .unwrap_or_default();

        info!(
            "OpenRouter response: {} tokens (prompt: {}, completion: {})",
            response.usage.total_tokens,
            response.usage.prompt_tokens,
            response.usage.completion_tokens
        );

        Ok(content)
    }
}

// ============================================================================
// Request/Response types
// ============================================================================

#[derive(Debug, Serialize)]
struct ChatCompletionRequest {
    model: String,
    messages: Vec<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<ResponseFormat>,
    /// Provider routing for cache consistency
    #[serde(skip_serializing_if = "Option::is_none")]
    provider: Option<ProviderRouting>,
}

/// Provider routing options for cache consistency.
#[derive(Debug, Serialize)]
struct ProviderRouting {
    /// Only use these providers (for cache hits)
    #[serde(skip_serializing_if = "Option::is_none")]
    only: Option<Vec<String>>,
    /// Don't fallback to other providers
    #[serde(skip_serializing_if = "Option::is_none")]
    allow_fallbacks: Option<bool>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ResponseFormat {
    JsonSchema { json_schema: JsonSchemaFormat },
}

#[derive(Debug, Serialize)]
struct JsonSchemaFormat {
    name: String,
    schema: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<Choice>,
    usage: Usage,
}

#[derive(Debug, Deserialize)]
struct Choice {
    message: ResponseMessage,
}

#[derive(Debug, Deserialize)]
struct ResponseMessage {
    content: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Usage {
    prompt_tokens: u32,
    completion_tokens: u32,
    total_tokens: u32,
}

// ============================================================================
// Message types
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: MessageContent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MessageContent {
    Text(String),
    Parts(Vec<ContentPart>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentPart {
    Text { text: String },
    ImageUrl { image_url: ImageUrl },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageUrl {
    pub url: String,
}

impl Message {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: MessageContent::Text(content.into()),
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: MessageContent::Text(content.into()),
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: MessageContent::Text(content.into()),
        }
    }

    /// Create a user message with text and images (base64 encoded).
    pub fn user_with_images(text: impl Into<String>, images: Vec<Vec<u8>>) -> Self {
        let mut parts = vec![ContentPart::Text { text: text.into() }];

        for image_data in images {
            let base64_data = BASE64.encode(&image_data);
            // Assume PNG for now, could detect from magic bytes
            let data_url = format!("data:image/png;base64,{}", base64_data);
            parts.push(ContentPart::ImageUrl {
                image_url: ImageUrl { url: data_url },
            });
        }

        Self {
            role: Role::User,
            content: MessageContent::Parts(parts),
        }
    }
}
