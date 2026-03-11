//! Modular OCR provider abstraction.
//!
//! Defines the [`OcrProvider`] trait and unified types so different OCR backends
//! (Docling sidecar, Mistral OCR, etc.) can be swapped via query parameter.

pub mod docling;
pub mod gemini;
pub mod gemini_pages;
pub mod mistral;
pub mod smol_docling;

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
    /// Process with a job_id for progress tracking. Defaults to process() ignoring job_id.
    async fn process_with_job_id(&self, input: &OcrInput, _job_id: &str) -> anyhow::Result<OcrResult> {
        self.process(input).await
    }
}

/// Known provider identifiers used for registry lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OcrProviderKind {
    Docling,
    GeminiOcr,
    GeminiOcrPages,
    MistralOcr,
    SmolDocling,
}

impl OcrProviderKind {
    /// Parse a query-parameter string into a provider kind.
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "docling" => Some(Self::Docling),
            "gemini_ocr" | "gemini" => Some(Self::GeminiOcr),
            "gemini_ocr_pages" | "gemini_pages" => Some(Self::GeminiOcrPages),
            "mistral_ocr" => Some(Self::MistralOcr),
            "smol_docling" => Some(Self::SmolDocling),
            _ => None,
        }
    }
}
