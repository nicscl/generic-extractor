"use client";

import { useState, useCallback } from "react";
import type { Dataset } from "@/lib/types";

const API_URL = "/api/extractor";

export function useDataset(datasetId: string) {
  const [dataset, setDataset] = useState<Dataset | null>(null);
  const [selectedSchema, setSelectedSchema] = useState<string | null>(null);
  const [rows, setRows] = useState<Record<string, unknown>[]>([]);
  const [totalRows, setTotalRows] = useState(0);
  const [offset, setOffset] = useState(0);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const limit = 50;

  const loadDataset = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const res = await fetch(`${API_URL}/datasets/${datasetId}`);
      if (!res.ok) throw new Error(`Failed to load dataset: ${res.status}`);
      const data: Dataset = await res.json();
      setDataset(data);
      if (data.schemas.length > 0) {
        setSelectedSchema(data.schemas[0].name);
        setRows(data.schemas[0].rows.slice(0, limit));
        setTotalRows(data.schemas[0].row_count);
      }
    } catch (err) {
      setError((err as Error).message);
    }
    setLoading(false);
  }, [datasetId]);

  const selectSchema = useCallback(
    (schemaName: string) => {
      setSelectedSchema(schemaName);
      setOffset(0);
      if (dataset) {
        const schema = dataset.schemas.find((s) => s.name === schemaName);
        if (schema) {
          setRows(schema.rows.slice(0, limit));
          setTotalRows(schema.row_count);
        }
      }
    },
    [dataset]
  );

  const loadPage = useCallback(
    async (newOffset: number) => {
      if (!selectedSchema) return;
      setLoading(true);
      try {
        const params = new URLSearchParams({
          schema_name: selectedSchema,
          offset: String(newOffset),
          limit: String(limit),
        });
        const res = await fetch(
          `${API_URL}/datasets/${datasetId}/rows?${params.toString()}`
        );
        if (!res.ok) throw new Error(`Failed to load rows: ${res.status}`);
        const data = await res.json();
        setRows(data.rows ?? data);
        setOffset(newOffset);
      } catch (err) {
        setError((err as Error).message);
      }
      setLoading(false);
    },
    [datasetId, selectedSchema]
  );

  return {
    dataset,
    selectedSchema,
    rows,
    totalRows,
    offset,
    limit,
    loading,
    error,
    loadDataset,
    selectSchema,
    loadPage,
  };
}
