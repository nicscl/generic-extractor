//! Generic Extractor - Config-driven hierarchical document extraction server.

mod config;
mod content_store;
mod entities;
mod extractor;
mod ocr;
mod openrouter;
mod schema;
mod sheet_extractor;
mod sheet_parser;
mod sheet_schema;
mod supabase;

use axum::{
    extract::{DefaultBodyLimit, Multipart, Path, Query, State},
    http::StatusCode,
    response::Json,
    routing::{get, post},
    Router,
};
use config::ConfigStore;
use content_store::{ContentChunk, ContentStore};
use extractor::Extractor;
use ocr::{OcrInput, OcrProvider, OcrProviderKind};
use openrouter::OpenRouterClient;
use schema::{Extraction, ExtractionStatus};
use sheet_schema::SheetExtraction;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing::{debug, error, info};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

/// Application state shared across handlers.
#[derive(Clone)]
struct AppState {
    extractions: Arc<RwLock<HashMap<String, Extraction>>>,
    datasets: Arc<RwLock<HashMap<String, SheetExtraction>>>,
    content_store: ContentStore,
    openrouter: Arc<OpenRouterClient>,
    configs: Arc<ConfigStore>,
    http_client: reqwest::Client,
    supabase: Option<supabase::SupabaseClient>,
    ocr_providers: Arc<HashMap<OcrProviderKind, Arc<dyn OcrProvider>>>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Load .env file if present
    dotenvy::dotenv().ok();

    // Initialize tracing
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "generic_extractor=debug,tower_http=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    // Load configs from filesystem
    let config_dir = std::path::Path::new("configs");
    let configs = ConfigStore::load_from_dir(config_dir)?;
    info!(
        "Loaded {} configs: {:?}",
        configs.list().len(),
        configs.list()
    );

    // Initialize OpenRouter client
    let openrouter = OpenRouterClient::from_env()?;
    info!("OpenRouter client initialized");

    // Initialize Supabase client (optional)
    let supabase = match supabase::SupabaseClient::from_env() {
        Ok(client) => {
            info!("Supabase client initialized");
            Some(client)
        }
        Err(e) => {
            info!("Supabase not configured: {} (upload=true will fail)", e);
            None
        }
    };

    // Initialize OCR providers
    let http_client = reqwest::Client::new();
    let mut ocr_providers: HashMap<OcrProviderKind, Arc<dyn OcrProvider>> = HashMap::new();

    // Docling is always available
    ocr_providers.insert(
        OcrProviderKind::Docling,
        Arc::new(ocr::docling::DoclingProvider::new(http_client.clone())),
    );
    info!("OCR provider registered: docling");

    // Mistral OCR is optional (only if MISTRAL_API_KEY is set)
    match ocr::mistral::MistralOcrProvider::from_env(http_client.clone()) {
        Ok(provider) => {
            ocr_providers.insert(OcrProviderKind::MistralOcr, Arc::new(provider));
            info!("OCR provider registered: mistral_ocr");
        }
        Err(_) => {
            info!("OCR provider skipped: mistral_ocr (MISTRAL_API_KEY not set)");
        }
    }

    // SmolDocling is optional (only if SMOL_DOCLING_URL is set)
    if let Some(provider) = ocr::smol_docling::SmolDoclingProvider::from_env(http_client.clone()) {
        ocr_providers.insert(OcrProviderKind::SmolDocling, Arc::new(provider));
        info!("OCR provider registered: smol_docling");
    } else {
        info!("OCR provider skipped: smol_docling (SMOL_DOCLING_URL not set)");
    }

    // Load persisted datasets from disk
    let datasets = load_datasets_from_disk();
    info!("Loaded {} dataset(s) from data/datasets/", datasets.len());

    // Build application state
    let state = AppState {
        extractions: Arc::new(RwLock::new(HashMap::new())),
        datasets: Arc::new(RwLock::new(datasets)),
        content_store: ContentStore::new(),
        openrouter: Arc::new(openrouter),
        configs: Arc::new(configs),
        http_client,
        supabase,
        ocr_providers: Arc::new(ocr_providers),
    };

