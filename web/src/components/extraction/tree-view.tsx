"use client";

import { useState } from "react";
import { ChevronDown, ChevronRight, FileText } from "lucide-react";
import clsx from "clsx";
import type { ExtractionNode } from "@/lib/types";

interface TreeViewProps {
  node: ExtractionNode;
  selectedId: string | null;
  onSelect: (node: ExtractionNode) => void;
  depth?: number;
}

export function TreeView({
  node,
  selectedId,
  onSelect,
  depth = 0,
}: TreeViewProps) {
  const [expanded, setExpanded] = useState(depth < 2);
  const hasChildren = node.children && node.children.length > 0;
  const isSelected = selectedId === node.id;

  return (
    <div>
      <button
        onClick={() => {
          onSelect(node);
          if (hasChildren) setExpanded(!expanded);
        }}
        className={clsx(
          "flex w-full items-center gap-1.5 rounded-lg px-2 py-1.5 text-left text-sm transition-colors",
          isSelected
            ? "bg-blue-600/20 text-blue-300"
            : "text-zinc-300 hover:bg-zinc-800"
        )}
        style={{ paddingLeft: `${depth * 16 + 8}px` }}
      >
        {hasChildren ? (
          expanded ? (
            <ChevronDown className="h-3.5 w-3.5 shrink-0 text-zinc-500" />
          ) : (
            <ChevronRight className="h-3.5 w-3.5 shrink-0 text-zinc-500" />
          )
        ) : (
          <FileText className="h-3.5 w-3.5 shrink-0 text-zinc-600" />
        )}
        <span className="mr-2 rounded bg-zinc-800 px-1.5 py-0.5 font-mono text-[10px] text-zinc-500">
          {node.node_type}
        </span>
        <span className="truncate">{node.label}</span>
        {node.page_range && (
          <span className="ml-auto shrink-0 text-[10px] text-zinc-600">
            p.{node.page_range}
          </span>
        )}
      </button>

      {expanded && hasChildren && (
        <div>
          {node.children.map((child) => (
            <TreeView
              key={child.id}
              node={child}
              selectedId={selectedId}
              onSelect={onSelect}
              depth={depth + 1}
            />
          ))}
        </div>
      )}
    </div>
  );
}
