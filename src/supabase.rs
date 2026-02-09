//! Supabase client for uploading and reading extraction results.

use anyhow::{anyhow, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::{debug, info};

use crate::schema::{
    ConfidenceScores, DocumentNode, Extraction, Relationship, StructureMapEntry,
};

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
        let base_url =
            std::env::var("SUPABASE_URL").map_err(|_| anyhow!("SUPABASE_URL not set"))?;
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
        self.insert_nodes(&extraction.id, &extraction.children, None, content_store)
            .await?;

        // 3. Insert relationships
        self.insert_relationships(&extraction.id, &extraction.relationships)
            .await?;

        info!(
            "Successfully uploaded extraction {} to Supabase",
            extraction.id
        );
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

        let resp = self
            .client
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
            return Err(anyhow!(
                "Failed to insert extraction: {} - {}",
                status,
                text
            ));
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
                    self.insert_content(extraction_id, &node.id, &chunk.content)
                        .await?;
                }
            }

            // Recursively insert children
            if !node.children.is_empty() {
                Box::pin(self.insert_nodes(
                    extraction_id,
                    &node.children,
                    Some(&node.id),
                    content_store,
                ))
                .await?;
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

        let (page_start, page_end) = node
            .page_range
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

        let resp = self
            .client
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
            return Err(anyhow!(
                "Failed to insert node {}: {} - {}",
                node.id,
                status,
                text
            ));
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

        let resp = self
            .client
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
            return Err(anyhow!(
                "Failed to insert content for {}: {} - {}",
                node_id,
                status,
                text
            ));
        }

        debug!(
            "Inserted content for node: {} ({} chars)",
            node_id,
            content.len()
        );
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

        let bodies: Vec<_> = relationships
            .iter()
            .map(|r| {
                json!({
                    "extraction_id": extraction_id,
                    "from_node": r.from,
                    "to_node": r.to,
                    "relationship_type": r.rel_type,
                })
            })
            .collect();

        let resp = self
            .client
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
            return Err(anyhow!(
                "Failed to insert relationships: {} - {}",
                status,
                text
            ));
        }

        debug!("Inserted {} relationships", relationships.len());
        Ok(())
    }

    // ========================================================================
    // Read methods
    // ========================================================================

    /// Helper: GET from Supabase REST API.
    async fn get_json<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T> {
        let url = format!("{}/rest/v1/{}", self.base_url, path);
        let resp = self
            .client
            .get(&url)
            .header("apikey", &self.service_role_key)
            .header("Authorization", format!("Bearer {}", self.service_role_key))
            .header("Accept-Profile", "extraction")
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(anyhow!("Supabase GET {} failed: {} - {}", path, status, text));
        }

        Ok(resp.json().await?)
    }

    /// List all extractions (lightweight summaries).
    pub async fn list_extractions(&self) -> Result<Vec<ExtractionRow>> {
        self.get_json("extractions?select=id,config_name,source_file,content_hash,total_pages,summary,structure_map,metadata,extracted_at,extractor_version&order=extracted_at.desc")
            .await
    }

    /// Fetch a full extraction by ID, reconstructing the tree from flat nodes.
    pub async fn fetch_extraction(
        &self,
        id: &str,
        content_store: &crate::content_store::ContentStore,
    ) -> Result<Option<Extraction>> {
        // 1. Fetch main record
        let rows: Vec<ExtractionRow> = self
            .get_json(&format!(
                "extractions?id=eq.{}&select=*",
                id
            ))
            .await?;

        let row = match rows.into_iter().next() {
            Some(r) => r,
            None => return Ok(None),
        };

        // 2. Fetch all nodes
        let nodes: Vec<NodeRow> = self
            .get_json(&format!(
                "extraction_nodes?extraction_id=eq.{}&select=*",
                id
            ))
            .await?;

        // 3. Fetch all content
        let contents: Vec<ContentRow> = self
            .get_json(&format!(
                "node_content?extraction_id=eq.{}&select=node_id,content",
                id
            ))
            .await?;

        // Store content in content_store
        let content_map: std::collections::HashMap<String, String> = contents
            .into_iter()
            .map(|c| (c.node_id, c.content))
            .collect();

        for (node_id, content) in &content_map {
            content_store.store(node_id, content.clone());
        }

        // 4. Fetch relationships
        let rel_rows: Vec<RelationshipRow> = self
            .get_json(&format!(
                "extraction_relationships?extraction_id=eq.{}&select=*",
                id
            ))
            .await?;

        let relationships: Vec<Relationship> = rel_rows
            .into_iter()
            .map(|r| Relationship {
                from: r.from_node,
                to: r.to_node,
                rel_type: r.relationship_type,
                citation: None,
            })
            .collect();

        // 5. Reconstruct tree from flat nodes
        let children = build_tree(&nodes, &content_map);

        let extraction = Extraction {
            id: row.id,
            version: 1,
            status: crate::schema::ExtractionStatus::Completed,
            error: None,
            config_name: row.config_name,
            previous_version_id: None,
            content_hash: row.content_hash,
            source_file: row.source_file,
            extracted_at: row.extracted_at,
            extractor_version: row.extractor_version,
            total_pages: row.total_pages,
            summary: row.summary,
            structure_map: row.structure_map.unwrap_or_default(),
            relationships,
            metadata: row.metadata.unwrap_or(serde_json::Value::Null),
            children,
        };

        info!(
            "Hydrated extraction {} from Supabase ({} nodes)",
            extraction.id,
            nodes.len()
        );

        Ok(Some(extraction))
    }

    /// Fetch content for a single node by node_id.
    pub async fn fetch_content(&self, extraction_id: &str, node_id: &str) -> Result<Option<String>> {
        let rows: Vec<ContentRow> = self
            .get_json(&format!(
                "node_content?extraction_id=eq.{}&node_id=eq.{}&select=content",
                extraction_id, node_id
            ))
            .await?;

        Ok(rows.into_iter().next().map(|r| r.content))
    }

    /// Fetch content by node_id only (no extraction_id needed).
    pub async fn fetch_content_by_node_id(&self, node_id: &str) -> Result<Option<String>> {
        let rows: Vec<ContentRow> = self
            .get_json(&format!(
                "node_content?node_id=eq.{}&select=node_id,content&limit=1",
                node_id
            ))
            .await?;

        Ok(rows.into_iter().next().map(|r| r.content))
    }
}

