// ---- Projects ----
export interface Project {
  id: string;
  user_id: string;
  name: string;
  description: string;
  created_at: string;
  updated_at: string;
  extraction_count?: number;
  dataset_count?: number;
}

// ---- Chat ----
export interface ChatMessage {
  id: string;
  project_id: string;
  role: "user" | "assistant" | "system" | "tool";
  content: string;
  tool_calls?: ToolCall[] | null;
  tool_call_id?: string | null;
  tool_name?: string | null;
  created_at: string;
}

export interface ToolCall {
  id: string;
  type: "function";
  function: {
    name: string;
    arguments: string;
  };
}

// ---- Extractions ----
export interface ExtractionSummary {
  id: string;
  source_file: string;
  readable_id?: string;
  summary: string;
  page_count: number;
  created_at: string;
  project_id?: string | null;
}

export interface ExtractionSnapshot {
  id: string;
  source_file: string;
  readable_id?: string;
  summary: string;
  metadata: Record<string, unknown>;
  reference_index: Record<string, unknown>;
  tree: ExtractionNode;
  structure_map: Record<string, { label: string; type: string; page_range?: string }>;
  relationships: ExtractionRelationship[];
  content_index: Record<string, { chars: number; content_ref: string }>;
}

export interface ExtractionNode {
  id: string;
  node_type: string;
  label: string;
  summary: string;
  page_range?: string;
  content_ref?: string;
  metadata?: Record<string, unknown>;
  children: ExtractionNode[];
}

export interface ExtractionRelationship {
  source_id: string;
  target_id: string;
  rel_type: string;
  description?: string;
}

// ---- Datasets ----
export interface DatasetSummary {
  id: string;
  source_file: string;
  summary: string;
  status: "processing" | "completed" | "failed";
  schema_count: number;
  row_count: number;
  created_at: string;
  project_id?: string | null;
}

export interface Dataset {
  id: string;
  source_file: string;
  summary: string;
  status: string;
  schemas: DataSchema[];
  created_at: string;
}

export interface DataSchema {
  name: string;
  description: string;
  columns: ColumnDef[];
  rows: Record<string, unknown>[];
  row_count: number;
}

export interface ColumnDef {
  name: string;
  data_type: string;
  format?: string;
  transform?: string;
  required: boolean;
}

// ---- SSE events from /api/chat ----
export type ChatSSEEvent =
  | { type: "status"; message: string }
  | { type: "tool_call"; tool_name: string; arguments: Record<string, unknown> }
  | { type: "tool_result"; tool_name: string; result: string }
  | { type: "message"; content: string }
  | { type: "error"; message: string }
  | { type: "done"; messages: ChatMessage[] };
