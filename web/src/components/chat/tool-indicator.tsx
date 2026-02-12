"use client";

import { useState } from "react";
import { Wrench, ChevronDown, ChevronRight, Loader2 } from "lucide-react";

interface ToolCallInfo {
  name: string;
  args: Record<string, unknown>;
  result?: string;
}

export function ToolIndicator({ toolCall }: { toolCall: ToolCallInfo }) {
  const [expanded, setExpanded] = useState(false);
  const isComplete = toolCall.result !== undefined;

  return (
    <div className="rounded-lg border border-zinc-700 bg-zinc-800/50 text-sm">
      <button
        onClick={() => setExpanded(!expanded)}
        className="flex w-full items-center gap-2 px-3 py-2 text-left hover:bg-zinc-800"
      >
        {isComplete ? (
          <Wrench className="h-3.5 w-3.5 text-green-400" />
        ) : (
          <Loader2 className="h-3.5 w-3.5 animate-spin text-yellow-400" />
        )}
        <span className="font-mono text-xs text-zinc-300">{toolCall.name}</span>
        <span className="ml-auto">
          {expanded ? (
            <ChevronDown className="h-3.5 w-3.5 text-zinc-500" />
          ) : (
            <ChevronRight className="h-3.5 w-3.5 text-zinc-500" />
          )}
        </span>
      </button>

      {expanded && (
        <div className="border-t border-zinc-700 px-3 py-2">
          <div className="mb-2">
            <span className="text-xs font-medium text-zinc-500">Arguments</span>
            <pre className="mt-1 max-h-32 overflow-auto rounded bg-zinc-900 p-2 text-xs text-zinc-400">
              {JSON.stringify(toolCall.args, null, 2)}
            </pre>
          </div>
          {toolCall.result && (
            <div>
              <span className="text-xs font-medium text-zinc-500">Result</span>
              <pre className="mt-1 max-h-48 overflow-auto rounded bg-zinc-900 p-2 text-xs text-zinc-400">
                {toolCall.result.length > 2000
                  ? toolCall.result.slice(0, 2000) + "\n...[truncated in UI]"
                  : toolCall.result}
              </pre>
            </div>
          )}
        </div>
      )}
    </div>
  );
}
