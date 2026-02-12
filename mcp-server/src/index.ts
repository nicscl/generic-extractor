#!/usr/bin/env node

import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { StreamableHTTPServerTransport } from "@modelcontextprotocol/sdk/server/streamableHttp.js";
import { z } from "zod";
import { readFile } from "node:fs/promises";
import { basename } from "node:path";
import { createServer, IncomingMessage, ServerResponse } from "node:http";
import { randomUUID } from "node:crypto";

const API_URL = process.env.EXTRACTOR_API_URL ?? "http://localhost:3002";
const TRANSPORT = process.env.MCP_TRANSPORT ?? "stdio"; // "stdio" or "http"
const MCP_PORT = parseInt(process.env.MCP_PORT ?? "3003", 10);

// ---------------------------------------------------------------------------
// HTTP helper
// ---------------------------------------------------------------------------

async function api(path: string, init?: RequestInit): Promise<unknown> {
  const res = await fetch(`${API_URL}${path}`, init);
  if (!res.ok) {
    const body = await res.text().catch(() => "");
    throw new Error(`API ${res.status}: ${body}`);
  }
  if (res.status === 204) return null;
  return res.json();
}

// ---------------------------------------------------------------------------
// Tool registration — applied to every McpServer instance
// ---------------------------------------------------------------------------

