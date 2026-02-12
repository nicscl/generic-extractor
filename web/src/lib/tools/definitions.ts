// 13 tools in OpenAI function-calling format, derived from MCP tool definitions

export interface ToolDefinition {
  type: "function";
  function: {
    name: string;
    description: string;
    parameters: {
      type: "object";
      properties: Record<string, unknown>;
      required?: string[];
    };
  };
}

export const toolDefinitions: ToolDefinition[] = [
  {
    type: "function",
    function: {
      name: "list_configs",
      description:
        "List available extraction configuration names (e.g. 'legal_br')",
      parameters: { type: "object", properties: {} },
    },
  },
  {
    type: "function",
    function: {
      name: "list_extractions",
      description:
        "List all extractions with IDs, source files, readable_id, summaries, and page counts. Supports filtering by readable_id (case-insensitive substring match).",
      parameters: {
        type: "object",
        properties: {
          readable_id: {
            type: "string",
            description:
              "Filter by readable_id (case-insensitive substring match). Example: '0266175'",
          },
        },
      },
    },
  },
  {
    type: "function",
    function: {
      name: "extract_document",
      description:
        "Upload a PDF and run the extraction pipeline. Provide the file via file_base64 (with file_name) or file_url. Returns the full extraction result.",
      parameters: {
        type: "object",
        properties: {
          file_base64: {
            type: "string",
            description: "Base64-encoded PDF content. Must also provide file_name.",
          },
          file_url: {
            type: "string",
            description: "URL to download the PDF from.",
          },
          file_name: {
            type: "string",
            description:
              "Filename for the PDF (required with file_base64). Example: 'document.pdf'",
          },
          config: {
            type: "string",
            description: "Extraction config name. Default: 'legal_br'",
          },
          upload: {
            type: "boolean",
            description: "Persist to Supabase. Default: true",
          },
        },
      },
    },
  },
  {
    type: "function",
    function: {
      name: "get_extraction_snapshot",
      description:
        "Get the full extraction tree for an extraction ID. Returns hierarchical structure with summaries, structure_map, relationships, reference_index, and content index — no raw content.",
      parameters: {
        type: "object",
        properties: {
          extraction_id: {
            type: "string",
            description: "The extraction ID (e.g. ext_abc123...)",
          },
        },
        required: ["extraction_id"],
      },
    },
  },
  {
    type: "function",
    function: {
      name: "get_node",
      description:
        "Get a specific node from an extraction by its node ID. Returns type, label, summary, page range, content_ref, and children.",
      parameters: {
        type: "object",
        properties: {
          extraction_id: { type: "string", description: "The extraction ID" },
          node_id: { type: "string", description: "The node ID within the extraction" },
        },
        required: ["extraction_id", "node_id"],
      },
    },
  },
  {
    type: "function",
    function: {
      name: "get_content",
      description:
        "Lazy-load text content for a node via its content:// reference. Supports pagination with offset and limit.",
      parameters: {
        type: "object",
        properties: {
          ref: {
            type: "string",
            description:
              "The content reference (e.g. 'content://node_abc123' or just 'node_abc123')",
          },
          offset: { type: "number", description: "Character offset. Default: 0" },
          limit: { type: "number", description: "Max characters. Default: 4000" },
        },
        required: ["ref"],
      },
    },
  },
  {
    type: "function",
    function: {
      name: "extract_sheet",
      description:
        "Upload a file (CSV, Excel, or PDF) and extract structured tabular data. Provide via file_base64 (with file_name) or file_url. Returns dataset placeholder (id + processing status).",
      parameters: {
        type: "object",
        properties: {
          file_base64: {
            type: "string",
            description: "Base64-encoded file content. Must also provide file_name.",
          },
          file_url: {
            type: "string",
            description: "URL to download the file from.",
          },
          file_name: {
            type: "string",
            description:
              "Filename (required with file_base64). Example: 'data.csv', 'report.xlsx'",
          },
          config: {
            type: "string",
            description: "Extraction config name. Default: 'financial_br'",
          },
          upload: {
            type: "boolean",
            description: "Persist to Supabase. Default: true",
          },
        },
      },
    },
  },
  {
    type: "function",
    function: {
      name: "list_datasets",
      description:
        "List all datasets with IDs, source files, summaries, schema counts, and row counts.",
      parameters: { type: "object", properties: {} },
    },
  },
  {
    type: "function",
    function: {
      name: "get_dataset",
      description:
        "Get a complete dataset by ID, including all schemas, column definitions, and typed rows.",
      parameters: {
        type: "object",
        properties: {
          dataset_id: {
            type: "string",
            description: "The dataset ID (e.g. ds_abc123...)",
          },
        },
        required: ["dataset_id"],
      },
    },
  },
  {
    type: "function",
    function: {
      name: "query_dataset_rows",
      description:
        "Query rows from a specific schema within a dataset. Paginated access.",
      parameters: {
        type: "object",
        properties: {
          dataset_id: { type: "string", description: "The dataset ID" },
          schema_name: {
            type: "string",
            description: "Schema name (e.g. 'card_transactions')",
          },
          offset: { type: "number", description: "Row offset. Default: 0" },
          limit: { type: "number", description: "Max rows. Default: 100" },
        },
        required: ["dataset_id", "schema_name"],
      },
    },
  },
  {
    type: "function",
    function: {
      name: "create_config",
      description:
        "Create a new extraction config. Provide the full ExtractionConfig JSON object.",
      parameters: {
        type: "object",
        properties: {
          config: {
            type: "object",
            description: "Full ExtractionConfig JSON object with name, description, prompts, etc.",
          },
        },
        required: ["config"],
      },
    },
  },
  {
    type: "function",
    function: {
      name: "update_config",
      description:
        "Update an existing extraction config. Provide the config name and full ExtractionConfig JSON object.",
      parameters: {
        type: "object",
        properties: {
          name: { type: "string", description: "Config name to update" },
          config: {
            type: "object",
            description: "Full ExtractionConfig JSON object (name must match)",
          },
        },
        required: ["name", "config"],
      },
    },
  },
  {
    type: "function",
    function: {
      name: "delete_config",
      description: "Delete an extraction config by name.",
      parameters: {
        type: "object",
        properties: {
          name: { type: "string", description: "Config name to delete" },
        },
        required: ["name"],
      },
    },
  },
];