    // Build router
    let app = Router::new()
        .route("/health", get(health))
        .route("/configs", get(list_configs))
        .route("/configs/:name", get(get_config))
        .route("/extract", post(extract_document))
        .route("/extractions", get(list_extractions))
        .route("/extractions/:id/snapshot", get(get_extraction_snapshot))
        .route("/extractions/:id", get(get_extraction))
        .route("/extractions/:id/node/:node_id", get(get_node))
        .route("/content/:ref_path", get(get_content))
        .route("/extract-sheet", post(extract_sheet))
        .route("/datasets", get(list_datasets))
        .route("/datasets/:id", get(get_dataset))
        .route("/datasets/:id/rows", get(get_dataset_rows))
        .layer(DefaultBodyLimit::max(100 * 1024 * 1024)) // 100MB
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive())
        .with_state(state);

    // Run server
    let port = std::env::var("PORT").unwrap_or_else(|_| "3002".to_string());
    let addr = format!("0.0.0.0:{}", port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    info!("Server listening on http://{}", addr);
    axum::serve(listener, app).await?;

    Ok(())
}

// ============================================================================
// Handlers
// ============================================================================

/// Health check endpoint.
async fn health() -> &'static str {
    "ok"
}

/// List available configs.
async fn list_configs(State(state): State<AppState>) -> Json<Vec<String>> {
    Json(state.configs.list().iter().map(|s| s.to_string()).collect())
}

/// Get a specific config.
async fn get_config(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<config::ExtractionConfig>, StatusCode> {
    state
        .configs
        .get(&name)
        .cloned()
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}

#[derive(serde::Deserialize)]
struct ExtractQuery {
    config: Option<String>,
    upload: Option<bool>,
    file_url: Option<String>,
    callback_url: Option<String>,
    ocr_provider: Option<String>,
}

/// Upload a document and start async extraction using OCR + LLM.
/// Returns immediately with extraction ID and status "processing".
/// Poll GET /extractions/:id to check when status becomes "completed" or "failed".
///
/// Query params:
///   - `config` — extraction config name (default: `legal_br`)
///   - `upload` — upload result to Supabase (default: false)
///   - `file_url` — download file from this URL instead of multipart upload
///   - `callback_url` — POST completed extraction to this URL
///   - `ocr_provider` — `docling` (default) or `mistral_ocr`
async fn extract_document(
    State(state): State<AppState>,
    Query(query): Query<ExtractQuery>,
    multipart: Option<Multipart>,
) -> Result<Json<Extraction>, (StatusCode, String)> {
    // Get the config
    let config_name = query.config.as_deref().unwrap_or("legal_br");
    let config = state.configs.get(config_name).ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            format!(
                "Unknown config: {}. Available: {:?}",
                config_name,
                state.configs.list()
            ),
        )
    })?;

    // Resolve OCR provider
    let provider_name = query.ocr_provider.as_deref().unwrap_or("docling");
    let provider_kind = OcrProviderKind::from_str(provider_name).ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            format!(
                "Unknown ocr_provider: '{}'. Available: docling, mistral_ocr, smol_docling",
                provider_name
            ),
        )
    })?;
    let provider = state.ocr_providers.get(&provider_kind).ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            format!(
                "OCR provider '{}' is not configured. Check env vars.",
                provider_name
            ),
        )
    })?;
    let provider = Arc::clone(provider);

    // Read file input from multipart or URL
    let (filename_for_log, file_data) =
        read_file_input(multipart, query.file_url.as_deref()).await?;

    // Build OCR input
    let ocr_input = if let Some(file_url) = &query.file_url {
        info!(
            "Received file_url: {} (ocr_provider={})",
            file_url, provider_name
        );
        OcrInput::Url {
            filename: filename_for_log.clone(),
            url: file_url.clone(),
        }
    } else {
        info!(
            "Received file: {} ({} bytes, ocr_provider={})",
            filename_for_log,
            file_data.len(),
            provider_name
        );
        OcrInput::Bytes {
            filename: filename_for_log.clone(),
            data: file_data,
        }
    };

    // Create a placeholder extraction with status "processing"
    let extraction = Extraction::new(filename_for_log.clone(), Some(config_name.to_string()));
    let extraction_id = extraction.id.clone();

    // Store the placeholder in memory
    {
        let mut extractions = state.extractions.write().unwrap();
        extractions.insert(extraction.id.clone(), extraction.clone());
    }

    info!("Queued extraction {} for async processing", extraction_id);

    // Spawn background task to run the pipeline
    let bg_state = state.clone();
    let bg_config = config.clone();
    let bg_upload = query.upload.unwrap_or(true);
    let bg_callback_url = query.callback_url.clone();
    let bg_id = extraction_id.clone();

    tokio::spawn(async move {
        // Step 1: Run OCR via the selected provider
        let ocr_result = match provider.process(&ocr_input).await {
            Ok(result) => result,
            Err(e) => {
                error!("OCR ({}) failed for {}: {}", provider.name(), bg_id, e);
                let mut extractions = bg_state.extractions.write().unwrap();
                if let Some(ext) = extractions.get_mut(&bg_id) {
                    ext.status = ExtractionStatus::Failed;
                    ext.error = Some(format!("OCR ({}) failed: {}", provider.name(), e));
                }
                return;
            }
        };

        info!(
            "{} extracted {} pages, {} chars markdown for {}",
            ocr_result.provider_name,
            ocr_result.total_pages,
            ocr_result.markdown.len(),
            bg_id
        );

        // Step 2: Run LLM extraction with OCR output
        let extractor =
            Extractor::new((*bg_state.openrouter).clone(), bg_state.content_store.clone());

        let mut completed =
            match extractor.extract(&filename_for_log, &ocr_result, &bg_config).await {
                Ok(ext) => ext,
                Err(e) => {
                    error!("LLM extraction failed for {}: {}", bg_id, e);
                    let mut extractions = bg_state.extractions.write().unwrap();
                    if let Some(ext) = extractions.get_mut(&bg_id) {
                        ext.status = ExtractionStatus::Failed;
                        ext.error = Some(format!("Extraction failed: {}", e));
                    }
                    return;
                }
            };

        // Preserve the original ID (extractor.extract creates a new one)
        completed.id = bg_id.clone();
        completed.status = ExtractionStatus::Completed;

        // Store completed extraction in memory
        {
            let mut extractions = bg_state.extractions.write().unwrap();
            extractions.insert(bg_id.clone(), completed.clone());
        }

        // Upload to Supabase if requested
        if bg_upload {
            if let Some(ref supabase) = bg_state.supabase {
                match supabase
                    .upload_extraction(&completed, &bg_state.content_store)
                    .await
                {
                    Ok(()) => info!("Uploaded extraction {} to Supabase", bg_id),
                    Err(e) => error!("Supabase upload failed for {}: {}", bg_id, e),
                }
            }
        }

        // POST result to callback URL if provided
        if let Some(ref url) = bg_callback_url {
            info!("Sending callback for {} to {}", bg_id, url);
            match bg_state
                .http_client
                .post(url)
                .json(&completed)
                .send()
                .await
            {
                Ok(resp) => info!("Callback for {} returned {}", bg_id, resp.status()),
                Err(e) => error!("Callback for {} failed: {}", bg_id, e),
            }
        }

        info!("Extraction complete: {}", bg_id);
    });

    // Return immediately with the placeholder
    Ok(Json(extraction))
}

