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
use schema::Extraction;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing::{debug, error, info};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

/// Docling sidecar URL (runs on port 3001)
const DOCLING_SIDECAR_URL: &str = "http://localhost:3001";

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
        .with(tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| "generic_extractor=debug,tower_http=debug".into()))
        .with(tracing_subscriber::fmt::layer())
        .init();

    // Load configs from filesystem
    let config_dir = std::path::Path::new("configs");
    let configs = ConfigStore::load_from_dir(config_dir)?;
    info!("Loaded {} configs: {:?}", configs.list().len(), configs.list());

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
        .route("/extractions/:id", get(get_extraction))
        .route("/extractions/:id/node/:node_id", get(get_node))
        .route("/content/:ref_path", get(get_content))
        .layer(DefaultBodyLimit::max(100 * 1024 * 1024)) // 100MB
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive())
        .with_state(state);

    // Run server
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await?;
    info!("Server listening on http://0.0.0.0:3000");
    info!("Docling sidecar expected at {}", DOCLING_SIDECAR_URL);
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
async fn list_configs(
    State(state): State<AppState>,
) -> Json<Vec<String>> {
    Json(state.configs.list().iter().map(|s| s.to_string()).collect())
}

/// Get a specific config.
async fn get_config(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<config::ExtractionConfig>, StatusCode> {
    state.configs
        .get(&name)
        .cloned()
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}

#[derive(serde::Deserialize)]
struct ExtractQuery {
    config: Option<String>,
    upload: Option<bool>,
}

/// Upload a document and extract its structure using Docling + LLM.
async fn extract_document(
    State(state): State<AppState>,
    Query(query): Query<ExtractQuery>,
    mut multipart: Multipart,
) -> Result<Json<Extraction>, (StatusCode, String)> {
    // Get the config
    let config_name = query.config.as_deref().unwrap_or("legal_br");
    let config = state.configs.get(config_name).ok_or_else(|| {
        (StatusCode::BAD_REQUEST, format!("Unknown config: {}. Available: {:?}", config_name, state.configs.list()))
    })?;

    // Read the uploaded file
    let mut filename = String::new();
    let mut file_data = Vec::new();

    while let Some(field) = multipart.next_field().await.map_err(|e| {
        (StatusCode::BAD_REQUEST, format!("Multipart error: {}", e))
    })? {
        if field.name() == Some("file") {
            filename = field.file_name().unwrap_or("document").to_string();
            file_data = field.bytes().await.map_err(|e| {
                (StatusCode::BAD_REQUEST, format!("Failed to read file: {}", e))
            })?.to_vec();
            break;
        }
    }

    if file_data.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "No file uploaded".to_string()));
    }

    info!("Received file: {} ({} bytes) with config: {}", filename, file_data.len(), config_name);

    // Step 1: Call Docling sidecar for OCR + structure
    let docling_response = call_docling_sidecar(&state.http_client, &filename, &file_data)
        .await
        .map_err(|e| {
            error!("Docling conversion failed: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, format!("Docling conversion failed: {}", e))
        })?;

    info!(
        "Docling extracted {} pages, {} chars markdown",
        docling_response.total_pages,
        docling_response.markdown.len()
    );
    
    // Debug: check pages content
    let non_empty_pages = docling_response.pages.iter()
        .filter(|p| !p.text.is_empty())
        .count();
    let total_page_chars: usize = docling_response.pages.iter()
        .map(|p| p.text.len())
        .sum();
    debug!(
        "Pages array: {} items, {} non-empty, {} total chars",
        docling_response.pages.len(),
        non_empty_pages,
        total_page_chars
    );

    // Step 2: Run LLM extraction with docling output
    let extractor = Extractor::new(
        (*state.openrouter).clone(),
        state.content_store.clone(),
    );

    let extraction = extractor
        .extract(&filename, &docling_response, config)
        .await
        .map_err(|e| {
            error!("Extraction failed: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, format!("Extraction failed: {}", e))
        })?;

    // Store extraction in memory
    {
        let mut extractions = state.extractions.write().unwrap();
        extractions.insert(extraction.id.clone(), extraction.clone());
    }

    // Upload to Supabase if requested
    if query.upload.unwrap_or(false) {
        if let Some(ref supabase) = state.supabase {
            supabase.upload_extraction(&extraction, &state.content_store).await.map_err(|e| {
                error!("Supabase upload failed: {}", e);
                (StatusCode::INTERNAL_SERVER_ERROR, format!("Supabase upload failed: {}", e))
            })?;
            info!("Uploaded extraction {} to Supabase", extraction.id);
        } else {
            return Err((StatusCode::BAD_REQUEST, "Supabase not configured. Set SUPABASE_URL and SUPABASE_SERVICE_ROLE_KEY".to_string()));
        }
    }

    info!("Extraction complete: {}", extraction.id);
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
        .post(format!("{}/convert", DOCLING_SIDECAR_URL))
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

/// Get an extraction by ID.
async fn get_extraction(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Extraction>, StatusCode> {
    let extractions = state.extractions.read().unwrap();
    extractions
        .get(&id)
        .cloned()
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}

/// Get a specific node from an extraction.
async fn get_node(
    State(state): State<AppState>,
    Path((id, node_id)): Path<(String, String)>,
) -> Result<Json<schema::DocumentNode>, StatusCode> {
    let extractions = state.extractions.read().unwrap();
    let extraction = extractions.get(&id).ok_or(StatusCode::NOT_FOUND)?;

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

/// Get content by reference with pagination.
async fn get_content(
    State(state): State<AppState>,
    Path(ref_path): Path<String>,
    Query(query): Query<ContentQuery>,
) -> Result<Json<ContentChunk>, StatusCode> {
    let content_ref = format!("content://{}", ref_path);
    let offset = query.offset.unwrap_or(0);
    let limit = query.limit.unwrap_or(4000);

    info!("get_content: ref_path='{}', content_ref='{}', exists={}", 
           ref_path, content_ref, state.content_store.exists(&content_ref));

    state
        .content_store
        .get(&content_ref, offset, limit)
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
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
