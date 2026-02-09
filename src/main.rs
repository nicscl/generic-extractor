//! Generic Extractor - Config-driven hierarchical document extraction server.

mod config;
mod content_store;
mod extractor;
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
use extractor::{DoclingResponse, Extractor};
use openrouter::OpenRouterClient;
use schema::{Extraction, ExtractionStatus};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing::{debug, error, info};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

/// Docling sidecar URL (configurable via DOCLING_URL env var)
fn docling_url() -> String {
    std::env::var("DOCLING_URL").unwrap_or_else(|_| "http://localhost:3001".to_string())
}

/// Application state shared across handlers.
#[derive(Clone)]
struct AppState {
    extractions: Arc<RwLock<HashMap<String, Extraction>>>,
    content_store: ContentStore,
    openrouter: Arc<OpenRouterClient>,
    configs: Arc<ConfigStore>,
    http_client: reqwest::Client,
    supabase: Option<supabase::SupabaseClient>,
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

    // Build application state
    let state = AppState {
        extractions: Arc::new(RwLock::new(HashMap::new())),
        content_store: ContentStore::new(),
        openrouter: Arc::new(openrouter),
        configs: Arc::new(configs),
        http_client: reqwest::Client::new(),
        supabase,
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
    info!("Docling sidecar expected at {}", docling_url());
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
}

/// Upload a document and start async extraction using Docling + LLM.
/// Returns immediately with extraction ID and status "processing".
/// Poll GET /extractions/:id to check when status becomes "completed" or "failed".
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

    // Get file data from either multipart upload or URL download
    let (filename, file_data) = if let Some(file_url) = &query.file_url {
        // Download from URL
        info!("Downloading file from URL: {}", file_url);
        let resp = state.http_client.get(file_url).send().await.map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                format!("Failed to download file from URL: {}", e),
            )
        })?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err((
                StatusCode::BAD_REQUEST,
                format!("URL download failed ({}): {}", status, text),
            ));
        }

        // Derive filename from URL path
        let url_filename = file_url
            .rsplit('/')
            .next()
            .and_then(|s| s.split('?').next())
            .filter(|s| !s.is_empty())
            .unwrap_or("document.pdf")
            .to_string();

        let data = resp.bytes().await.map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                format!("Failed to read URL response body: {}", e),
            )
        })?;

        (url_filename, data.to_vec())
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

        (filename, file_data)
    } else {
        return Err((
            StatusCode::BAD_REQUEST,
            "No file provided. Send multipart 'file' field or use ?file_url= parameter."
                .to_string(),
        ));
    };

    info!(
        "Received file: {} ({} bytes) with config: {}",
        filename,
        file_data.len(),
        config_name
    );

    // Create a placeholder extraction with status "processing"
    let extraction = Extraction::new(filename.clone(), Some(config_name.to_string()));
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
    let bg_upload = query.upload.unwrap_or(false);
    let bg_callback_url = query.callback_url.clone();
    let bg_id = extraction_id.clone();

    tokio::spawn(async move {
        // Step 1: Call Docling sidecar for OCR + structure
        let docling_response = match call_docling_sidecar(&bg_state.http_client, &filename, &file_data).await {
            Ok(resp) => resp,
            Err(e) => {
                error!("Docling conversion failed for {}: {}", bg_id, e);
                let mut extractions = bg_state.extractions.write().unwrap();
                if let Some(ext) = extractions.get_mut(&bg_id) {
                    ext.status = ExtractionStatus::Failed;
                    ext.error = Some(format!("Docling conversion failed: {}", e));
                }
                return;
            }
        };

        info!(
            "Docling extracted {} pages, {} chars markdown for {}",
            docling_response.total_pages,
            docling_response.markdown.len(),
            bg_id
        );

        // Step 2: Run LLM extraction with docling output
        let extractor = Extractor::new((*bg_state.openrouter).clone(), bg_state.content_store.clone());

        let mut completed = match extractor.extract(&filename, &docling_response, &bg_config).await {
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
                match supabase.upload_extraction(&completed, &bg_state.content_store).await {
                    Ok(()) => info!("Uploaded extraction {} to Supabase", bg_id),
                    Err(e) => error!("Supabase upload failed for {}: {}", bg_id, e),
                }
            }
        }

        // POST result to callback URL if provided
        if let Some(ref url) = bg_callback_url {
            info!("Sending callback for {} to {}", bg_id, url);
            match bg_state.http_client.post(url).json(&completed).send().await {
                Ok(resp) => info!("Callback for {} returned {}", bg_id, resp.status()),
                Err(e) => error!("Callback for {} failed: {}", bg_id, e),
            }
        }

        info!("Extraction complete: {}", bg_id);
    });

    // Return immediately with the placeholder
    Ok(Json(extraction))
}

/// Call the Docling sidecar to convert a PDF.
async fn call_docling_sidecar(
    client: &reqwest::Client,
    filename: &str,
    file_data: &[u8],
) -> anyhow::Result<DoclingResponse> {
    use reqwest::multipart::{Form, Part};

    let part = Part::bytes(file_data.to_vec())
        .file_name(filename.to_string())
        .mime_str("application/pdf")?;

    let form = Form::new().part("file", part);

    let response = client
        .post(format!("{}/convert", docling_url()))
        .multipart(form)
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let error_text = response.text().await.unwrap_or_default();
        anyhow::bail!("Docling sidecar error ({}): {}", status, error_text);
    }

    let docling: DoclingResponse = response.json().await?;
    Ok(docling)
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

/// List all extractions (lightweight summaries).
/// Merges in-memory extractions with Supabase if configured.
async fn list_extractions(
    State(state): State<AppState>,
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