#[derive(serde::Serialize)]
struct ExtractionSummary {
    id: String,
    status: ExtractionStatus,
    source_file: String,
    config_name: Option<String>,
    extracted_at: String,
    total_pages: Option<u32>,
    summary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    readable_id: Option<String>,
    node_count: usize,
}

/// Try to get an extraction from memory, falling back to Supabase if configured.
/// Caches hydrated extractions in memory for subsequent requests.
async fn get_or_hydrate_extraction(state: &AppState, id: &str) -> Option<Extraction> {
    // 1. Check in-memory cache
    {
        let extractions = state.extractions.read().unwrap();
        if let Some(extraction) = extractions.get(id) {
            return Some(extraction.clone());
        }
    }

    // 2. Fall back to Supabase
    if let Some(ref supabase) = state.supabase {
        match supabase
            .fetch_extraction(id, &state.content_store)
            .await
        {
            Ok(Some(extraction)) => {
                // Cache in memory for future requests
                let mut extractions = state.extractions.write().unwrap();
                extractions.insert(extraction.id.clone(), extraction.clone());
                info!("Hydrated extraction {} from Supabase into cache", id);
                return Some(extraction);
            }
            Ok(None) => {
                debug!("Extraction {} not found in Supabase", id);
            }
            Err(e) => {
                error!("Failed to fetch extraction {} from Supabase: {}", id, e);
            }
        }
    }

    None
}

#[derive(Debug, serde::Deserialize)]
struct ListExtractionsQuery {
    /// Filter by readable_id (substring match, case-insensitive)
    readable_id: Option<String>,
}

