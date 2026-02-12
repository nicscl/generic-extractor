"use client";

import { useState, useCallback } from "react";
import type { ExtractionSnapshot, ExtractionNode } from "@/lib/types";

const API_URL = "/api/extractor";

export function useExtraction(extractionId: string) {
  const [snapshot, setSnapshot] = useState<ExtractionSnapshot | null>(null);
  const [selectedNode, setSelectedNode] = useState<ExtractionNode | null>(null);
  const [nodeContent, setNodeContent] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const loadSnapshot = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const res = await fetch(`${API_URL}/extractions/${extractionId}/snapshot`);
      if (!res.ok) throw new Error(`Failed to load extraction: ${res.status}`);
      const data = await res.json();
      setSnapshot(data);
    } catch (err) {
      setError((err as Error).message);
    }
    setLoading(false);
  }, [extractionId]);

  const selectNode = useCallback(
    async (node: ExtractionNode) => {
      setSelectedNode(node);
      setNodeContent(null);
    },
    []
  );

  const loadContent = useCallback(async (contentRef: string) => {
    const refPath = contentRef.replace(/^content:\/\//, "");
    try {
      const res = await fetch(`${API_URL}/content/${refPath}`);
      if (!res.ok) throw new Error(`Failed to load content: ${res.status}`);
      const data = await res.json();
      setNodeContent(data.text ?? JSON.stringify(data, null, 2));
    } catch (err) {
      setNodeContent(`Error: ${(err as Error).message}`);
    }
  }, []);

  return {
    snapshot,
    selectedNode,
    nodeContent,
    loading,
    error,
    loadSnapshot,
    selectNode,
    loadContent,
  };
}