function registerTools(server: McpServer) {
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

  server.tool(
    "list_extractions",
    "List all available extractions with their IDs, source files, readable_id (human-readable document identifier like a case number or invoice ID), summaries, and page counts. Use this to find extraction IDs for get_extraction_snapshot. Supports filtering by readable_id.",
    {
      readable_id: z
        .string()
        .optional()
        .describe(
          "Filter by readable_id (case-insensitive substring match). Example: '0266175' to find a specific case.",
        ),
    },
    async ({ readable_id }) => {
      const params = new URLSearchParams();
      if (readable_id) params.set("readable_id", readable_id);
      const qs = params.toString();
      const extractions = await api(`/extractions${qs ? `?${qs}` : ""}`);
      return {
        content: [{ type: "text", text: JSON.stringify(extractions, null, 2) }],
      };
    },
  );

  server.tool(
    "extract_document",
    `Upload a PDF and run the extraction pipeline. Provide the file via exactly ONE of:
- file_path: local filesystem path (for STDIO/local usage)
- file_base64: base64-encoded PDF content (for remote HTTP usage)
- file_url: URL to download the PDF from (for remote HTTP usage)
Returns the full extraction result with ID, summary, structure map, and document tree.`,
    {
      file_path: z
        .string()
        .optional()
        .describe("Absolute path to the PDF file on disk (local mode)"),
      file_base64: z
        .string()
        .optional()
        .describe(
          "Base64-encoded PDF file content (remote mode). Must also provide file_name.",
        ),
      file_url: z
        .string()
        .url()
        .optional()
        .describe(
          "URL to download the PDF from (remote mode). The server will fetch it.",
        ),
      file_name: z
        .string()
        .optional()
        .describe(
          "Filename for the PDF (required with file_base64, optional with file_url). Example: 'document.pdf'",
        ),
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
    async ({ file_path, file_base64, file_url, file_name, config, upload }) => {
      let fileBuffer: Uint8Array;
      let fileName: string;

      const sourceCount = [file_path, file_base64, file_url].filter(Boolean).length;
      if (sourceCount === 0) {
        return {
          content: [
            {
              type: "text",
              text: "Error: Provide exactly one of file_path, file_base64, or file_url.",
            },
          ],
          isError: true,
        };
      }
      if (sourceCount > 1) {
        return {
          content: [
            {
              type: "text",
              text: "Error: Provide only ONE of file_path, file_base64, or file_url — not multiple.",
            },
          ],
          isError: true,
        };
      }

      if (file_path) {
        // Local file
        fileBuffer = new Uint8Array(await readFile(file_path));
        fileName = file_name ?? basename(file_path);
      } else if (file_base64) {
        // Base64-encoded content
        if (!file_name) {
          return {
            content: [
              {
                type: "text",
                text: "Error: file_name is required when using file_base64.",
              },
            ],
            isError: true,
          };
        }
        fileBuffer = new Uint8Array(Buffer.from(file_base64, "base64"));
        fileName = file_name;
      } else if (file_url) {
        // Download from URL
        const urlRes = await fetch(file_url);
        if (!urlRes.ok) {
          return {
            content: [
              {
                type: "text",
                text: `Error: Failed to download file from URL (${urlRes.status}): ${await urlRes.text().catch(() => "")}`,
              },
            ],
            isError: true,
          };
        }
        fileBuffer = new Uint8Array(await urlRes.arrayBuffer());
        // Derive filename from URL path or use provided name
        fileName =
          file_name ??
          new URL(file_url).pathname.split("/").pop() ??
          "document.pdf";
      } else {
        // Unreachable, but satisfy TypeScript
        return {
          content: [{ type: "text", text: "Error: No file source provided." }],
          isError: true,
        };
      }

      const form = new FormData();
      form.append(
        "file",
        new Blob([fileBuffer as BlobPart], { type: "application/pdf" }),
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

  server.tool(
    "get_extraction_snapshot",
    "Get the full extraction tree for an extraction ID. Returns hierarchical structure with summaries, readable_id, structure_map, relationships, reference_index (entity cross-references like CPFs, CNPJs, process numbers), metadata, and content index — but no raw content blobs. Use get_content to lazy-load actual text.",
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

  // -------------------------------------------------------------------------
  // Sheet / Dataset tools
  // -------------------------------------------------------------------------

  server.tool(
    "extract_sheet",
    `Upload a file (CSV, Excel, or PDF) and extract structured tabular data with LLM-discovered schemas. Provide the file via exactly ONE of:
- file_path: local filesystem path (for STDIO/local usage)
- file_base64: base64-encoded file content (for remote HTTP usage)
- file_url: URL to download the file from (for remote HTTP usage)
Returns the dataset placeholder JSON (id + processing status). Poll list_datasets or get_dataset to check completion.`,
    {
      file_path: z
        .string()
        .optional()
        .describe("Absolute path to the file on disk (CSV, Excel, or PDF)"),
      file_base64: z
        .string()
        .optional()
        .describe(
          "Base64-encoded file content. Must also provide file_name.",
        ),
      file_url: z
        .string()
        .url()
        .optional()
        .describe("URL to download the file from."),
      file_name: z
        .string()
        .optional()
        .describe(
          "Filename (required with file_base64, optional otherwise). Example: 'data.csv', 'report.xlsx'",
        ),
      config: z
        .string()
        .optional()
        .default("financial_br")
        .describe("Extraction config name"),
      upload: z
        .boolean()
        .optional()
        .default(true)
        .describe("Whether to persist the dataset to Supabase"),
    },
    async ({ file_path, file_base64, file_url, file_name, config, upload }) => {
      let fileBuffer: Uint8Array;
      let fileName: string;

      const sourceCount = [file_path, file_base64, file_url].filter(Boolean).length;
      if (sourceCount === 0) {
        return {
          content: [
            {
              type: "text",
              text: "Error: Provide exactly one of file_path, file_base64, or file_url.",
            },
          ],
          isError: true,
        };
      }
      if (sourceCount > 1) {
        return {
          content: [
            {
              type: "text",
              text: "Error: Provide only ONE of file_path, file_base64, or file_url — not multiple.",
            },
          ],
          isError: true,
        };
      }

      if (file_path) {
        fileBuffer = new Uint8Array(await readFile(file_path));
        fileName = file_name ?? basename(file_path);
      } else if (file_base64) {
        if (!file_name) {
          return {
            content: [
              {
                type: "text",
                text: "Error: file_name is required when using file_base64.",
              },
            ],
            isError: true,
          };
        }
        fileBuffer = new Uint8Array(Buffer.from(file_base64, "base64"));
        fileName = file_name;
      } else if (file_url) {
        const urlRes = await fetch(file_url);
        if (!urlRes.ok) {
          return {
            content: [
              {
                type: "text",
                text: `Error: Failed to download file from URL (${urlRes.status}): ${await urlRes.text().catch(() => "")}`,
              },
            ],
            isError: true,
          };
        }
        fileBuffer = new Uint8Array(await urlRes.arrayBuffer());
        fileName =
          file_name ??
          new URL(file_url).pathname.split("/").pop() ??
          "data.csv";
      } else {
        return {
          content: [{ type: "text", text: "Error: No file source provided." }],
          isError: true,
        };
      }

      // Detect MIME type from extension
      const ext = fileName.split(".").pop()?.toLowerCase() ?? "";
      const mimeMap: Record<string, string> = {
        csv: "text/csv",
        xlsx: "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        xlsm: "application/vnd.ms-excel.sheet.macroEnabled.12",
        xlsb: "application/vnd.ms-excel.sheet.binary.macroEnabled.12",
        xls: "application/vnd.ms-excel",
        pdf: "application/pdf",
      };
      const mimeType = mimeMap[ext] ?? "application/octet-stream";

      const form = new FormData();
      form.append(
        "file",
        new Blob([fileBuffer as BlobPart], { type: mimeType }),
        fileName,
      );

      const params = new URLSearchParams();
      if (config) params.set("config", config);
      if (upload !== undefined) params.set("upload", String(upload));

      const result = await api(`/extract-sheet?${params.toString()}`, {
        method: "POST",
        body: form,
      });

      return {
        content: [{ type: "text", text: JSON.stringify(result, null, 2) }],
      };
    },
  );

  // -------------------------------------------------------------------------
  // Config management tools
  // -------------------------------------------------------------------------

  server.tool(
    "create_config",
    "Create a new extraction config. Provide the full ExtractionConfig JSON object.",
    {
      config: z
        .object({
          name: z.string().describe("Unique config name (e.g. 'legal_br')"),
          description: z.string().describe("Human-readable description"),
          prompts: z
            .object({
              structure: z.string().describe("System prompt for document structure extraction"),
              metadata: z.string().optional().describe("Optional metadata extraction prompt"),
              summary: z.string().optional().describe("Optional summary generation prompt"),
            })
            .describe("LLM prompts"),
          node_types: z.array(z.any()).optional().describe("Node type definitions"),
          relationship_types: z.array(z.string()).optional().describe("Relationship type names"),
          metadata_schema: z.any().optional().describe("JSON schema for metadata"),
          entity_patterns: z.array(z.any()).optional().describe("Regex-based entity patterns"),
          readable_id_hint: z.string().optional().describe("Hint for extracting readable document ID"),
          sheet_config: z.any().optional().describe("Sheet extraction config"),
        })
        .describe("Full ExtractionConfig JSON object"),
    },
    async ({ config }) => {
      const result = await api("/configs", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(config),
      });
      return {
        content: [{ type: "text", text: JSON.stringify(result, null, 2) }],
      };
    },
  );

  server.tool(
    "update_config",
    "Update an existing extraction config. Provide the config name and full ExtractionConfig JSON object.",
    {
      name: z.string().describe("Config name to update"),
      config: z
        .object({
          name: z.string().describe("Config name (must match the name parameter)"),
          description: z.string().describe("Human-readable description"),
          prompts: z
            .object({
              structure: z.string().describe("System prompt for document structure extraction"),
              metadata: z.string().optional().describe("Optional metadata extraction prompt"),
              summary: z.string().optional().describe("Optional summary generation prompt"),
            })
            .describe("LLM prompts"),
          node_types: z.array(z.any()).optional().describe("Node type definitions"),
          relationship_types: z.array(z.string()).optional().describe("Relationship type names"),
          metadata_schema: z.any().optional().describe("JSON schema for metadata"),
          entity_patterns: z.array(z.any()).optional().describe("Regex-based entity patterns"),
          readable_id_hint: z.string().optional().describe("Hint for extracting readable document ID"),
          sheet_config: z.any().optional().describe("Sheet extraction config"),
        })
        .describe("Full ExtractionConfig JSON object"),
    },
    async ({ name, config }) => {
      const result = await api(`/configs/${encodeURIComponent(name)}`, {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(config),
      });
      return {
        content: [{ type: "text", text: JSON.stringify(result, null, 2) }],
      };
    },
  );

  server.tool(
    "delete_config",
    "Delete an extraction config by name.",
    {
      name: z.string().describe("Config name to delete"),
    },
    async ({ name }) => {
      await api(`/configs/${encodeURIComponent(name)}`, { method: "DELETE" });
      return {
        content: [{ type: "text", text: `Config '${name}' deleted successfully.` }],
      };
    },
  );

  server.tool(
    "list_datasets",
    "List all datasets with their IDs, source files, summaries, schema counts, and row counts. Datasets are created by extract_sheet from CSV, Excel, or PDF files.",
    {},
    async () => {
      const datasets = await api("/datasets");
      return {
        content: [{ type: "text", text: JSON.stringify(datasets, null, 2) }],
      };
    },
  );

  server.tool(
    "get_dataset",
    "Get a complete dataset by ID, including all discovered schemas, column definitions, and typed rows. Use this to inspect the full extraction result from extract_sheet.",
    {
      dataset_id: z.string().describe("The dataset ID (e.g. ds_abc123...)"),
    },
    async ({ dataset_id }) => {
      const result = await api(`/datasets/${dataset_id}`);
      return {
        content: [{ type: "text", text: JSON.stringify(result, null, 2) }],
      };
    },
  );

  server.tool(
    "query_dataset_rows",
    "Query rows from a specific schema within a dataset. Use for paginated access to large datasets. Returns just the row data (no column definitions).",
    {
      dataset_id: z.string().describe("The dataset ID"),
      schema_name: z
        .string()
        .describe("The schema name within the dataset (e.g. 'card_transactions')"),
      offset: z
        .number()
        .int()
        .min(0)
        .optional()
        .default(0)
        .describe("Row offset to start from"),
      limit: z
        .number()
        .int()
        .min(1)
        .optional()
        .default(100)
        .describe("Maximum rows to return"),
    },
    async ({ dataset_id, schema_name, offset, limit }) => {
      const params = new URLSearchParams();
      params.set("schema_name", schema_name);
      if (offset !== undefined) params.set("offset", String(offset));
      if (limit !== undefined) params.set("limit", String(limit));

      const result = await api(`/datasets/${dataset_id}/rows?${params.toString()}`);
      return {
        content: [{ type: "text", text: JSON.stringify(result, null, 2) }],
      };
    },
  );
}

// ---------------------------------------------------------------------------
// Start
// ---------------------------------------------------------------------------

function createMcpServer(): McpServer {
  const server = new McpServer({
    name: "generic-extractor",
    version: "0.1.0",
  });
  registerTools(server);
  return server;
}

async function main() {
  if (TRANSPORT === "http") {
    await startHttp();
  } else {
    const server = createMcpServer();
    const transport = new StdioServerTransport();
    await server.connect(transport);
  }
}

async function startHttp() {
  // Track sessions: sessionId → transport
  const sessions = new Map<string, StreamableHTTPServerTransport>();

  async function readBody(req: IncomingMessage): Promise<unknown> {
    const chunks: Buffer[] = [];
    for await (const chunk of req) {
      chunks.push(chunk as Buffer);
    }
    return JSON.parse(Buffer.concat(chunks).toString());
  }

  function isInitializeRequest(body: unknown): boolean {
    if (Array.isArray(body)) {
      return body.some(
        (msg) => typeof msg === "object" && msg !== null && "method" in msg && msg.method === "initialize",
      );
    }
    return typeof body === "object" && body !== null && "method" in body && (body as { method: string }).method === "initialize";
  }

  const httpServer = createServer(async (req: IncomingMessage, res: ServerResponse) => {
    const url = req.url ?? "";

    // Health check
    if (url === "/health" && req.method === "GET") {
      res.writeHead(200, { "Content-Type": "application/json" });
      res.end(JSON.stringify({ status: "ok" }));
      return;
    }

    // MCP endpoint
    if (url === "/mcp" || url.startsWith("/mcp?")) {
      // GET — SSE stream for an existing session
      if (req.method === "GET" || req.method === "DELETE") {
        const sessionId = req.headers["mcp-session-id"] as string | undefined;
        if (sessionId && sessions.has(sessionId)) {
          await sessions.get(sessionId)!.handleRequest(req, res);
        } else {
          res.writeHead(400, { "Content-Type": "application/json" });
          res.end(JSON.stringify({ error: "Invalid or missing session" }));
        }
        return;
      }

      // POST — either initialize or message to existing session
      if (req.method === "POST") {
        const body = await readBody(req);
        const sessionId = req.headers["mcp-session-id"] as string | undefined;

        // Existing session
        if (sessionId && sessions.has(sessionId)) {
          await sessions.get(sessionId)!.handleRequest(req, res, body);
          return;
        }

        // New session — must be an initialize request
        if (isInitializeRequest(body)) {
          const transport = new StreamableHTTPServerTransport({
            sessionIdGenerator: () => randomUUID(),
            onsessioninitialized: (id) => {
              sessions.set(id, transport);
            },
          });

          transport.onclose = () => {
            if (transport.sessionId) {
              sessions.delete(transport.sessionId);
            }
          };

          const server = createMcpServer();
          await server.connect(transport);
          await transport.handleRequest(req, res, body);
          return;
        }

        res.writeHead(400, { "Content-Type": "application/json" });
        res.end(JSON.stringify({ jsonrpc: "2.0", error: { code: -32600, message: "Bad Request: No session ID and not an initialize request" }, id: null }));
        return;
      }

      res.writeHead(405);
      res.end("Method not allowed");
      return;
    }

    res.writeHead(404);
    res.end("Not found");
  });

  httpServer.listen(MCP_PORT, "127.0.0.1", () => {
    console.error(`MCP HTTP server listening on http://127.0.0.1:${MCP_PORT}/mcp`);
  });
}

main().catch((err) => {
  console.error("Fatal error:", err);
  process.exit(1);
});