/// List all extractions (lightweight summaries).
/// Merges in-memory extractions with Supabase if configured.
async fn list_extractions(
    State(state): State<AppState>,
    Query(query): Query<ListExtractionsQuery>,
) -> Json<Vec<ExtractionSummary>> {
    fn count_nodes(nodes: &[schema::DocumentNode]) -> usize {
        nodes.iter().map(|n| 1 + count_nodes(&n.children)).sum()
    }

    // Collect in-memory extractions
    let mut list: Vec<ExtractionSummary> = {
        let extractions = state.extractions.read().unwrap();
        extractions
            .values()
            .map(|e| ExtractionSummary {
                id: e.id.clone(),
                status: e.status.clone(),
                source_file: e.source_file.clone(),
                config_name: e.config_name.clone(),
                extracted_at: e.extracted_at.clone(),
                total_pages: e.total_pages,
                summary: e.summary.clone(),
                readable_id: e.readable_id.clone(),
                node_count: count_nodes(&e.children),
            })
            .collect()
    };

    // Merge Supabase extractions (dedup by ID)
    if let Some(ref supabase) = state.supabase {
        match supabase.list_extractions().await {
            Ok(rows) => {
                let in_memory_ids: HashSet<String> =
                    list.iter().map(|e| e.id.clone()).collect();
                for row in rows {
                    if !in_memory_ids.contains(&row.id) {
                        list.push(ExtractionSummary {
                            id: row.id,
                            status: ExtractionStatus::Completed, // Supabase entries are always completed
                            source_file: row.source_file,
                            config_name: row.config_name,
                            extracted_at: row.extracted_at,
                            total_pages: row.total_pages,
                            summary: row.summary,
                            readable_id: row.readable_id,
                            node_count: 0, // not hydrated yet
                        });
                    }
                }
            }
            Err(e) => {
                error!("Failed to list extractions from Supabase: {}", e);
            }
        }
    }

    // Filter by readable_id if provided (case-insensitive substring match)
    if let Some(ref filter) = query.readable_id {
        let filter_lower = filter.to_lowercase();
        list.retain(|e| {
            e.readable_id
                .as_ref()
                .map(|rid| rid.to_lowercase().contains(&filter_lower))
                .unwrap_or(false)
        });
    }

    list.sort_by(|a, b| b.extracted_at.cmp(&a.extracted_at));
    Json(list)
}

