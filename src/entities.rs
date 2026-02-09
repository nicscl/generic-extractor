//! Entity extraction from OCR text using regex patterns.
//!
//! Pure functions, no async — easily testable. Walks the document tree,
//! reads content from ContentStore, runs compiled regex patterns, and
//! returns per-node entity metadata plus a global ReferenceIndex.

use std::collections::HashMap;

use regex::Regex;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::config::EntityPattern;
use crate::content_store::ContentStore;
use crate::schema::DocumentNode;

/// Pre-compiled regex patterns ready for matching.
pub struct CompiledPatterns {
    patterns: Vec<CompiledPattern>,
}

struct CompiledPattern {
    id: String,
    #[allow(dead_code)]
    label: String,
    regex: Regex,
    normalize: Option<String>,
    deduplicate: bool,
}

/// A single occurrence of an entity, tracking which nodes it appears in.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityOccurrence {
    pub value: String,
    pub node_ids: Vec<String>,
}

/// Global reference index: entity type → list of unique occurrences.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReferenceIndex {
    pub entities: HashMap<String, Vec<EntityOccurrence>>,
}

impl CompiledPatterns {
    /// Compile entity patterns from config. Skips invalid regexes with a warning.
    pub fn compile(patterns: &[EntityPattern]) -> Self {
        let mut compiled = Vec::new();
        for p in patterns {
            match Regex::new(&p.pattern) {
                Ok(regex) => {
                    compiled.push(CompiledPattern {
                        id: p.id.clone(),
                        label: p.label.clone(),
                        regex,
                        normalize: p.normalize.clone(),
                        deduplicate: p.deduplicate,
                    });
                }
                Err(e) => {
                    warn!(
                        "Skipping invalid entity pattern '{}' ({}): {}",
                        p.id, p.pattern, e
                    );
                }
            }
        }
        debug!("Compiled {} entity patterns", compiled.len());
        Self { patterns: compiled }
    }

    /// Returns true if there are no compiled patterns.
    pub fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }
}

/// Extract entities from all nodes in the tree.
///
/// Returns a map of node_id → entity metadata (JSON Value), plus a global ReferenceIndex.
pub fn extract_entities(
    nodes: &[DocumentNode],
    content_store: &ContentStore,
    compiled: &CompiledPatterns,
) -> (HashMap<String, serde_json::Value>, ReferenceIndex) {
    // node_id → { pattern_id → Vec<matched_value> }
    let mut node_entities: HashMap<String, HashMap<String, Vec<String>>> = HashMap::new();
    // pattern_id → { value → Vec<node_id> } for global index
    let mut global_index: HashMap<String, HashMap<String, Vec<String>>> = HashMap::new();

    // Recursively walk all nodes
    walk_nodes(
        nodes,
        content_store,
        compiled,
        &mut node_entities,
        &mut global_index,
    );

    // Convert node_entities to JSON values
    let node_metadata: HashMap<String, serde_json::Value> = node_entities
        .into_iter()
        .map(|(node_id, entities)| {
            let json = serde_json::to_value(entities).unwrap_or(serde_json::Value::Null);
            (node_id, json)
        })
        .collect();

    // Convert global_index to ReferenceIndex
    let reference_index = ReferenceIndex {
        entities: global_index
            .into_iter()
            .map(|(pattern_id, value_map)| {
                let occurrences: Vec<EntityOccurrence> = value_map
                    .into_iter()
                    .map(|(value, node_ids)| EntityOccurrence { value, node_ids })
                    .collect();
                (pattern_id, occurrences)
            })
            .collect(),
    };

    (node_metadata, reference_index)
}

/// Recursively walk the node tree, extracting entities from each node's content.
fn walk_nodes(
    nodes: &[DocumentNode],
    content_store: &ContentStore,
    compiled: &CompiledPatterns,
    node_entities: &mut HashMap<String, HashMap<String, Vec<String>>>,
    global_index: &mut HashMap<String, HashMap<String, Vec<String>>>,
) {
    for node in nodes {
        // Get content for this node from the content store
        let content_ref = format!("content://{}", node.id);
        if let Some(text) = content_store.get_full(&content_ref) {
            let entities = extract_from_text(&text, compiled);

            for (pattern_id, values) in &entities {
                // Update global index
                let global_entry = global_index.entry(pattern_id.clone()).or_default();
                for value in values {
                    global_entry
                        .entry(value.clone())
                        .or_default()
                        .push(node.id.clone());
                }
            }

            if !entities.is_empty() {
                node_entities.insert(node.id.clone(), entities);
            }
        }

        // Recurse into children
        if !node.children.is_empty() {
            walk_nodes(
                &node.children,
                content_store,
                compiled,
                node_entities,
                global_index,
            );
        }
    }
}

