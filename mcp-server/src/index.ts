#!/usr/bin/env node

import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { z } from "zod";
import { readFile } from "node:fs/promises";
import { basename } from "node:path";

const API_URL = process.env.EXTRACTOR_API_URL ?? "http://localhost:3002";

// ---------------------------------------------------------------------------
// HTTP helper
// ---------------------------------------------------------------------------

async function api(path: string, init?: RequestInit): Promise<unknown> {
  const res = await fetch(`${API_URL}${path}`, init);
  if (!res.ok) {
    const body = await res.text().catch(() => "");
    throw new Error(`API ${res.status}: ${body}`);
  }
  return res.json();
}

// ---------------------------------------------------------------------------
// MCP Server
// ---------------------------------------------------------------------------

const server = new McpServer({
  name: "generic-extractor",
  version: "0.1.0",
});

// -- list_configs -----------------------------------------------------------

server.tool(
  "list_configs",
  "List available extraction configuration names (e.g. 'legal_br')",
  {},
  async () => {
    const configs = await api("/configs");
    return {
      content: [{ type: "text", text: JSON.stringify(configs, null, 2) }],
    };
  },
);

// -- extract_document -------------------------------------------------------

server.tool(
  "extract_document",
  "Upload a PDF file and run the extraction pipeline. Returns the full extraction result with ID, summary, structure map, and document tree.",
  {
    file_path: z.string().describe("Absolute path to the PDF file on disk"),
    config: z
      .string()
      .optional()
      .default("legal_br")
      .describe("Extraction config name"),
    upload: z
      .boolean()
      .optional()
      .default(true)
      .describe("Whether to persist the extraction to Supabase"),
  },
  async ({ file_path, config, upload }) => {
    const fileBuffer = await readFile(file_path);
    const fileName = basename(file_path);

    const form = new FormData();
    form.append(
      "file",
      new Blob([fileBuffer], { type: "application/pdf" }),
      fileName,
    );

    const params = new URLSearchParams();
    if (config) params.set("config", config);
    if (upload !== undefined) params.set("upload", String(upload));

    const result = await api(`/extract?${params.toString()}`, {
      method: "POST",
      body: form,
    });

    return {
      content: [{ type: "text", text: JSON.stringify(result, null, 2) }],
    };
  },
);

// -- get_extraction_snapshot ------------------------------------------------

server.tool(
  "get_extraction_snapshot",
  "Get the full extraction tree for an extraction ID. Returns hierarchical structure with summaries, structure map, relationships, and content index — but no raw content blobs. Use get_content to lazy-load actual text.",
  {
    extraction_id: z
      .string()
      .describe("The extraction ID (e.g. ext_abc123...)"),
  },
  async ({ extraction_id }) => {
    const result = await api(`/extractions/${extraction_id}/snapshot`);
    return {
      content: [{ type: "text", text: JSON.stringify(result, null, 2) }],
    };
  },
);

// -- get_node ---------------------------------------------------------------

server.tool(
  "get_node",
  "Get a specific node from an extraction by its node ID. Returns the node with type, label, summary, page range, content_ref, and children.",
  {
    extraction_id: z.string().describe("The extraction ID"),
    node_id: z.string().describe("The node ID within the extraction"),
  },
  async ({ extraction_id, node_id }) => {
    const result = await api(
      `/extractions/${extraction_id}/node/${node_id}`,
    );
    return {
      content: [{ type: "text", text: JSON.stringify(result, null, 2) }],
    };
  },
);

// -- get_content ------------------------------------------------------------

server.tool(
  "get_content",
  "Lazy-load the text content for a node via its content:// reference. Supports pagination. Returns the text chunk, total character count, and whether more content is available.",
  {
    ref: z
      .string()
      .describe(
        "The content reference (e.g. 'content://node_abc123' or just 'node_abc123')",
      ),
    offset: z
      .number()
      .int()
      .min(0)
      .optional()
      .default(0)
      .describe("Character offset to start from"),
    limit: z
      .number()
      .int()
      .min(1)
      .optional()
      .default(4000)
      .describe("Maximum characters to return"),
  },
  async ({ ref, offset, limit }) => {
    const refPath = ref.replace(/^content:\/\//, "");

    const params = new URLSearchParams();
    if (offset !== undefined) params.set("offset", String(offset));
    if (limit !== undefined) params.set("limit", String(limit));

    const qs = params.toString();
    const result = await api(`/content/${refPath}${qs ? `?${qs}` : ""}`);
    return {
      content: [{ type: "text", text: JSON.stringify(result, null, 2) }],
    };
  },
);

// ---------------------------------------------------------------------------
// Start
// ---------------------------------------------------------------------------

async function main() {
  const transport = new StdioServerTransport();
  await server.connect(transport);
}

main().catch((err) => {
  console.error("Fatal error:", err);
  process.exit(1);
});