/// Get an extraction by ID (in-memory + Supabase fallback).
async fn get_extraction(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Extraction>, StatusCode> {
    get_or_hydrate_extraction(&state, &id)
        .await
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}

#[derive(serde::Deserialize)]
struct SnapshotQuery {
    include_content_meta: Option<bool>,
}

#[derive(serde::Serialize)]
struct ExtractionSnapshot {
    #[serde(flatten)]
    extraction: Extraction,
    content_blobs_included: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    content_index: Vec<NodeContentMeta>,
}

#[derive(serde::Serialize)]
struct NodeContentMeta {
    node_id: String,
    content_ref: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    char_count: Option<usize>,
    available: bool,
}

/// Get a full extraction snapshot optimized for MCP/context loading.
///
/// Returns the entire extraction tree in a single call and never includes raw
/// content text. Use `/content/:ref_path` to lazy-load content when needed.
async fn get_extraction_snapshot(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<SnapshotQuery>,
) -> Result<Json<ExtractionSnapshot>, StatusCode> {
    let extraction = get_or_hydrate_extraction(&state, &id)
        .await
        .ok_or(StatusCode::NOT_FOUND)?;

    let include_content_meta = query.include_content_meta.unwrap_or(true);
    let content_index = if include_content_meta {
        let mut index = Vec::new();
        collect_content_meta(&extraction.children, &state.content_store, &mut index);
        index
    } else {
        Vec::new()
    };

    Ok(Json(ExtractionSnapshot {
        extraction,
        content_blobs_included: false,
        content_index,
    }))
}

/// Get a specific node from an extraction (in-memory + Supabase fallback).
async fn get_node(
    State(state): State<AppState>,
    Path((id, node_id)): Path<(String, String)>,
) -> Result<Json<schema::DocumentNode>, StatusCode> {
    let extraction = get_or_hydrate_extraction(&state, &id)
        .await
        .ok_or(StatusCode::NOT_FOUND)?;

    find_node(&extraction.children, &node_id)
        .cloned()
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}

#[derive(serde::Deserialize)]
struct ContentQuery {
    offset: Option<usize>,
    limit: Option<usize>,
}

/// Get content by reference with pagination (in-memory + Supabase fallback).
async fn get_content(
    State(state): State<AppState>,
    Path(ref_path): Path<String>,
    Query(query): Query<ContentQuery>,
) -> Result<Json<ContentChunk>, StatusCode> {
    let content_ref = format!("content://{}", ref_path);
    let offset = query.offset.unwrap_or(0);
    let limit = query.limit.unwrap_or(4000);

    // 1. Try in-memory content store
    if let Some(chunk) = state.content_store.get(&content_ref, offset, limit) {
        return Ok(Json(chunk));
    }

    // 2. Fall back to Supabase
    if let Some(ref supabase) = state.supabase {
        match supabase.fetch_content_by_node_id(&ref_path).await {
            Ok(Some(content)) => {
                info!(
                    "Hydrated content for {} from Supabase ({} chars)",
                    ref_path,
                    content.len()
                );
                // Cache in content store
                state.content_store.store(&ref_path, content);
                // Now serve from store (applies pagination)
                if let Some(chunk) = state.content_store.get(&content_ref, offset, limit) {
                    return Ok(Json(chunk));
                }
            }
            Ok(None) => {
                debug!("Content for {} not found in Supabase", ref_path);
            }
            Err(e) => {
                error!(
                    "Failed to fetch content for {} from Supabase: {}",
                    ref_path, e
                );
            }
        }
    }

    Err(StatusCode::NOT_FOUND)
}

// ============================================================================
// Sheet extraction handlers
// ============================================================================

#[derive(serde::Deserialize)]
struct SheetExtractQuery {
    config: Option<String>,
    upload: Option<bool>,
    ocr_provider: Option<String>,
}

/// Upload a file and start async sheet extraction.
/// Supports CSV, Excel (.xlsx/.xlsm/.xlsb), and PDF (via OCR → table parsing).
/// Returns immediately with dataset ID and status "processing".
/// Poll GET /datasets/:id to check when status becomes "completed" or "failed".
async fn extract_sheet(
    State(state): State<AppState>,
    Query(query): Query<SheetExtractQuery>,
    multipart: Option<Multipart>,
) -> Result<Json<SheetExtraction>, (StatusCode, String)> {
    let config_name = query.config.as_deref().unwrap_or("financial_br");
    let config = state.configs.get(config_name).ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            format!(
                "Unknown config: {}. Available: {:?}",
                config_name,
                state.configs.list()
            ),
        )
    })?;

    let (filename, file_data) = read_file_input(multipart, None).await?;

    let ext = filename
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_lowercase();
    let is_pdf = ext == "pdf";

    // For PDFs, resolve OCR provider
    let ocr_provider = if is_pdf {
        let provider_name = query.ocr_provider.as_deref().unwrap_or("docling");
        let provider_kind = OcrProviderKind::from_str(provider_name).ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                format!(
                    "Unknown ocr_provider: '{}'. Available: docling, mistral_ocr, smol_docling",
                    provider_name
                ),
            )
        })?;
        let provider = state.ocr_providers.get(&provider_kind).ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                format!(
                    "OCR provider '{}' is not configured. Check env vars.",
                    provider_name
                ),
            )
        })?;
        Some(Arc::clone(provider))
    } else {
        None
    };

    info!(
        "Received sheet file: {} ({} bytes, config={}, pdf={})",
        filename,
        file_data.len(),
        config_name,
        is_pdf
    );

    // Create placeholder
    let dataset = SheetExtraction::new(filename.clone(), Some(config_name.to_string()));
    let dataset_id = dataset.id.clone();

    {
        let mut datasets = state.datasets.write().unwrap();
        datasets.insert(dataset.id.clone(), dataset.clone());
    }

    info!("Queued sheet extraction {} for async processing", dataset_id);

    // Spawn background task
    let bg_state = state.clone();
    let bg_config = config.clone();
    let bg_upload = query.upload.unwrap_or(true);
    let bg_id = dataset_id.clone();

    tokio::spawn(async move {
        // Step 1: Get raw sheets — either direct parse or OCR → table extraction
        let sheets = if let Some(provider) = ocr_provider {
            // PDF path: OCR → markdown → extract tables
            let ocr_input = OcrInput::Bytes {
                filename: filename.clone(),
                data: file_data,
            };

            let ocr_result = match provider.process(&ocr_input).await {
                Ok(r) => r,
                Err(e) => {
                    error!("OCR failed for sheet extraction {}: {}", bg_id, e);
                    let mut datasets = bg_state.datasets.write().unwrap();
                    if let Some(ds) = datasets.get_mut(&bg_id) {
                        ds.status = ExtractionStatus::Failed;
                        ds.error = Some(format!("OCR failed: {}", e));
                    }
                    return;
                }
            };

            info!(
                "OCR complete for {}: {} pages, {} chars",
                bg_id, ocr_result.total_pages, ocr_result.markdown.len()
            );

            // Debug: dump OCR markdown to disk for inspection
            let dump_dir = std::path::Path::new("data/debug");
            let _ = std::fs::create_dir_all(dump_dir);
            let dump_path = dump_dir.join(format!("{}_ocr.md", bg_id));
            if let Err(e) = std::fs::write(&dump_path, &ocr_result.markdown) {
                error!("Failed to dump OCR markdown: {}", e);
            } else {
                info!("Dumped OCR markdown to {:?}", dump_path);
            }

            match sheet_parser::parse_ocr_markdown(&ocr_result) {
                Ok(s) => s,
                Err(e) => {
                    error!("No tables found in OCR output for {}: {}", bg_id, e);
                    let mut datasets = bg_state.datasets.write().unwrap();
                    if let Some(ds) = datasets.get_mut(&bg_id) {
                        ds.status = ExtractionStatus::Failed;
                        ds.error = Some(format!("No tables found in PDF: {}", e));
                    }
                    return;
                }
            }
        } else {
            // Direct parse: CSV / Excel
            match sheet_parser::parse_file(&filename, &file_data) {
                Ok(s) => s,
                Err(e) => {
                    error!("Sheet parsing failed for {}: {}", bg_id, e);
                    let mut datasets = bg_state.datasets.write().unwrap();
                    if let Some(ds) = datasets.get_mut(&bg_id) {
                        ds.status = ExtractionStatus::Failed;
                        ds.error = Some(format!("Parsing failed: {}", e));
                    }
                    return;
                }
            }
        };

        info!(
            "Parsed {} sheet(s) for {}: {}",
            sheets.len(),
            bg_id,
            sheets
                .iter()
                .map(|s| format!("\"{}\" ({} rows)", s.name, s.rows.len()))
                .collect::<Vec<_>>()
                .join(", ")
        );

        // Step 2: LLM schema discovery
        let extractor = sheet_extractor::SheetExtractor::new((*bg_state.openrouter).clone());
        let mut completed = match extractor.extract(&filename, &sheets, &bg_config).await {
            Ok(ext) => ext,
            Err(e) => {
                error!("Sheet extraction failed for {}: {}", bg_id, e);
                let mut datasets = bg_state.datasets.write().unwrap();
                if let Some(ds) = datasets.get_mut(&bg_id) {
                    ds.status = ExtractionStatus::Failed;
                    ds.error = Some(format!("Extraction failed: {}", e));
                }
                return;
            }
        };

        // Preserve original ID and mark completed
        completed.id = bg_id.clone();
        completed.status = ExtractionStatus::Completed;

        // Persist to disk
        if let Err(e) = save_dataset_to_disk(&completed) {
            error!("Failed to persist dataset {} to disk: {}", bg_id, e);
        }

        // Upload to Supabase if requested
        if bg_upload {
            if let Some(ref supabase) = bg_state.supabase {
                match supabase.upload_dataset(&completed).await {
                    Ok(()) => info!("Uploaded dataset {} to Supabase", bg_id),
                    Err(e) => error!("Supabase upload failed for dataset {}: {}", bg_id, e),
                }
            }
        }

        {
            let mut datasets = bg_state.datasets.write().unwrap();
            datasets.insert(bg_id.clone(), completed);
        }

        info!("Sheet extraction complete: {}", bg_id);
    });

    Ok(Json(dataset))
}

