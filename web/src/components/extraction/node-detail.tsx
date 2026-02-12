"use client";

import type { ExtractionNode } from "@/lib/types";
import { Markdown } from "@/components/ui/markdown";

interface NodeDetailProps {
  node: ExtractionNode;
  content: string | null;
  onLoadContent: (ref: string) => void;
}

export function NodeDetail({ node, content, onLoadContent }: NodeDetailProps) {
  return (
    <div className="space-y-4">
      {/* Header */}
      <div>
        <div className="flex items-center gap-2">
          <span className="rounded bg-blue-600/20 px-2 py-0.5 text-xs font-medium text-blue-400">
            {node.node_type}
          </span>
          {node.page_range && (
            <span className="text-xs text-zinc-500">Pages {node.page_range}</span>
          )}
        </div>
        <h2 className="mt-2 text-lg font-medium text-white">{node.label}</h2>
      </div>

      {/* Summary */}
      {node.summary && (
        <div>
          <h3 className="mb-1 text-sm font-medium text-zinc-400">Summary</h3>
          <div className="rounded-lg bg-zinc-800 p-4">
            <Markdown content={node.summary} />
          </div>
        </div>
      )}

      {/* Metadata */}
      {node.metadata && Object.keys(node.metadata).length > 0 && (
        <div>
          <h3 className="mb-1 text-sm font-medium text-zinc-400">Metadata</h3>
          <pre className="max-h-48 overflow-auto rounded-lg bg-zinc-800 p-4 text-xs text-zinc-300">
            {JSON.stringify(node.metadata, null, 2)}
          </pre>
        </div>
      )}

      {/* Content */}
      {node.content_ref && (
        <div>
          <div className="mb-1 flex items-center justify-between">
            <h3 className="text-sm font-medium text-zinc-400">Content</h3>
            {content === null && (
              <button
                onClick={() => onLoadContent(node.content_ref!)}
                className="rounded-lg bg-zinc-800 px-3 py-1 text-xs text-blue-400 hover:bg-zinc-700"
              >
                Load content
              </button>
            )}
          </div>
          {content !== null && (
            <div className="rounded-lg bg-zinc-800 p-4">
              <Markdown content={content} />
            </div>
          )}
        </div>
      )}
    </div>
  );
}
