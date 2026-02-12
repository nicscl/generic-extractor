//! LLM-based schema discovery for tabular data.
//!
//! Phase 1: Single-turn extraction — sends a data sample to the LLM which discovers
//! schemas, defines column types, and classifies rows.

use crate::config::ExtractionConfig;
use crate::openrouter::{Message, OpenRouterClient};
use crate::sheet_parser::RawSheet;
use crate::sheet_schema::{ColumnDef, DataSchema, SchemaRelationship, SheetExtraction};
use anyhow::{Context, Result};
use tracing::{debug, info};

/// Maximum rows to include in the data sample sent to the LLM.
const MAX_SAMPLE_ROWS: usize = 50;

/// Sheet extraction pipeline orchestrator.
pub struct SheetExtractor {
    client: OpenRouterClient,
}

impl SheetExtractor {
    pub fn new(client: OpenRouterClient) -> Self {
        Self { client }
    }

    /// Run schema discovery on parsed sheets.
    pub async fn extract(
        &self,
        filename: &str,
        sheets: &[RawSheet],
        config: &ExtractionConfig,
    ) -> Result<SheetExtraction> {
        info!(
            "Starting sheet extraction for: {} ({} sheets, config={})",
            filename,
            sheets.len(),
            config.name
        );

        let total_rows: usize = sheets.iter().map(|s| s.rows.len()).sum();
        info!("Total rows across all sheets: {}", total_rows);

        // Build data sample for the LLM
        let data_sample = build_data_sample(sheets, MAX_SAMPLE_ROWS);

        // Build prompts following the cache-friendly pattern:
        // System = generic strategy + data (stable, cacheable)
        // User = config-driven instructions (variable)
        let system_prompt = format!(
            r#"You are a tabular data analyst. You receive raw tabular data (CSV/spreadsheet rows) and must discover the schema(s) within.

Your task:
1. Identify one or more logical schemas (tables) in the data
2. Define typed columns for each schema
3. Identify relationships between schemas if multiple exist
4. Map all rows to their corresponding schema

Rules:
- Column names should be lowercase_snake_case
- Every row must belong to exactly one schema
- Non-tabular context (headers, annotations) should become metadata columns
- Be specific about data types: "string", "integer", "float", "date", "currency_brl", "currency_usd", "boolean"

Available transforms you may assign to columns:
- parse_date_br (DD/MM/YYYY → ISO8601)
- parse_date_us (MM/DD/YYYY → ISO8601)
- parse_currency_brl ("1.234,56" → 1234.56)
- parse_currency_usd ("1,234.56" → 1234.56)
- normalize_cpf (strip punctuation from CPF)
- normalize_cnpj (strip punctuation from CNPJ)
- strip_whitespace (trim + collapse)
- to_uppercase
- to_lowercase
- to_number (parse as float)
- to_integer (parse as int)

--- DATA START ---

{}

--- DATA END ---"#,
            data_sample
        );

        // Build user prompt with config-driven hints
        let mut user_sections = Vec::new();

        if let Some(ref sheet_config) = config.sheet_config {
            if !sheet_config.expected_columns.is_empty() {
                let cols: Vec<String> = sheet_config
                    .expected_columns
                    .iter()
                    .map(|c| {
                        let mut desc = c.name.clone();
                        if let Some(ref dt) = c.data_type {
                            desc.push_str(&format!(" ({})", dt));
                        }
                        if let Some(ref fmt) = c.format {
                            desc.push_str(&format!(" [{}]", fmt));
                        }
                        if c.required {
                            desc.push_str(" *required*");
                        }
                        desc
                    })
                    .collect();
                user_sections.push(format!("Expected columns:\n{}", cols.join("\n")));
            }

            if let Some(ref hints) = sheet_config.classification_hints {
                user_sections.push(format!("Business context:\n{}", hints));
            }
        }

        let user_prompt = format!(
            r#"{}Analyze the data above and return ONLY valid JSON with this structure:

{{
  "summary": "2-4 sentence overview of the dataset",
  "schemas": [
    {{
      "name": "lowercase_snake_case_name",
      "description": "What this table represents",
      "columns": [
        {{
          "name": "column_name",
          "data_type": "string|integer|float|date|currency_brl|boolean",
          "format": "original format if relevant (e.g. DD/MM/YYYY)",
          "transform": "transform_name or null",
          "required": true,
          "source": "cell|header|annotation",
          "description": "What this column represents"
        }}
      ]
    }}
  ],
  "relationships": [
    {{
      "from": "schema_name.column_name",
      "to": "schema_name.column_name",
      "type": "references"
    }}
  ]
}}"#,
            if user_sections.is_empty() {
                String::new()
            } else {
                format!("{}\n\n", user_sections.join("\n\n"))
            }
        );

        let messages = vec![Message::system(system_prompt), Message::user(user_prompt)];

        debug!("Calling LLM for schema discovery");
        let response = self.client.chat(messages).await?;
        debug!("LLM response length: {} chars", response.len());

        // Parse LLM response
        let discovered: DiscoveredSchemas =
            parse_llm_json(&response).context("Failed to parse LLM schema response")?;

        info!(
            "Discovered {} schema(s), {} relationship(s)",
            discovered.schemas.len(),
            discovered.relationships.len()
        );

        // Map raw rows to discovered schemas
        let populated_schemas = map_rows_to_schemas(sheets, discovered.schemas)?;

        // Build result
        let mut extraction = SheetExtraction::new(filename.to_string(), Some(config.name.clone()));
        extraction.summary = discovered.summary;
        extraction.schemas = populated_schemas;
        extraction.relationships = discovered
            .relationships
            .into_iter()
            .map(|r| SchemaRelationship {
                from: r.from,
                to: r.to,
                rel_type: r.rel_type,
            })
            .collect();

        info!(
            "Sheet extraction complete: {} schemas, {} total rows",
            extraction.schemas.len(),
            extraction.schemas.iter().map(|s| s.row_count).sum::<usize>()
        );

        Ok(extraction)
    }
}