#[derive(serde::Serialize)]
struct DatasetSummary {
    id: String,
    status: ExtractionStatus,
    source_file: String,
    config_name: Option<String>,
    extracted_at: String,
    summary: String,
    schema_count: usize,
    total_rows: usize,
}

/// Try to get a dataset from memory, falling back to Supabase if configured.
/// Caches hydrated datasets in memory for subsequent requests.
async fn get_or_hydrate_dataset(state: &AppState, id: &str) -> Option<SheetExtraction> {
    // 1. Check in-memory cache
    {
        let datasets = state.datasets.read().unwrap();
        if let Some(dataset) = datasets.get(id) {
            return Some(dataset.clone());
        }
    }

    // 2. Fall back to Supabase
    if let Some(ref supabase) = state.supabase {
        match supabase.fetch_dataset(id).await {
            Ok(Some(dataset)) => {
                let mut datasets = state.datasets.write().unwrap();
                datasets.insert(dataset.id.clone(), dataset.clone());
                info!("Hydrated dataset {} from Supabase into cache", id);
                return Some(dataset);
            }
            Ok(None) => {
                debug!("Dataset {} not found in Supabase", id);
            }
            Err(e) => {
                error!("Failed to fetch dataset {} from Supabase: {}", id, e);
            }
        }
    }

    None
}

