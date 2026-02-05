//! Supabase client for uploading extraction results.

use anyhow::{anyhow, Result};
use reqwest::Client;
use serde_json::json;
use tracing::{debug, info};

use crate::schema::{Extraction, DocumentNode, Relationship};

/// Supabase client configuration.
#[derive(Clone)]
pub struct SupabaseClient {
    client: Client,
    base_url: String,
    service_role_key: String,
}

impl SupabaseClient {
    /// Create a new Supabase client from environment variables.
    pub fn from_env() -> Result<Self> {
        let base_url = std::env::var("SUPABASE_URL")
            .map_err(|_| anyhow!("SUPABASE_URL not set"))?;
        let service_role_key = std::env::var("SUPABASE_SERVICE_ROLE_KEY")
            .map_err(|_| anyhow!("SUPABASE_SERVICE_ROLE_KEY not set"))?;
        
        Ok(Self {
            client: Client::new(),
            base_url,
            service_role_key,
        })
    }
    
    /// Upload an extraction to Supabase.
    pub async fn upload_extraction(
        &self,
        extraction: &Extraction,
        content_store: &crate::content_store::ContentStore,
    ) -> Result<()> {
        info!("Uploading extraction {} to Supabase", extraction.id);
        
        // 1. Insert main extraction record
        self.insert_extraction(extraction).await?;
        
        // 2. Insert nodes (flattened) and content
        self.insert_nodes(&extraction.id, &extraction.children, None, content_store).await?;
        
        // 3. Insert relationships
        self.insert_relationships(&extraction.id, &extraction.relationships).await?;
        
        info!("Successfully uploaded extraction {} to Supabase", extraction.id);
        Ok(())
    }
    
    /// Insert the main extraction record.
    async fn insert_extraction(&self, extraction: &Extraction) -> Result<()> {
        let url = format!("{}/rest/v1/extractions", self.base_url);
        
        let body = json!({
            "id": extraction.id,
            "config_name": extraction.config_name,
            "source_file": extraction.source_file,
            "content_hash": extraction.content_hash,
            "total_pages": extraction.total_pages,
            "summary": extraction.summary,
            "structure_map": extraction.structure_map,
            "metadata": extraction.metadata,
            "extracted_at": extraction.extracted_at,
            "extractor_version": extraction.extractor_version,
        });
        
        debug!("Inserting extraction: {}", extraction.id);
        
        let resp = self.client
            .post(&url)
            .header("apikey", &self.service_role_key)
            .header("Authorization", format!("Bearer {}", self.service_role_key))
            .header("Content-Type", "application/json")
            .header("Content-Profile", "extraction")
            .header("Prefer", "return=minimal")
            .json(&body)
            .send()
            .await?;
        
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(anyhow!("Failed to insert extraction: {} - {}", status, text));
        }
        
        Ok(())
    }
    
    /// Recursively insert nodes and their content.
    async fn insert_nodes(
        &self,
        extraction_id: &str,
        nodes: &[DocumentNode],
        parent_id: Option<&str>,
        content_store: &crate::content_store::ContentStore,
    ) -> Result<()> {
        for node in nodes {
            // Insert node
            self.insert_node(extraction_id, node, parent_id).await?;
            
            // Insert content if available
            if let Some(content_ref) = &node.content_ref {
                if let Some(chunk) = content_store.get(content_ref, 0, usize::MAX) {
                    self.insert_content(extraction_id, &node.id, &chunk.content).await?;
                }
            }
            
            // Recursively insert children
            if !node.children.is_empty() {
                Box::pin(self.insert_nodes(extraction_id, &node.children, Some(&node.id), content_store)).await?;
            }
        }
        
        Ok(())
    }
    
    /// Insert a single node.
    async fn insert_node(
        &self,
        extraction_id: &str,
        node: &DocumentNode,
        parent_id: Option<&str>,
    ) -> Result<()> {
        let url = format!("{}/rest/v1/extraction_nodes", self.base_url);
        
        let (page_start, page_end) = node.page_range
            .map(|arr| (Some(arr[0]), Some(arr[1])))
            .unwrap_or((None, None));
        
        let body = json!({
            "id": node.id,
            "extraction_id": extraction_id,
            "parent_id": parent_id,
            "type": node.node_type,
            "subtype": node.subtype,
            "label": node.label,
            "page_start": page_start,
            "page_end": page_end,
            "date": node.date,
            "author": node.author,
            "summary": node.summary,
            "confidence": node.confidence,
        });
        
        let resp = self.client
            .post(&url)
            .header("apikey", &self.service_role_key)
            .header("Authorization", format!("Bearer {}", self.service_role_key))
            .header("Content-Type", "application/json")
            .header("Content-Profile", "extraction")
            .header("Prefer", "return=minimal")
            .json(&body)
            .send()
            .await?;
        
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(anyhow!("Failed to insert node {}: {} - {}", node.id, status, text));
        }
        
        debug!("Inserted node: {}", node.id);
        Ok(())
    }
    
    /// Insert node content.
    async fn insert_content(
        &self,
        extraction_id: &str,
        node_id: &str,
        content: &str,
    ) -> Result<()> {
        let url = format!("{}/rest/v1/node_content", self.base_url);
        
        let body = json!({
            "extraction_id": extraction_id,
            "node_id": node_id,
            "content": content,
            "char_count": content.len(),
        });
        
        let resp = self.client
            .post(&url)
            .header("apikey", &self.service_role_key)
            .header("Authorization", format!("Bearer {}", self.service_role_key))
            .header("Content-Type", "application/json")
            .header("Content-Profile", "extraction")
            .header("Prefer", "return=minimal")
            .json(&body)
            .send()
            .await?;
        
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(anyhow!("Failed to insert content for {}: {} - {}", node_id, status, text));
        }
        
        debug!("Inserted content for node: {} ({} chars)", node_id, content.len());
        Ok(())
    }
    
    /// Insert relationships.
    async fn insert_relationships(
        &self,
        extraction_id: &str,
        relationships: &[Relationship],
    ) -> Result<()> {
        if relationships.is_empty() {
            return Ok(());
        }
        
        let url = format!("{}/rest/v1/extraction_relationships", self.base_url);
        
        let bodies: Vec<_> = relationships.iter().map(|r| json!({
            "extraction_id": extraction_id,
            "from_node": r.from,
            "to_node": r.to,
            "relationship_type": r.rel_type,
        })).collect();
        
        let resp = self.client
            .post(&url)
            .header("apikey", &self.service_role_key)
            .header("Authorization", format!("Bearer {}", self.service_role_key))
            .header("Content-Type", "application/json")
            .header("Content-Profile", "extraction")
            .header("Prefer", "return=minimal")
            .json(&bodies)
            .send()
            .await?;
        
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(anyhow!("Failed to insert relationships: {} - {}", status, text));
        }
        
        debug!("Inserted {} relationships", relationships.len());
        Ok(())
    }
}