/// Run all compiled patterns against a text, returning pattern_id → matched values.
fn extract_from_text(
    text: &str,
    compiled: &CompiledPatterns,
) -> HashMap<String, Vec<String>> {
    let mut results: HashMap<String, Vec<String>> = HashMap::new();

    for pattern in &compiled.patterns {
        let mut values: Vec<String> = Vec::new();

        for cap in pattern.regex.captures_iter(text) {
            // Use first capture group if available, otherwise full match
            let raw = cap
                .get(1)
                .or_else(|| cap.get(0))
                .map(|m| m.as_str().to_string())
                .unwrap_or_default();

            if raw.is_empty() {
                continue;
            }

            let normalized = normalize_value(&raw, pattern.normalize.as_deref());
            values.push(normalized);
        }

        // Deduplicate if configured
        if pattern.deduplicate && !values.is_empty() {
            let mut seen = std::collections::HashSet::new();
            values.retain(|v| seen.insert(v.clone()));
        }

        if !values.is_empty() {
            results.insert(pattern.id.clone(), values);
        }
    }

    results
}

/// Apply normalization to a matched value.
fn normalize_value(value: &str, normalize: Option<&str>) -> String {
    match normalize {
        Some("uppercase") => value.to_uppercase(),
        Some("strip_punctuation") => value
            .chars()
            .filter(|c| c.is_alphanumeric())
            .collect(),
        Some("uppercase_strip_punctuation") => value
            .to_uppercase()
            .chars()
            .filter(|c| c.is_alphanumeric())
            .collect(),
        _ => value.to_string(),
    }
}

/// Deduplicate node_ids in the global reference index (a node may match
/// the same value multiple times, but we only want it listed once).
pub fn dedup_reference_index(index: &mut ReferenceIndex) {
    for occurrences in index.entities.values_mut() {
        for occ in occurrences.iter_mut() {
            occ.node_ids.sort();
            occ.node_ids.dedup();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::EntityPattern;

    fn make_patterns() -> Vec<EntityPattern> {
        vec![
            EntityPattern {
                id: "cpf".to_string(),
                label: "CPF".to_string(),
                pattern: r"(\d{3}\.\d{3}\.\d{3}-\d{2})".to_string(),
                normalize: Some("strip_punctuation".to_string()),
                deduplicate: true,
            },
            EntityPattern {
                id: "pnr".to_string(),
                label: "PNR / Localizador".to_string(),
                pattern: r"\b([A-Z]{6})\b".to_string(),
                normalize: Some("uppercase".to_string()),
                deduplicate: true,
            },
        ]
    }

    #[test]
    fn test_compile_patterns() {
        let patterns = make_patterns();
        let compiled = CompiledPatterns::compile(&patterns);
        assert_eq!(compiled.patterns.len(), 2);
    }

    #[test]
    fn test_extract_from_text() {
        let patterns = make_patterns();
        let compiled = CompiledPatterns::compile(&patterns);

        let text = "CPF: 123.456.789-00, Localizador VJLXXZ, outro CPF 123.456.789-00";
        let results = extract_from_text(text, &compiled);

        assert_eq!(results.get("cpf").unwrap(), &vec!["12345678900".to_string()]);
        assert_eq!(results.get("pnr").unwrap(), &vec!["VJLXXZ".to_string()]);
    }

    #[test]
    fn test_normalize_value() {
        assert_eq!(normalize_value("abc", Some("uppercase")), "ABC");
        assert_eq!(
            normalize_value("123.456.789-00", Some("strip_punctuation")),
            "12345678900"
        );
        assert_eq!(normalize_value("hello", None), "hello");
    }

    #[test]
    fn test_invalid_regex_skipped() {
        let patterns = vec![EntityPattern {
            id: "bad".to_string(),
            label: "Bad".to_string(),
            pattern: r"[invalid".to_string(),
            normalize: None,
            deduplicate: true,
        }];
        let compiled = CompiledPatterns::compile(&patterns);
        assert!(compiled.is_empty());
    }
}