/// List all datasets (lightweight summaries).
/// Merges in-memory datasets with Supabase if configured.
async fn list_datasets(State(state): State<AppState>) -> Json<Vec<DatasetSummary>> {
    // Collect in-memory datasets
    let mut list: Vec<DatasetSummary> = {
        let datasets = state.datasets.read().unwrap();
        datasets
            .values()
            .map(|d| DatasetSummary {
                id: d.id.clone(),
                status: d.status.clone(),
                source_file: d.source_file.clone(),
                config_name: d.config_name.clone(),
                extracted_at: d.extracted_at.clone(),
                summary: d.summary.clone(),
                schema_count: d.schemas.len(),
                total_rows: d.schemas.iter().map(|s| s.row_count).sum(),
            })
            .collect()
    };

    // Merge Supabase datasets (dedup by ID)
    if let Some(ref supabase) = state.supabase {
        match supabase.list_datasets().await {
            Ok(rows) => {
                let in_memory_ids: HashSet<String> =
                    list.iter().map(|d| d.id.clone()).collect();
                for row in rows {
                    if !in_memory_ids.contains(&row.id) {
                        // Parse schema count from JSONB
                        let schema_count = row
                            .schemas
                            .as_array()
                            .map(|a| a.len())
                            .unwrap_or(0);
                        let total_rows: usize = row
                            .schemas
                            .as_array()
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|s| s.get("row_count").and_then(|v| v.as_u64()))
                                    .sum::<u64>() as usize
                            })
                            .unwrap_or(0);

                        list.push(DatasetSummary {
                            id: row.id,
                            status: ExtractionStatus::Completed,
                            source_file: row.source_file,
                            config_name: row.config_name,
                            extracted_at: row.extracted_at,
                            summary: row.summary,
                            schema_count,
                            total_rows,
                        });
                    }
                }
            }
            Err(e) => {
                error!("Failed to list datasets from Supabase: {}", e);
            }
        }
    }

    list.sort_by(|a, b| b.extracted_at.cmp(&a.extracted_at));
    Json(list)
}

/// Get a dataset by ID (in-memory + Supabase fallback).
async fn get_dataset(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<SheetExtraction>, StatusCode> {
    get_or_hydrate_dataset(&state, &id)
        .await
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}

#[derive(serde::Deserialize)]
struct DatasetRowsQuery {
    schema_name: Option<String>,
    offset: Option<usize>,
    limit: Option<usize>,
}

/// Query rows from a specific schema within a dataset (paginated).
/// GET /datasets/:id/rows?schema_name=...&offset=0&limit=100
async fn get_dataset_rows(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<DatasetRowsQuery>,
) -> Result<Json<Vec<serde_json::Value>>, (StatusCode, String)> {
    let schema_name = query.schema_name.as_deref().unwrap_or("");
    let offset = query.offset.unwrap_or(0);
    let limit = query.limit.unwrap_or(100);

    if schema_name.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "schema_name query parameter is required".to_string(),
        ));
    }

    // 1. Try in-memory
    {
        let datasets = state.datasets.read().unwrap();
        if let Some(dataset) = datasets.get(&id) {
            if let Some(schema) = dataset.schemas.iter().find(|s| s.name == schema_name) {
                let rows: Vec<serde_json::Value> = schema
                    .rows
                    .iter()
                    .skip(offset)
                    .take(limit)
                    .cloned()
                    .collect();
                return Ok(Json(rows));
            }
            return Err((
                StatusCode::NOT_FOUND,
                format!("Schema '{}' not found in dataset", schema_name),
            ));
        }
    }

    // 2. Fall back to Supabase
    if let Some(ref supabase) = state.supabase {
        match supabase
            .query_dataset_rows(&id, schema_name, offset, limit)
            .await
        {
            Ok(rows) => return Ok(Json(rows)),
            Err(e) => {
                error!(
                    "Failed to query dataset rows from Supabase: {}",
                    e
                );
            }
        }
    }

    Err((
        StatusCode::NOT_FOUND,
        format!("Dataset {} not found", id),
    ))
}

