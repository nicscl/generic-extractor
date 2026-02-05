//! Generic hierarchical document extraction schema types.
//!
//! This schema is config-independent. Domain-specific metadata is stored as
//! dynamic JSON values, with the structure defined by the extraction config.

use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

/// Generate ISO8601 timestamp for current time.
pub fn now_iso8601() -> String {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = duration.as_secs();
    let days_since_epoch = secs / 86400;
    let time_of_day = secs % 86400;
    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    let seconds = time_of_day % 60;
    
    let mut year = 1970i32;
    let mut remaining_days = days_since_epoch as i32;
    
    loop {
        let days_in_year = if is_leap_year(year) { 366 } else { 365 };
        if remaining_days < days_in_year {
            break;
        }
        remaining_days -= days_in_year;
        year += 1;
    }
    
    let days_in_months: [i32; 12] = if is_leap_year(year) {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };
    
    let mut month = 1;
    for days in days_in_months {
        if remaining_days < days {
            break;
        }
        remaining_days -= days;
        month += 1;
    }
    let day = remaining_days + 1;
    
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year, month, day, hours, minutes, seconds
    )
}

fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

/// Root extraction result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Extraction {
    pub id: String,
    pub version: u32,
    /// Which config was used for this extraction
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_version_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
    pub source_file: String,
    pub extracted_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extractor_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_pages: Option<u32>,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub structure_map: Vec<StructureMapEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relationships: Vec<Relationship>,
    /// Dynamic metadata - structure defined by config
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub metadata: serde_json::Value,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<DocumentNode>,
}

impl Extraction {
    pub fn new(source_file: String, config_name: Option<String>) -> Self {
        Self {
            id: format!("ext_{}", Uuid::new_v4().simple()),
            version: 1,
            config_name,
            previous_version_id: None,
            content_hash: None,
            source_file,
            extracted_at: now_iso8601(),
            extractor_version: Some(env!("CARGO_PKG_VERSION").to_string()),
            total_pages: None,
            summary: String::new(),
            structure_map: Vec::new(),
            relationships: Vec::new(),
            metadata: serde_json::Value::Null,
            children: Vec::new(),
        }
    }
}

/// Flat structure map entry for quick navigation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructureMapEntry {
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<String>,
}

/// Cross-reference between document nodes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Relationship {
    pub from: String,
    pub to: String,
    #[serde(rename = "type")]
    pub rel_type: String,  // Now a string, validated against config
    #[serde(skip_serializing_if = "Option::is_none")]
    pub citation: Option<String>,
}

/// A node in the document tree.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentNode {
    pub id: String,
    #[serde(rename = "type")]
    pub node_type: String,  // Now a string, validated against config
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subtype: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_range: Option<[u32; 2]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub references: Vec<EmbeddedReference>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub referenced_by: Vec<EmbeddedReference>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<ConfidenceScores>,
    /// Node-level dynamic metadata
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub metadata: serde_json::Value,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<DocumentNode>,
}

/// Embedded cross-reference within a node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddedReference {
    pub node: String,
    #[serde(rename = "type")]
    pub ref_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub citation: Option<String>,
}

/// Confidence scores for extraction quality.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfidenceScores {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ocr: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extraction: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub low_confidence_regions: Vec<LowConfidenceRegion>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LowConfidenceRegion {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}
