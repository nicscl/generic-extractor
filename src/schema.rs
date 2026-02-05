//! Hierarchical document extraction schema types.
//!
//! These types match the JSON Schema defined in `plan/initial-schema/extraction_schema.json`.

use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

/// Generate ISO8601 timestamp for current time.
pub fn now_iso8601() -> String {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = duration.as_secs();
    // Simple UTC timestamp without external deps
    // Format: 2025-02-05T12:00:00Z
    let days_since_epoch = secs / 86400;
    let time_of_day = secs % 86400;
    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    let seconds = time_of_day % 60;
    
    // Simplified date calculation (good enough for timestamps)
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_version_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
    pub source_file: String,
    pub extracted_at: String, // ISO8601 timestamp
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extractor_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_pages: Option<u32>,
    pub summary: String,
    pub structure_map: Vec<StructureMapEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relationships: Vec<Relationship>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<ProcessoMetadata>,
    pub children: Vec<DocumentNode>,
}

impl Extraction {
    pub fn new(source_file: String) -> Self {
        Self {
            id: format!("ext_{}", Uuid::new_v4().simple()),
            version: 1,
            previous_version_id: None,
            content_hash: None,
            source_file,
            extracted_at: now_iso8601(),
            extractor_version: Some(env!("CARGO_PKG_VERSION").to_string()),
            total_pages: None,
            summary: String::new(),
            structure_map: Vec::new(),
            relationships: Vec::new(),
            metadata: None,
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
    pub rel_type: RelationshipType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub citation: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationshipType {
    RespondsTo,
    References,
    DecidesOn,
    Appeals,
    Cites,
    Amends,
    Supersedes,
}

/// Metadata for Brazilian judicial processes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessoMetadata {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub numero: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub classe: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub orgao_julgador: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ultima_distribuicao: Option<String>, // YYYY-MM-DD
    #[serde(skip_serializing_if = "Option::is_none")]
    pub valor_causa: Option<f64>,
    #[serde(default = "default_currency")]
    pub valor_causa_moeda: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assuntos: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nivel_sigilo: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub justica_gratuita: Option<bool>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub partes: Vec<Parte>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub terceiros: Vec<Parte>,
}

fn default_currency() -> String {
    "BRL".to_string()
}

/// Party in a judicial process.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Parte {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub polo: Option<Polo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub qualificacao: Option<String>,
    pub nome: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tipo_pessoa: Option<TipoPessoa>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub documentos: Option<Documentos>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endereco: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub representante_legal: Option<RepresentanteLegal>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub advogados: Vec<Advogado>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Polo {
    Ativo,
    Passivo,
    Terceiro,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TipoPessoa {
    #[serde(rename = "FÍSICA")]
    Fisica,
    #[serde(rename = "JURÍDICA")]
    Juridica,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Documentos {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpf: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cnpj: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rg: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepresentanteLegal {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nome: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub relacao: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpf: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Advogado {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nome: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oab: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub telefones: Vec<String>,
}

/// A node in the document tree (document, section, group, etc.).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentNode {
    pub id: String,
    #[serde(rename = "type")]
    pub node_type: DocumentNodeType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subtype: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_range: Option<[u32; 2]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pdf_page_range: Option<[u32; 2]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date: Option<String>, // YYYY-MM-DD
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<DocumentNode>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum DocumentNodeType {
    #[serde(rename = "PETIÇÃO")]
    Peticao,
    #[serde(rename = "DECISÃO")]
    Decisao,
    Recurso,
    #[serde(rename = "CERTIDÃO")]
    Certidao,
    Documento,
    Grupo,
    Section,
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
