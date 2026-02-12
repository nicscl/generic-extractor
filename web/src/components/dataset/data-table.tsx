"use client";

import { ChevronLeft, ChevronRight } from "lucide-react";
import type { ColumnDef } from "@/lib/types";

interface DataTableProps {
  columns: ColumnDef[];
  rows: Record<string, unknown>[];
  totalRows: number;
  offset: number;
  limit: number;
  onPageChange: (newOffset: number) => void;
  loading?: boolean;
}

export function DataTable({
  columns,
  rows,
  totalRows,
  offset,
  limit,
  onPageChange,
  loading,
}: DataTableProps) {
  const totalPages = Math.ceil(totalRows / limit);
  const currentPage = Math.floor(offset / limit) + 1;

  return (
    <div>
      <div className="overflow-x-auto rounded-lg border border-zinc-800">
        <table className="w-full text-left text-sm">
          <thead className="border-b border-zinc-800 bg-zinc-900">
            <tr>
              {columns.map((col) => (
                <th
                  key={col.name}
                  className="whitespace-nowrap px-4 py-3 font-medium text-zinc-300"
                >
                  <div>{col.name}</div>
                  <div className="mt-0.5 text-[10px] font-normal text-zinc-600">
                    {col.data_type}
                    {col.format ? ` (${col.format})` : ""}
                  </div>
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {rows.map((row, i) => (
              <tr
                key={i}
                className="border-b border-zinc-800/50 hover:bg-zinc-800/30"
              >
                {columns.map((col) => (
                  <td
                    key={col.name}
                    className="whitespace-nowrap px-4 py-2 text-zinc-300"
                  >
                    {row[col.name] !== null && row[col.name] !== undefined
                      ? String(row[col.name])
                      : "—"}
                  </td>
                ))}
              </tr>
            ))}
            {rows.length === 0 && (
              <tr>
                <td
                  colSpan={columns.length}
                  className="px-4 py-8 text-center text-zinc-500"
                >
                  {loading ? "Loading..." : "No rows"}
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </div>

      {/* Pagination */}
      {totalRows > limit && (
        <div className="mt-3 flex items-center justify-between">
          <span className="text-sm text-zinc-500">
            Showing {offset + 1}-{Math.min(offset + limit, totalRows)} of{" "}
            {totalRows} rows
          </span>
          <div className="flex items-center gap-2">
            <button
              onClick={() => onPageChange(Math.max(0, offset - limit))}
              disabled={offset === 0 || loading}
              className="rounded-lg border border-zinc-700 p-1.5 text-zinc-400 hover:bg-zinc-800 hover:text-white disabled:opacity-30"
            >
              <ChevronLeft className="h-4 w-4" />
            </button>
            <span className="text-sm text-zinc-400">
              Page {currentPage} of {totalPages}
            </span>
            <button
              onClick={() => onPageChange(offset + limit)}
              disabled={offset + limit >= totalRows || loading}
              className="rounded-lg border border-zinc-700 p-1.5 text-zinc-400 hover:bg-zinc-800 hover:text-white disabled:opacity-30"
            >
              <ChevronRight className="h-4 w-4" />
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
