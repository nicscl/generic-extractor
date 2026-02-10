//! Generic Extractor - Config-driven hierarchical document extraction server.

mod config;
mod content_store;
mod entities;
mod extractor;
mod ocr;
mod openrouter;
mod schema;
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

    // Build application state
    let state = AppState {
        extractions: Arc::new(RwLock::new(HashMap::new())),
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

    // Build OCR input from either multipart upload or URL
    let ocr_input = if let Some(file_url) = &query.file_url {
        // Derive filename from URL path
        let url_filename = file_url
            .rsplit('/')
            .next()
            .and_then(|s| s.split('?').next())
            .filter(|s| !s.is_empty())
            .unwrap_or("document.pdf")
            .to_string();

        info!(
            "Received file_url: {} (ocr_provider={})",
            file_url, provider_name
        );

        OcrInput::Url {
            filename: url_filename,
            url: file_url.clone(),
        }
    } else if let Some(mut multipart) = multipart {
        // Multipart file upload
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

        info!(
            "Received file: {} ({} bytes, ocr_provider={})",
            filename,
            file_data.len(),
            provider_name
        );

        OcrInput::Bytes {
            filename,
            data: file_data,
        }
    } else {
        return Err((
            StatusCode::BAD_REQUEST,
            "No file provided. Send multipart 'file' field or use ?file_url= parameter."
                .to_string(),
        ));
    };

    let filename_for_log = match &ocr_input {
        OcrInput::Bytes { filename, .. } | OcrInput::Url { filename, .. } => filename.clone(),
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