/// Build a readable text representation of sheet data for the LLM prompt.
fn build_data_sample(sheets: &[RawSheet], max_rows: usize) -> String {
    let mut parts = Vec::new();

    for sheet in sheets {
        let mut section = format!("Sheet: \"{}\" ({} rows)\n", sheet.name, sheet.rows.len());

        // Header row
        section.push_str(&format!("| {} |\n", sheet.headers.join(" | ")));
        section.push_str(&format!(
            "|{}|\n",
            sheet
                .headers
                .iter()
                .map(|h| "-".repeat(h.len().max(3) + 2))
                .collect::<Vec<_>>()
                .join("|")
        ));

        // Data rows (limited)
        let row_limit = max_rows.min(sheet.rows.len());
        for row in &sheet.rows[..row_limit] {
            section.push_str(&format!("| {} |\n", row.join(" | ")));
        }

        if sheet.rows.len() > row_limit {
            section.push_str(&format!("... ({} more rows)\n", sheet.rows.len() - row_limit));
        }

        parts.push(section);
    }

    parts.join("\n")
}

/// Map raw rows to discovered schemas, producing typed JSON objects.
///
/// Matching strategy (per sheet × schema):
/// 1. **Name-based**: match LLM column names against sheet headers (case-insensitive).
///    Used when ≥50% of schema columns match a header.
/// 2. **Positional fallback**: map columns by index position when name matching fails.
///    Common for OCR-extracted tables where "headers" are actually the first data row.
///    Used when column count is close (sheet cols ≥ schema cols - 1).
fn map_rows_to_schemas(
    sheets: &[RawSheet],
    schemas: Vec<DiscoveredSchema>,
) -> Result<Vec<DataSchema>> {
    let mut result = Vec::new();

    for schema in schemas {
        let column_names: Vec<&str> = schema.columns.iter().map(|c| c.name.as_str()).collect();
        let mut rows = Vec::new();

        for sheet in sheets {
            // Build header-to-index mapping
            let header_map: std::collections::HashMap<String, usize> = sheet
                .headers
                .iter()
                .enumerate()
                .map(|(i, h)| (h.to_lowercase().trim().to_string(), i))
                .collect();

            // Try name-based matching first
            let name_matched: Vec<(&str, usize)> = column_names
                .iter()
                .filter_map(|col| {
                    header_map
                        .get(&col.to_lowercase())
                        .map(|&idx| (*col, idx))
                })
                .collect();

            // Use name matching if ≥50% of columns match
            let use_name_matching = name_matched.len() * 2 >= column_names.len();

            if use_name_matching && !name_matched.is_empty() {
                // Name-based mapping
                for raw_row in &sheet.rows {
                    let mut obj = serde_json::Map::new();
                    for (col_name, idx) in &name_matched {
                        let value = raw_row
                            .get(*idx)
                            .map(|v| v.as_str())
                            .unwrap_or("");
                        obj.insert(
                            col_name.to_string(),
                            serde_json::Value::String(value.to_string()),
                        );
                    }
                    rows.push(serde_json::Value::Object(obj));
                }
            } else {
                // Positional fallback: map columns by index
                // Only if sheet has enough columns (allow schema to have 1-2 extra
                // inferred columns like "categoria" that don't exist in raw data)
                let sheet_cols = sheet.headers.len();
                let schema_cols = column_names.len();
                let mappable = sheet_cols.min(schema_cols);

                if mappable == 0 || sheet_cols + 2 < schema_cols {
                    continue; // Column count too different, skip this sheet
                }

                debug!(
                    "Using positional mapping for schema '{}' on sheet '{}' ({} sheet cols → {} schema cols)",
                    schema.name, sheet.name, sheet_cols, schema_cols
                );

                // The "headers" row is actually data for headerless tables — include it
                let include_header_as_data = name_matched.is_empty();

                if include_header_as_data {
                    let mut obj = serde_json::Map::new();
                    for (i, col_name) in column_names.iter().enumerate().take(mappable) {
                        let value = &sheet.headers[i];
                        obj.insert(col_name.to_string(), serde_json::Value::String(value.clone()));
                    }
                    rows.push(serde_json::Value::Object(obj));
                }

                for raw_row in &sheet.rows {
                    let mut obj = serde_json::Map::new();
                    for (i, col_name) in column_names.iter().enumerate().take(mappable) {
                        let value = raw_row
                            .get(i)
                            .map(|v| v.as_str())
                            .unwrap_or("");
                        obj.insert(
                            col_name.to_string(),
                            serde_json::Value::String(value.to_string()),
                        );
                    }
                    rows.push(serde_json::Value::Object(obj));
                }
            }
        }

        let row_count = rows.len();
        info!(
            "Schema '{}': mapped {} rows from {} sheets",
            schema.name, row_count, sheets.len()
        );

        result.push(DataSchema {
            name: schema.name,
            description: schema.description,
            columns: schema
                .columns
                .into_iter()
                .map(|c| ColumnDef {
                    name: c.name,
                    data_type: c.data_type,
                    format: c.format,
                    transform: c.transform,
                    required: c.required,
                    source: c.source,
                    description: c.description,
                })
                .collect(),
            row_count,
            rows,
        });
    }

    Ok(result)
}

