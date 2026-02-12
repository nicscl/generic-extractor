-- Dataset tables for sheet extraction persistence
-- Run manually in Supabase SQL editor

CREATE TABLE extraction.datasets (
    id TEXT PRIMARY KEY,
    source_file TEXT NOT NULL,
    config_name TEXT,
    extracted_at TIMESTAMPTZ DEFAULT NOW(),
    summary TEXT,
    schemas JSONB NOT NULL,
    relationships JSONB,
    status TEXT DEFAULT 'processing'
);

CREATE TABLE extraction.dataset_rows (
    id TEXT PRIMARY KEY,
    dataset_id TEXT REFERENCES extraction.datasets(id),
    schema_name TEXT NOT NULL,
    row_data JSONB NOT NULL,
    page_num INTEGER,
    row_index INTEGER
);

CREATE INDEX idx_dataset_rows_dataset ON extraction.dataset_rows(dataset_id);
CREATE INDEX idx_dataset_rows_schema ON extraction.dataset_rows(dataset_id, schema_name);
