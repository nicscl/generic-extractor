//! Generic Extractor - Hierarchical document extraction server.

mod content_store;
mod extractor;
mod openrouter;
mod schema;

use axum::{
    extract::{DefaultBodyLimit, Multipart, Path, Query, State},
    http::StatusCode,
    response::Json,
    routing::{get, post},
    Router,
};
use content_store::{ContentChunk, ContentStore};
use extractor::Extractor;
use openrouter::OpenRouterClient;
use schema::Extraction;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing::{error, info};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

/// Application state shared across handlers.
#[derive(Clone)]
struct AppState {
    extractions: Arc<RwLock<HashMap<String, Extraction>>>,
    content_store: ContentStore,
    openrouter: Arc<OpenRouterClient>,
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

    // Initialize OpenRouter client
    let openrouter = OpenRouterClient::from_env()?;
    info!("OpenRouter client initialized");

    // Build application state
    let state = AppState {
        extractions: Arc::new(RwLock::new(HashMap::new())),
        content_store: ContentStore::new(),
        openrouter: Arc::new(openrouter),
    };

    // Build router
    let app = Router::new()
        .route("/health", get(health))
        .route("/extract", post(extract_document))
        .route("/extractions/{id}", get(get_extraction))
        .route("/extractions/{id}/node/{node_id}", get(get_node))
        .route("/content/{ref}", get(get_content))
        .layer(DefaultBodyLimit::max(100 * 1024 * 1024)) // 100MB
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive())
        .with_state(state);

    // Run server
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await?;
    info!("Server listening on http://0.0.0.0:3000");
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

/// Upload a document and extract its structure.
async fn extract_document(
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> Result<Json<Extraction>, (StatusCode, String)> {
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

    info!("Received file: {} ({} bytes)", filename, file_data.len());

    // Extract text from PDF (basic extraction)
    let text_content = if filename.to_lowercase().ends_with(".pdf") {
        extract_pdf_text(&file_data).unwrap_or_else(|e| {
            error!("PDF extraction failed: {}", e);
            String::new()
        })
    } else {
        // Assume plain text
        String::from_utf8_lossy(&file_data).to_string()
    };

    if text_content.is_empty() {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            "Could not extract text from document".to_string(),
        ));
    }

    // Run extraction
    let extractor = Extractor::new(
        (*state.openrouter).clone(),
        state.content_store.clone(),
    );

    let extraction = extractor
        .extract(&filename, &text_content, None)
        .await
        .map_err(|e| {
            error!("Extraction failed: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, format!("Extraction failed: {}", e))
        })?;

    // Store extraction
    {
        let mut extractions = state.extractions.write().unwrap();
        extractions.insert(extraction.id.clone(), extraction.clone());
    }

    info!("Extraction complete: {}", extraction.id);
    Ok(Json(extraction))
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

    state
        .content_store
        .get(&content_ref, offset, limit)
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}

// ============================================================================
// Helper functions
// ============================================================================

/// Extract text from a PDF file using lopdf.
fn extract_pdf_text(data: &[u8]) -> anyhow::Result<String> {
    use lopdf::Document;
    use std::io::Cursor;
    
    let doc = Document::load_from(Cursor::new(data))
        .map_err(|e| anyhow::anyhow!("Failed to load PDF: {}", e))?;
    
    let mut text = String::new();
    let pages = doc.get_pages();
    
    for (page_num, _) in pages {
        if let Ok(content) = doc.extract_text(&[page_num]) {
            text.push_str(&content);
            text.push('\n');
        }
    }
    
    Ok(text)
}

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
