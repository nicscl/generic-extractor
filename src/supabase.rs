//! Supabase client for uploading and reading extraction results.

use anyhow::{anyhow, Result};
use reqwest::Client;
use serde::Deserialize;
use serde_json::json;
use tracing::{debug, info};

use crate::config::ExtractionConfig;
use crate::schema::{
    ConfidenceScores, DocumentNode, Extraction, ExtractionStatus, Relationship, StructureMapEntry,
};
use crate::sheet_schema::{ColumnDef, DataSchema, SchemaRelationship, SheetExtraction};

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

        let reference_index = if extraction.reference_index.is_null() {
            None
        } else {
            Some(&extraction.reference_index)
        };

        let body = json!({
            "id": extraction.id,
            "config_name": extraction.config_name,
            "source_file": extraction.source_file,
            "content_hash": extraction.content_hash,
            "total_pages": extraction.total_pages,
            "summary": extraction.summary,
            "structure_map": extraction.structure_map,
            "metadata": extraction.metadata,
            "reference_index": reference_index,
            "readable_id": extraction.readable_id,
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

        let metadata = if node.metadata.is_null() {
            None
        } else {
            Some(&node.metadata)
        };

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
            "node_metadata": metadata,
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
        self.get_json("extractions?select=id,config_name,source_file,content_hash,total_pages,summary,structure_map,metadata,readable_id,extracted_at,extractor_version&order=extracted_at.desc")
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
            reference_index: row.reference_index.unwrap_or(serde_json::Value::Null),
            readable_id: row.readable_id,
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

    // ========================================================================
    // Dataset methods (sheet extraction persistence)
    // ========================================================================

    /// Upload a sheet extraction (dataset) to Supabase.
    pub async fn upload_dataset(&self, dataset: &SheetExtraction) -> Result<()> {
        info!("Uploading dataset {} to Supabase", dataset.id);

        // 1. Build schemas JSONB (column defs only, no rows)
        let schemas_json: Vec<serde_json::Value> = dataset
            .schemas
            .iter()
            .map(|s| {
                json!({
                    "name": s.name,
                    "description": s.description,
                    "columns": s.columns,
                    "row_count": s.row_count,
                })
            })
            .collect();

        let relationships_json: serde_json::Value = serde_json::to_value(&dataset.relationships)?;

        // 2. Insert main dataset record
        let url = format!("{}/rest/v1/datasets", self.base_url);
        let body = json!({
            "id": dataset.id,
            "source_file": dataset.source_file,
            "config_name": dataset.config_name,
            "extracted_at": dataset.extracted_at,
            "summary": dataset.summary,
            "schemas": schemas_json,
            "relationships": relationships_json,
            "status": "completed",
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
                "Failed to insert dataset: {} - {}",
                status,
                text
            ));
        }

        // 3. Batch insert rows into dataset_rows (100 per batch)
        let rows_url = format!("{}/rest/v1/dataset_rows", self.base_url);
        let mut total_inserted = 0usize;

        for schema in &dataset.schemas {
            let mut batch: Vec<serde_json::Value> = Vec::with_capacity(100);

            for (row_idx, row_data) in schema.rows.iter().enumerate() {
                batch.push(json!({
                    "id": format!("dsr_{}", uuid::Uuid::new_v4().simple()),
                    "dataset_id": dataset.id,
                    "schema_name": schema.name,
                    "row_data": row_data,
                    "row_index": row_idx,
                }));

                if batch.len() >= 100 {
                    self.post_batch(&rows_url, &batch).await?;
                    total_inserted += batch.len();
                    info!(
                        "Inserted {} rows for dataset {} schema '{}'",
                        total_inserted, dataset.id, schema.name
                    );
                    batch.clear();
                }
            }

            // Flush remaining
            if !batch.is_empty() {
                self.post_batch(&rows_url, &batch).await?;
                total_inserted += batch.len();
            }
        }

        info!(
            "Successfully uploaded dataset {} to Supabase ({} rows)",
            dataset.id, total_inserted
        );
        Ok(())
    }

    /// POST a batch of JSON objects.
    async fn post_batch(&self, url: &str, batch: &[serde_json::Value]) -> Result<()> {
        let resp = self
            .client
            .post(url)
            .header("apikey", &self.service_role_key)
            .header("Authorization", format!("Bearer {}", self.service_role_key))
            .header("Content-Type", "application/json")
            .header("Content-Profile", "extraction")
            .header("Prefer", "return=minimal")
            .json(batch)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(anyhow!("Failed to insert batch: {} - {}", status, text));
        }
        Ok(())
    }

    /// List all datasets (lightweight summaries).
    pub async fn list_datasets(&self) -> Result<Vec<DatasetRow>> {
        self.get_json("datasets?select=id,source_file,config_name,extracted_at,summary,status,schemas&order=extracted_at.desc")
            .await
    }

    /// Fetch a full dataset by ID, reconstructing from Supabase tables.
    pub async fn fetch_dataset(&self, id: &str) -> Result<Option<SheetExtraction>> {
        // 1. Fetch main record
        let rows: Vec<DatasetRow> = self
            .get_json(&format!("datasets?id=eq.{}&select=*", id))
            .await?;

        let row = match rows.into_iter().next() {
            Some(r) => r,
            None => return Ok(None),
        };

        // 2. Fetch all dataset rows
        let data_rows: Vec<DatasetRowEntry> = self
            .get_json(&format!(
                "dataset_rows?dataset_id=eq.{}&select=*&order=row_index",
                id
            ))
            .await?;

        // 3. Reconstruct schemas by merging column defs from JSONB + rows
        let schema_defs: Vec<DatasetSchemaJson> =
            serde_json::from_value(row.schemas.clone()).unwrap_or_default();

        let mut schemas: Vec<DataSchema> = schema_defs
            .into_iter()
            .map(|s| {
                let rows_for_schema: Vec<serde_json::Value> = data_rows
                    .iter()
                    .filter(|r| r.schema_name == s.name)
                    .map(|r| r.row_data.clone())
                    .collect();

                let row_count = rows_for_schema.len();
                DataSchema {
                    name: s.name,
                    description: s.description,
                    columns: s.columns,
                    row_count,
                    rows: rows_for_schema,
                }
            })
            .collect();

        // Update row_count based on actual rows
        for schema in &mut schemas {
            schema.row_count = schema.rows.len();
        }

        let relationships: Vec<SchemaRelationship> = row
            .relationships
            .as_ref()
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();

        let dataset = SheetExtraction {
            id: row.id,
            status: ExtractionStatus::Completed,
            error: None,
            config_name: row.config_name,
            source_file: row.source_file,
            extracted_at: row.extracted_at,
            summary: row.summary,
            schemas,
            relationships,
        };

        info!(
            "Hydrated dataset {} from Supabase ({} rows)",
            dataset.id,
            data_rows.len()
        );

        Ok(Some(dataset))
    }

    /// Query rows from a specific schema within a dataset (paginated).
    pub async fn query_dataset_rows(
        &self,
        dataset_id: &str,
        schema_name: &str,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<serde_json::Value>> {
        let rows: Vec<DatasetRowEntry> = self
            .get_json(&format!(
                "dataset_rows?dataset_id=eq.{}&schema_name=eq.{}&select=row_data&order=row_index&offset={}&limit={}",
                dataset_id, schema_name, offset, limit
            ))
            .await?;

        Ok(rows.into_iter().map(|r| r.row_data).collect())
    }

    // ========================================================================
    // Config methods
    // ========================================================================

    /// List all configs from Supabase.
    pub async fn list_configs(&self) -> Result<Vec<ExtractionConfig>> {
        let rows: Vec<ConfigRow> = self
            .get_json("configs?select=config&order=name")
            .await?;
        Ok(rows.into_iter().map(|r| r.config).collect())
    }

    /// Get a single config by name.
    pub async fn get_config(&self, name: &str) -> Result<Option<ExtractionConfig>> {
        let rows: Vec<ConfigRow> = self
            .get_json(&format!("configs?name=eq.{}&select=config", name))
            .await?;
        Ok(rows.into_iter().next().map(|r| r.config))
    }

    /// Upsert a config (insert or update).
    pub async fn upsert_config(&self, config: &ExtractionConfig) -> Result<()> {
        let url = format!("{}/rest/v1/configs", self.base_url);

        let body = json!({
            "name": config.name,
            "config": config,
        });

        let resp = self
            .client
            .post(&url)
            .header("apikey", &self.service_role_key)
            .header("Authorization", format!("Bearer {}", self.service_role_key))
            .header("Content-Type", "application/json")
            .header("Content-Profile", "extraction")
            .header("Prefer", "resolution=merge-duplicates,return=minimal")
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(anyhow!(
                "Failed to upsert config '{}': {} - {}",
                config.name,
                status,
                text
            ));
        }

        debug!("Upserted config: {}", config.name);
        Ok(())
    }

    /// Delete a config by name.
    pub async fn delete_config(&self, name: &str) -> Result<()> {
        let url = format!(
            "{}/rest/v1/configs?name=eq.{}",
            self.base_url, name
        );

        let resp = self
            .client
            .delete(&url)
            .header("apikey", &self.service_role_key)
            .header("Authorization", format!("Bearer {}", self.service_role_key))
            .header("Content-Profile", "extraction")
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(anyhow!(
                "Failed to delete config '{}': {} - {}",
                name,
                status,
                text
            ));
        }

        debug!("Deleted config: {}", name);
        Ok(())
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
    pub reference_index: Option<serde_json::Value>,
    pub readable_id: Option<String>,
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
    #[serde(default)]
    metadata: Option<serde_json::Value>,
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

