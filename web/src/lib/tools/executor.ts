const API_URL = process.env.EXTRACTOR_API_URL ?? "http://localhost:3002";
const MAX_RESULT_CHARS = 30_000;

async function api(path: string, init?: RequestInit): Promise<unknown> {
  const res = await fetch(`${API_URL}${path}`, init);
  if (!res.ok) {
    const body = await res.text().catch(() => "");
    throw new Error(`Extractor API ${res.status}: ${body}`);
  }
  if (res.status === 204) return null;
  return res.json();
}

function truncate(result: unknown): string {
  const json = JSON.stringify(result, null, 2);
  if (json.length <= MAX_RESULT_CHARS) return json;
  return (
    json.slice(0, MAX_RESULT_CHARS) +
    `\n\n[Truncated: result was ${json.length} chars, showing first ${MAX_RESULT_CHARS}]`
  );
}

export async function executeTool(
  name: string,
  args: Record<string, unknown>
): Promise<string> {
  try {
    switch (name) {
      case "list_configs": {
        const result = await api("/configs");
        return truncate(result);
      }

      case "list_extractions": {
        const params = new URLSearchParams();
        if (args.readable_id) params.set("readable_id", String(args.readable_id));
        const qs = params.toString();
        const result = await api(`/extractions${qs ? `?${qs}` : ""}`);
        return truncate(result);
      }

      case "extract_document": {
        if (args.file_url) {
          // URL-based extraction
          const params = new URLSearchParams();
          params.set("file_url", String(args.file_url));
          if (args.config) params.set("config", String(args.config));
          if (args.upload !== undefined) params.set("upload", String(args.upload));
          const result = await api(`/extract?${params.toString()}`, { method: "POST" });
          return truncate(result);
        }
        if (args.file_base64) {
          // Base64 upload via multipart
          const buffer = Buffer.from(String(args.file_base64), "base64");
          const fileName = String(args.file_name ?? "document.pdf");
          const form = new FormData();
          form.append("file", new Blob([buffer], { type: "application/pdf" }), fileName);
          const params = new URLSearchParams();
          if (args.config) params.set("config", String(args.config));
          if (args.upload !== undefined) params.set("upload", String(args.upload));
          const result = await api(`/extract?${params.toString()}`, {
            method: "POST",
            body: form,
          });
          return truncate(result);
        }
        return JSON.stringify({ error: "Provide file_base64 or file_url" });
      }

      case "get_extraction_snapshot": {
        const result = await api(`/extractions/${args.extraction_id}/snapshot`);
        return truncate(result);
      }

      case "get_node": {
        const result = await api(
          `/extractions/${args.extraction_id}/node/${args.node_id}`
        );
        return truncate(result);
      }

      case "get_content": {
        const refPath = String(args.ref).replace(/^content:\/\//, "");
        const params = new URLSearchParams();
        if (args.offset !== undefined) params.set("offset", String(args.offset));
        if (args.limit !== undefined) params.set("limit", String(args.limit));
        const qs = params.toString();
        const result = await api(`/content/${refPath}${qs ? `?${qs}` : ""}`);
        return truncate(result);
      }

      case "extract_sheet": {
        if (args.file_url) {
          const params = new URLSearchParams();
          params.set("file_url", String(args.file_url));
          if (args.config) params.set("config", String(args.config));
          if (args.upload !== undefined) params.set("upload", String(args.upload));
          const result = await api(`/extract-sheet?${params.toString()}`, {
            method: "POST",
          });
          return truncate(result);
        }
        if (args.file_base64) {
          const buffer = Buffer.from(String(args.file_base64), "base64");
          const fileName = String(args.file_name ?? "data.csv");
          const ext = fileName.split(".").pop()?.toLowerCase() ?? "";
          const mimeMap: Record<string, string> = {
            csv: "text/csv",
            xlsx: "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            xls: "application/vnd.ms-excel",
            pdf: "application/pdf",
          };
          const form = new FormData();
          form.append(
            "file",
            new Blob([buffer], { type: mimeMap[ext] ?? "application/octet-stream" }),
            fileName
          );
          const params = new URLSearchParams();
          if (args.config) params.set("config", String(args.config));
          if (args.upload !== undefined) params.set("upload", String(args.upload));
          const result = await api(`/extract-sheet?${params.toString()}`, {
            method: "POST",
            body: form,
          });
          return truncate(result);
        }
        return JSON.stringify({ error: "Provide file_base64 or file_url" });
      }

      case "list_datasets": {
        const result = await api("/datasets");
        return truncate(result);
      }

      case "get_dataset": {
        const result = await api(`/datasets/${args.dataset_id}`);
        return truncate(result);
      }

      case "query_dataset_rows": {
        const params = new URLSearchParams();
        params.set("schema_name", String(args.schema_name));
        if (args.offset !== undefined) params.set("offset", String(args.offset));
        if (args.limit !== undefined) params.set("limit", String(args.limit));
        const result = await api(`/datasets/${args.dataset_id}/rows?${params.toString()}`);
        return truncate(result);
      }

      case "create_config": {
        const result = await api("/configs", {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify(args.config),
        });
        return truncate(result);
      }

      case "update_config": {
        const result = await api(`/configs/${encodeURIComponent(String(args.name))}`, {
          method: "PUT",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify(args.config),
        });
        return truncate(result);
      }

      case "delete_config": {
        await api(`/configs/${encodeURIComponent(String(args.name))}`, {
          method: "DELETE",
        });
        return JSON.stringify({ success: true, message: `Config '${args.name}' deleted` });
      }

      default:
        return JSON.stringify({ error: `Unknown tool: ${name}` });
    }
  } catch (err) {
    return JSON.stringify({
      error: err instanceof Error ? err.message : String(err),
    });
  }
}
