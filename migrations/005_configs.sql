-- Migration: extraction.configs table
-- Stores extraction configs as JSONB, replacing filesystem-based configs.

CREATE TABLE IF NOT EXISTS extraction.configs (
    name        TEXT PRIMARY KEY,
    config      JSONB NOT NULL,
    created_at  TIMESTAMPTZ DEFAULT NOW(),
    updated_at  TIMESTAMPTZ DEFAULT NOW()
);