// ============================================================================
// Config row types
// ============================================================================

#[derive(Debug, Deserialize)]
struct ConfigRow {
    config: ExtractionConfig,
}

// ============================================================================
// Dataset row types
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct DatasetRow {
    pub id: String,
    pub source_file: String,
    pub config_name: Option<String>,
    pub extracted_at: String,
    pub summary: String,
    pub schemas: serde_json::Value,
    pub relationships: Option<serde_json::Value>,
    pub status: Option<String>,
}

/// Schema definition as stored in the JSONB `schemas` column.
#[derive(Debug, Deserialize)]
struct DatasetSchemaJson {
    name: String,
    description: String,
    #[serde(default)]
    columns: Vec<ColumnDef>,
    #[allow(dead_code)]
    #[serde(default)]
    row_count: usize,
}

/// A single row entry from `dataset_rows` table.
#[derive(Debug, Deserialize)]
struct DatasetRowEntry {
    #[allow(dead_code)]
    #[serde(default)]
    id: Option<String>,
    #[allow(dead_code)]
    #[serde(default)]
    dataset_id: Option<String>,
    #[serde(default)]
    schema_name: String,
    row_data: serde_json::Value,
    #[allow(dead_code)]
    #[serde(default)]
    row_index: Option<i64>,
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
            metadata: row.metadata.clone().unwrap_or(serde_json::Value::Null),
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
