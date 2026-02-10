//! Modular OCR provider abstraction.
//!
//! Defines the [`OcrProvider`] trait and unified types so different OCR backends
//! (Docling sidecar, Mistral OCR, etc.) can be swapped via query parameter.

pub mod docling;
pub mod mistral;

/// Per-page OCR output (always 1-indexed).
#[derive(Debug, Clone)]
pub struct OcrPage {
    pub page_num: u32,
    pub text: String,
}

/// Unified OCR result returned by every provider.
#[derive(Debug, Clone)]
pub struct OcrResult {
    pub markdown: String,
    pub pages: Vec<OcrPage>,
    pub total_pages: u32,
    pub metadata: serde_json::Value,
    pub ocr_confidence: f64,
    pub provider_name: String,
}

/// Input to an OCR provider — either raw bytes or a remote URL.
pub enum OcrInput {
    Bytes { filename: String, data: Vec<u8> },
    Url { filename: String, url: String },
}

/// Async trait implemented by each OCR backend.
#[async_trait::async_trait]
pub trait OcrProvider: Send + Sync {
    fn name(&self) -> &str;
    async fn process(&self, input: &OcrInput) -> anyhow::Result<OcrResult>;
}

/// Known provider identifiers used for registry lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OcrProviderKind {
    Docling,
    MistralOcr,
}

impl OcrProviderKind {
    /// Parse a query-parameter string into a provider kind.
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "docling" => Some(Self::Docling),
            "mistral_ocr" => Some(Self::MistralOcr),
            _ => None,
        }
    }
}