// ============================================================================
// LLM response types
// ============================================================================

#[derive(Debug, serde::Deserialize)]
struct DiscoveredSchemas {
    summary: String,
    #[serde(default)]
    schemas: Vec<DiscoveredSchema>,
    #[serde(default)]
    relationships: Vec<DiscoveredRelationship>,
}

#[derive(Debug, serde::Deserialize)]
struct DiscoveredSchema {
    name: String,
    description: String,
    #[serde(default)]
    columns: Vec<DiscoveredColumn>,
}

#[derive(Debug, serde::Deserialize)]
struct DiscoveredColumn {
    name: String,
    data_type: String,
    #[serde(default)]
    format: Option<String>,
    #[serde(default)]
    transform: Option<String>,
    #[serde(default)]
    required: bool,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct DiscoveredRelationship {
    from: String,
    to: String,
    #[serde(rename = "type")]
    rel_type: String,
}

// ============================================================================
// Helpers
// ============================================================================

/// Parse JSON from LLM response, stripping markdown code blocks if present.
/// Same pattern as `extractor::parse_llm_json`.
fn parse_llm_json<T: serde::de::DeserializeOwned>(response: &str) -> Result<T> {
    let json_str = if response.contains("```json") {
        response
            .split("```json")
            .nth(1)
            .and_then(|s| s.split("```").next())
            .unwrap_or(response)
            .trim()
    } else if response.contains("```") {
        response.split("```").nth(1).unwrap_or(response).trim()
    } else {
        response.trim()
    };

    let _: serde_json::Value = serde_json::from_str(json_str).context(format!(
        "Invalid JSON syntax: {}",
        &json_str.chars().take(200).collect::<String>()
    ))?;

    serde_json::from_str(json_str).context(format!(
        "JSON structure mismatch: {}",
        &json_str.chars().take(200).collect::<String>()
    ))
}