// ============================================================================
// Shared helpers
// ============================================================================

/// Read file data from either a multipart upload or a URL parameter.
/// Returns (filename, file_bytes).
async fn read_file_input(
    multipart: Option<Multipart>,
    file_url: Option<&str>,
) -> Result<(String, Vec<u8>), (StatusCode, String)> {
    if let Some(file_url) = file_url {
        let filename = file_url
            .rsplit('/')
            .next()
            .and_then(|s| s.split('?').next())
            .filter(|s| !s.is_empty())
            .unwrap_or("document")
            .to_string();

        // For URL-based input, we don't download here (OCR providers handle URLs directly)
        // Return empty bytes — the caller will use OcrInput::Url
        Ok((filename, Vec::new()))
    } else if let Some(mut multipart) = multipart {
        let mut filename = String::new();
        let mut file_data = Vec::new();

        while let Some(field) = multipart
            .next_field()
            .await
            .map_err(|e| (StatusCode::BAD_REQUEST, format!("Multipart error: {}", e)))?
        {
            if field.name() == Some("file") {
                filename = field.file_name().unwrap_or("document").to_string();
                file_data = field
                    .bytes()
                    .await
                    .map_err(|e| {
                        (
                            StatusCode::BAD_REQUEST,
                            format!("Failed to read file: {}", e),
                        )
                    })?
                    .to_vec();
                break;
            }
        }

        if file_data.is_empty() {
            return Err((
                StatusCode::BAD_REQUEST,
                "No file uploaded. Send multipart 'file' field or use ?file_url= parameter."
                    .to_string(),
            ));
        }

        Ok((filename, file_data))
    } else {
        Err((
            StatusCode::BAD_REQUEST,
            "No file provided. Send multipart 'file' field or use ?file_url= parameter."
                .to_string(),
        ))
    }
}

// ============================================================================
// Dataset persistence (file-backed)
// ============================================================================

const DATASETS_DIR: &str = "data/datasets";

/// Load all datasets from `data/datasets/*.json` on startup.
fn load_datasets_from_disk() -> HashMap<String, SheetExtraction> {
    let dir = std::path::Path::new(DATASETS_DIR);
    let mut map = HashMap::new();

    if !dir.exists() {
        return map;
    }

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            error!("Failed to read datasets dir: {}", e);
            return map;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().map(|e| e == "json").unwrap_or(false) {
            match std::fs::read_to_string(&path) {
                Ok(content) => match serde_json::from_str::<SheetExtraction>(&content) {
                    Ok(ds) => {
                        info!("Loaded dataset {} from {:?}", ds.id, path);
                        map.insert(ds.id.clone(), ds);
                    }
                    Err(e) => error!("Failed to parse dataset {:?}: {}", path, e),
                },
                Err(e) => error!("Failed to read {:?}: {}", path, e),
            }
        }
    }

    map
}

/// Save a completed dataset to `data/datasets/{id}.json`.
fn save_dataset_to_disk(dataset: &SheetExtraction) -> anyhow::Result<()> {
    let dir = std::path::Path::new(DATASETS_DIR);
    std::fs::create_dir_all(dir)?;

    let path = dir.join(format!("{}.json", dataset.id));
    let json = serde_json::to_string_pretty(dataset)?;
    std::fs::write(&path, json)?;

    info!("Persisted dataset {} to {:?}", dataset.id, path);
    Ok(())
}

// ============================================================================
// Helper functions
// ============================================================================

/// Recursively find a node by ID.
fn find_node<'a>(
    nodes: &'a [schema::DocumentNode],
    node_id: &str,
) -> Option<&'a schema::DocumentNode> {
    for node in nodes {
        if node.id == node_id {
            return Some(node);
        }
        if let Some(found) = find_node(&node.children, node_id) {
            return Some(found);
        }
    }
    None
}

/// Recursively collect content metadata for all nodes.
fn collect_content_meta(
    nodes: &[schema::DocumentNode],
    content_store: &ContentStore,
    out: &mut Vec<NodeContentMeta>,
) {
    for node in nodes {
        if let Some(content_ref) = &node.content_ref {
            let char_count = content_store.len(content_ref);
            out.push(NodeContentMeta {
                node_id: node.id.clone(),
                content_ref: content_ref.clone(),
                char_count,
                available: char_count.is_some(),
            });
        }

        if !node.children.is_empty() {
            collect_content_meta(&node.children, content_store, out);
        }
    }
}
