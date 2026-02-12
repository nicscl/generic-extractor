"use client";

import { useEffect } from "react";
import { useParams } from "next/navigation";
import Link from "next/link";
import { ArrowLeft, Loader2 } from "lucide-react";
import { useDataset } from "@/hooks/use-dataset";
import { DataTable } from "@/components/dataset/data-table";
import clsx from "clsx";

export default function DatasetPage() {
  const { id } = useParams<{ id: string }>();
  const {
    dataset,
    selectedSchema,
    rows,
    totalRows,
    offset,
    limit,
    loading,
    error,
    loadDataset,
    selectSchema,
    loadPage,
  } = useDataset(id);

  useEffect(() => {
    loadDataset();
  }, [loadDataset]);

  if (loading && !dataset) {
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

  if (!dataset) return null;

  const currentSchema = dataset.schemas.find((s) => s.name === selectedSchema);

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
          <h1 className="font-medium text-white">{dataset.source_file}</h1>
          <p className="text-xs text-zinc-500">{dataset.id}</p>
        </div>
        <span
          className={clsx(
            "ml-2 rounded-full px-2 py-0.5 text-xs",
            dataset.status === "completed"
              ? "bg-green-900/50 text-green-400"
              : dataset.status === "processing"
              ? "bg-yellow-900/50 text-yellow-400"
              : "bg-red-900/50 text-red-400"
          )}
        >
          {dataset.status}
        </span>
      </div>

      {/* Summary */}
      {dataset.summary && (
        <div className="border-b border-zinc-800 px-6 py-3">
          <p className="text-sm text-zinc-400">{dataset.summary}</p>
        </div>
      )}

      {/* Schema tabs */}
      {dataset.schemas.length > 1 && (
        <div className="flex gap-1 border-b border-zinc-800 px-6 pt-3">
          {dataset.schemas.map((s) => (
            <button
              key={s.name}
              onClick={() => selectSchema(s.name)}
              className={clsx(
                "rounded-t-lg px-4 py-2 text-sm font-medium transition-colors",
                selectedSchema === s.name
                  ? "border-b-2 border-blue-500 bg-zinc-800 text-white"
                  : "text-zinc-400 hover:bg-zinc-800/50 hover:text-white"
              )}
            >
              {s.name}
              <span className="ml-1.5 text-xs text-zinc-500">
                ({s.row_count})
              </span>
            </button>
          ))}
        </div>
      )}

      {/* Schema description + columns info */}
      {currentSchema && (
        <div className="flex-1 overflow-y-auto p-6">
          {currentSchema.description && (
            <p className="mb-4 text-sm text-zinc-400">
              {currentSchema.description}
            </p>
          )}

          <DataTable
            columns={currentSchema.columns}
            rows={rows}
            totalRows={totalRows}
            offset={offset}
            limit={limit}
            onPageChange={loadPage}
            loading={loading}
          />
        </div>
      )}

      {!currentSchema && dataset.schemas.length === 0 && (
        <div className="flex flex-1 items-center justify-center text-zinc-500">
          <p>No schemas found in this dataset</p>
        </div>
      )}
    </div>
  );
}
