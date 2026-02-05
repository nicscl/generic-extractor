#![allow(dead_code)]
//! Content store for lazy-loaded document content with pagination support.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// Response from content retrieval with pagination info.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ContentChunk {
    pub content: String,
    pub offset: usize,
    pub limit: usize,
    pub total_chars: usize,
    pub has_more: bool,
}

/// In-memory content store.
/// 
/// Stores full text content and serves it with pagination support.
/// Content refs have format: `content://{node_id}`
#[derive(Debug, Clone, Default)]
pub struct ContentStore {
    inner: Arc<RwLock<HashMap<String, String>>>,
}

impl ContentStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Store content for a node, returns the content ref.
    pub fn store(&self, node_id: &str, content: String) -> String {
        let content_ref = format!("content://{}", node_id);
        let content_len = content.len();
        let mut store = self.inner.write().unwrap();
        store.insert(node_id.to_string(), content);
        tracing::debug!("ContentStore: stored '{}' ({} chars)", node_id, content_len);
        content_ref
    }

    /// Retrieve content with pagination.
    /// 
    /// - `content_ref`: The content reference (e.g., `content://node_id`)
    /// - `offset`: Character offset to start from
    /// - `limit`: Maximum characters to return
    pub fn get(&self, content_ref: &str, offset: usize, limit: usize) -> Option<ContentChunk> {
        let node_id = content_ref.strip_prefix("content://")?;
        let store = self.inner.read().unwrap();
        let content = store.get(node_id)?;

        let total_chars = content.chars().count();
        
        if offset >= total_chars {
            return Some(ContentChunk {
                content: String::new(),
                offset,
                limit,
                total_chars,
                has_more: false,
            });
        }

        // Get the substring by character indices (handles UTF-8 properly)
        let chars: Vec<char> = content.chars().collect();
        let end = (offset + limit).min(total_chars);
        let chunk: String = chars[offset..end].iter().collect();
        let has_more = end < total_chars;

        Some(ContentChunk {
            content: chunk,
            offset,
            limit,
            total_chars,
            has_more,
        })
    }

    /// Get full content without pagination.
    pub fn get_full(&self, content_ref: &str) -> Option<String> {
        let node_id = content_ref.strip_prefix("content://")?;
        let store = self.inner.read().unwrap();
        store.get(node_id).cloned()
    }

    /// Check if content exists.
    pub fn exists(&self, content_ref: &str) -> bool {
        if let Some(node_id) = content_ref.strip_prefix("content://") {
            let store = self.inner.read().unwrap();
            store.contains_key(node_id)
        } else {
            false
        }
    }

    /// Get total character count for a content ref.
    pub fn len(&self, content_ref: &str) -> Option<usize> {
        let node_id = content_ref.strip_prefix("content://")?;
        let store = self.inner.read().unwrap();
        store.get(node_id).map(|s| s.chars().count())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_store_and_retrieve() {
        let store = ContentStore::new();
        let content = "Hello, world! This is test content.".to_string();
        
        let ref_uri = store.store("test_node", content.clone());
        assert_eq!(ref_uri, "content://test_node");
        
        let full = store.get_full(&ref_uri).unwrap();
        assert_eq!(full, content);
    }

    #[test]
    fn test_pagination() {
        let store = ContentStore::new();
        let content = "ABCDEFGHIJ".to_string(); // 10 chars
        store.store("paginated", content);

        let chunk1 = store.get("content://paginated", 0, 5).unwrap();
        assert_eq!(chunk1.content, "ABCDE");
        assert_eq!(chunk1.total_chars, 10);
        assert!(chunk1.has_more);

        let chunk2 = store.get("content://paginated", 5, 5).unwrap();
        assert_eq!(chunk2.content, "FGHIJ");
        assert!(!chunk2.has_more);
    }

    #[test]
    fn test_utf8_pagination() {
        let store = ContentStore::new();
        let content = "Olá, você está bem?".to_string();
        store.store("utf8", content);

        // Should handle multi-byte chars correctly
        let chunk = store.get("content://utf8", 0, 10).unwrap();
        assert_eq!(chunk.content, "Olá, você ");
    }
}
