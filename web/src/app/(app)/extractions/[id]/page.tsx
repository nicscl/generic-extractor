"use client";

import { useEffect } from "react";
import { useParams } from "next/navigation";
import Link from "next/link";
import { ArrowLeft, Loader2 } from "lucide-react";
import { useExtraction } from "@/hooks/use-extraction";
import { TreeView } from "@/components/extraction/tree-view";
import { NodeDetail } from "@/components/extraction/node-detail";

export default function ExtractionPage() {
  const { id } = useParams<{ id: string }>();
  const {
    snapshot,
    selectedNode,
    nodeContent,
    loading,
    error,
    loadSnapshot,
    selectNode,
    loadContent,
  } = useExtraction(id);

  useEffect(() => {
    loadSnapshot();
  }, [loadSnapshot]);

  if (loading && !snapshot) {
    return (
      <div className="flex h-full items-center justify-center">
        <Loader2 className="h-6 w-6 animate-spin text-zinc-400" />
      </div>
    );
  }

  if (error) {
    return (
      <div className="p-8">
        <p className="text-red-400">Error: {error}</p>
      </div>
    );
  }

  if (!snapshot) return null;

  return (
    <div className="flex h-full flex-col">
      {/* Header */}
      <div className="flex items-center gap-3 border-b border-zinc-800 px-6 py-3">
        <Link
          href="/"
          className="rounded p-1 text-zinc-400 hover:bg-zinc-800 hover:text-white"
        >
          <ArrowLeft className="h-4 w-4" />
        </Link>
        <div>
          <h1 className="font-medium text-white">
            {snapshot.readable_id ?? snapshot.source_file}
          </h1>
          <p className="text-xs text-zinc-500">{snapshot.id}</p>
        </div>
      </div>

      {/* Body */}
      <div className="flex flex-1 overflow-hidden">
        {/* Tree */}
        <div className="w-80 shrink-0 overflow-y-auto border-r border-zinc-800 p-3">
          <TreeView
            node={snapshot.tree}
            selectedId={selectedNode?.id ?? null}
            onSelect={selectNode}
          />
        </div>

        {/* Detail */}
        <div className="flex-1 overflow-y-auto p-6">
          {selectedNode ? (
            <NodeDetail
              node={selectedNode}
              content={nodeContent}
              onLoadContent={loadContent}
            />
          ) : (
            <div className="flex h-full items-center justify-center text-zinc-500">
              <p>Select a node from the tree to view details</p>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
