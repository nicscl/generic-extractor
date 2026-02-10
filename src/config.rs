#![allow(dead_code)]
//! Extraction configuration system.
//!
//! Configs are loaded from `configs/` directory at startup and kept in memory.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tracing::info;

/// Configuration for a specific extraction domain.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractionConfig {
    pub name: String,
    pub description: String,
    pub prompts: Prompts,
    #[serde(default)]
    pub node_types: Vec<NodeTypeConfig>,
    #[serde(default)]
    pub relationship_types: Vec<String>,
    #[serde(default)]
    pub metadata_schema: serde_json::Value,
    /// Regex-based entity patterns for extracting structured identifiers from OCR text.
    #[serde(default)]
    pub entity_patterns: Vec<EntityPattern>,
    /// Hint for extracting a human-readable document identifier (e.g. case number, invoice ID).
    #[serde(default)]
    pub readable_id_hint: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Prompts {
    /// System prompt for document structure extraction
    pub structure: String,
    /// Optional prompt for metadata extraction (if separate pass needed)
    #[serde(default)]
    pub metadata: Option<String>,
    /// Optional prompt for summary generation
    #[serde(default)]
    pub summary: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeTypeConfig {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub subtypes: Vec<String>,
}

/// A regex-based entity pattern for extracting structured identifiers from OCR text.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityPattern {
    /// Unique identifier for this pattern (e.g. "cpf", "pnr", "flight_number")
    pub id: String,
    /// Human-readable label (e.g. "CPF", "PNR / Localizador")
    pub label: String,
    /// Regex pattern string (should contain a capture group for the value)
    pub pattern: String,
    /// Optional normalization: "uppercase" | "strip_punctuation"
    #[serde(default)]
    pub normalize: Option<String>,
    /// Whether to deduplicate matches within a node (default true)
    #[serde(default = "default_true")]
    pub deduplicate: bool,
}

fn default_true() -> bool {
    true
}

/// In-memory store for all loaded configs.
#[derive(Debug, Clone)]
pub struct ConfigStore {
    configs: Arc<HashMap<String, ExtractionConfig>>,
    default_config: String,
}

impl ConfigStore {
    /// Load all configs from the specified directory.
    pub fn load_from_dir(dir: &Path) -> Result<Self> {
        let mut configs = HashMap::new();

        if !dir.exists() {
            anyhow::bail!("Config directory does not exist: {:?}", dir);
        }

        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.extension().map(|e| e == "json").unwrap_or(false) {
                let content = std::fs::read_to_string(&path)
                    .with_context(|| format!("Failed to read config: {:?}", path))?;

                let config: ExtractionConfig = serde_json::from_str(&content)
                    .with_context(|| format!("Failed to parse config: {:?}", path))?;

                info!("Loaded config: {} from {:?}", config.name, path);
                configs.insert(config.name.clone(), config);
            }
        }

        if configs.is_empty() {
            anyhow::bail!("No configs found in {:?}", dir);
        }

        // Use first config as default, or "default" if exists
        let default_config = configs
            .get("default")
            .map(|c| c.name.clone())
            .unwrap_or_else(|| configs.keys().next().unwrap().clone());

        Ok(Self {
            configs: Arc::new(configs),
            default_config,
        })
    }

    /// Get a config by name.
    pub fn get(&self, name: &str) -> Option<&ExtractionConfig> {
        self.configs.get(name)
    }

    /// Get the default config.
    pub fn default(&self) -> &ExtractionConfig {
        self.configs.get(&self.default_config).unwrap()
    }

    /// List all available config names.
    pub fn list(&self) -> Vec<&str> {
        self.configs.keys().map(|s| s.as_str()).collect()
    }
}

/// Create a default generic config for testing.
pub fn create_default_config() -> ExtractionConfig {
    ExtractionConfig {
        name: "default".to_string(),
        description: "Generic document extraction".to_string(),
        prompts: Prompts {
            structure: r#"You are a document structure analyzer. Extract the hierarchical structure of this document.

Return a JSON object with:
{
  "summary": "2-4 sentence overview of the document",
  "metadata": {},
  "children": [
    {
      "id": "unique_id",
      "type": "DOCUMENT|SECTION|GROUP",
      "label": "Human readable label",
      "page_range": [start, end],
      "date": "YYYY-MM-DD if known",
      "author": "Author if known",
      "summary": "2-4 sentence summary",
      "children": []
    }
  ],
  "relationships": [
    {"from": "id1", "to": "id2", "type": "references"}
  ]
}"#.to_string(),
            metadata: None,
            summary: None,
        },
        node_types: vec![
            NodeTypeConfig { id: "DOCUMENT".to_string(), label: "Document".to_string(), subtypes: vec![] },
            NodeTypeConfig { id: "SECTION".to_string(), label: "Section".to_string(), subtypes: vec![] },
            NodeTypeConfig { id: "GROUP".to_string(), label: "Group".to_string(), subtypes: vec![] },
        ],
        relationship_types: vec!["references".to_string(), "contains".to_string()],
        metadata_schema: serde_json::json!({}),
        entity_patterns: Vec::new(),
        readable_id_hint: None,
    }
}
