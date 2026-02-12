//! Sheet extraction schema types for tabular data pipelines.
//!
//! Separate from `schema.rs` since the data model is fundamentally different:
//! flat datasets with typed columns vs hierarchical document trees.

use crate::schema::{now_iso8601, ExtractionStatus};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Root result of a sheet extraction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SheetExtraction {
    pub id: String,
    pub status: ExtractionStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config_name: Option<String>,
    pub source_file: String,
    pub extracted_at: String,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub schemas: Vec<DataSchema>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relationships: Vec<SchemaRelationship>,
}

impl SheetExtraction {
    pub fn new(source_file: String, config_name: Option<String>) -> Self {
        Self {
            id: format!("ds_{}", Uuid::new_v4().simple()),
            status: ExtractionStatus::Processing,
            error: None,
            config_name,
            source_file,
            extracted_at: now_iso8601(),
            summary: String::new(),
            schemas: Vec::new(),
            relationships: Vec::new(),
        }
    }
}

/// A discovered data schema (one logical table).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataSchema {
    pub name: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub columns: Vec<ColumnDef>,
    #[serde(default)]
    pub row_count: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rows: Vec<serde_json::Value>,
}

/// Column definition within a schema.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnDef {
    pub name: String,
    pub data_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transform: Option<String>,
    #[serde(default)]
    pub required: bool,
    /// Source of the column data (e.g. "header", "cell", "annotation").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Relationship between two schemas (e.g. foreign key).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaRelationship {
    /// Format: "schema_name.column_name"
    pub from: String,
    /// Format: "schema_name.column_name"
    pub to: String,
    #[serde(rename = "type")]
    pub rel_type: String,
}