// ============================================================================
// Supabase row types
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct ExtractionRow {
    pub id: String,
    pub config_name: Option<String>,
    pub source_file: String,
    pub content_hash: Option<String>,
    pub total_pages: Option<u32>,
    pub summary: String,
    pub structure_map: Option<Vec<StructureMapEntry>>,
    pub metadata: Option<serde_json::Value>,
    pub extracted_at: String,
    pub extractor_version: Option<String>,
}

#[derive(Debug, Deserialize)]
struct NodeRow {
    id: String,
    parent_id: Option<String>,
    #[serde(rename = "type")]
    node_type: String,
    subtype: Option<String>,
    label: Option<String>,
    page_start: Option<u32>,
    page_end: Option<u32>,
    date: Option<String>,
    author: Option<String>,
    summary: String,
    confidence: Option<ConfidenceScores>,
}

#[derive(Debug, Deserialize)]
struct ContentRow {
    node_id: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct RelationshipRow {
    from_node: String,
    to_node: String,
    relationship_type: String,
}

/// Build a nested tree from flat node rows.
fn build_tree(
    nodes: &[NodeRow],
    content_map: &std::collections::HashMap<String, String>,
) -> Vec<DocumentNode> {
    use std::collections::HashMap;

    // Index nodes by id
    let node_map: HashMap<&str, &NodeRow> = nodes.iter().map(|n| (n.id.as_str(), n)).collect();

    // Group children by parent_id
    let mut children_of: HashMap<Option<&str>, Vec<&str>> = HashMap::new();
    for node in nodes {
        children_of
            .entry(node.parent_id.as_deref())
            .or_default()
            .push(&node.id);
    }

    fn build_node(
        id: &str,
        node_map: &HashMap<&str, &NodeRow>,
        children_of: &HashMap<Option<&str>, Vec<&str>>,
        content_map: &std::collections::HashMap<String, String>,
    ) -> DocumentNode {
        let row = node_map[id];
        let page_range = match (row.page_start, row.page_end) {
            (Some(s), Some(e)) => Some([s, e]),
            _ => None,
        };
        let content_ref = if content_map.contains_key(id) {
            Some(format!("content://{}", id))
        } else {
            None
        };

        let children: Vec<DocumentNode> = children_of
            .get(&Some(id))
            .map(|ids| {
                ids.iter()
                    .map(|cid| build_node(cid, node_map, children_of, content_map))
                    .collect()
            })
            .unwrap_or_default();

        DocumentNode {
            id: row.id.clone(),
            node_type: row.node_type.clone(),
            subtype: row.subtype.clone(),
            label: row.label.clone(),
            page_range,
            date: row.date.clone(),
            author: row.author.clone(),
            summary: row.summary.clone(),
            references: Vec::new(),
            referenced_by: Vec::new(),
            content_ref,
            confidence: row.confidence.clone(),
            metadata: serde_json::Value::Null,
            children,
        }
    }

    // Root nodes have parent_id = None
    children_of
        .get(&None)
        .map(|ids| {
            ids.iter()
                .map(|id| build_node(id, &node_map, &children_of, content_map))
                .collect()
        })
        .unwrap_or_default()
}
