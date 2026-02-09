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
