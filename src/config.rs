#![allow(dead_code)]
//! Extraction configuration system.
//!
//! Configs are loaded from Supabase (primary) or `configs/` directory (fallback).
//! In-memory cache is backed by `RwLock` for runtime CRUD.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, RwLock};
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
    /// Sheet extraction config (for tabular data pipelines).
    #[serde(default)]
    pub sheet_config: Option<SheetConfig>,
}

/// Configuration for sheet/tabular data extraction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SheetConfig {
    /// Expected columns the agent should look for.
    #[serde(default)]
    pub expected_columns: Vec<ExpectedColumn>,
    /// Business-specific hints injected into the LLM prompt.
    #[serde(default)]
    pub classification_hints: Option<String>,
}

/// A column the agent should expect to find in the data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExpectedColumn {
    pub name: String,
    #[serde(default)]
    pub data_type: Option<String>,
    #[serde(default)]
    pub format: Option<String>,
    #[serde(default)]
    pub required: bool,
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

/// In-memory store for all loaded configs, backed by `RwLock` for runtime mutations.
#[derive(Debug)]
pub struct ConfigStore {
    configs: Arc<RwLock<HashMap<String, ExtractionConfig>>>,
    default_config: RwLock<String>,
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

        let default_config = Self::pick_default(&configs);

        Ok(Self {
            configs: Arc::new(RwLock::new(configs)),
            default_config: RwLock::new(default_config),
        })
    }

    /// Create a ConfigStore from a list of configs (e.g. loaded from Supabase).
    pub fn from_configs(configs: Vec<ExtractionConfig>) -> Result<Self> {
        if configs.is_empty() {
            anyhow::bail!("No configs provided");
        }

        let map: HashMap<String, ExtractionConfig> = configs
            .into_iter()
            .map(|c| (c.name.clone(), c))
            .collect();

        let default_config = Self::pick_default(&map);

        Ok(Self {
            configs: Arc::new(RwLock::new(map)),
            default_config: RwLock::new(default_config),
        })
    }

    /// Get a config by name (returns clone).
    pub fn get(&self, name: &str) -> Option<ExtractionConfig> {
        self.configs.read().unwrap().get(name).cloned()
    }

    /// Get the default config (returns clone).
    pub fn default_config(&self) -> ExtractionConfig {
        let default_name = self.default_config.read().unwrap().clone();
        self.configs
            .read()
            .unwrap()
            .get(&default_name)
            .cloned()
            .expect("default config must exist")
    }

    /// List all available config names.
    pub fn list(&self) -> Vec<String> {
        self.configs.read().unwrap().keys().cloned().collect()
    }

    /// Insert or update a config in the in-memory cache.
    pub fn insert(&self, config: ExtractionConfig) {
        self.configs
            .write()
            .unwrap()
            .insert(config.name.clone(), config);
    }

    /// Remove a config from the in-memory cache. Returns true if it existed.
    pub fn remove(&self, name: &str) -> bool {
        self.configs.write().unwrap().remove(name).is_some()
    }

    /// Get all configs as a Vec (for seeding).
    pub fn all(&self) -> Vec<ExtractionConfig> {
        self.configs.read().unwrap().values().cloned().collect()
    }

    fn pick_default(configs: &HashMap<String, ExtractionConfig>) -> String {
        configs
            .get("default")
            .map(|c| c.name.clone())
            .unwrap_or_else(|| configs.keys().next().unwrap().clone())
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
        sheet_config: None,
    }
}
