export const dynamic = "force-dynamic";

import { createClient } from "@/lib/supabase/server";
import { notFound } from "next/navigation";
import Link from "next/link";
import { MessageSquare, FileText, Table2, ArrowLeft } from "lucide-react";
import type { Project } from "@/lib/types";
import { ProjectActions } from "@/components/ui/project-actions";

export default async function ProjectPage({
  params,
}: {
  params: Promise<{ id: string }>;
}) {
  const { id } = await params;
  const supabase = await createClient();

  const { data: project } = await supabase
    .from("projects")
    .select("*")
    .eq("id", id)
    .single();

  if (!project) notFound();

  const typedProject = project as Project;

  // Fetch linked extractions and datasets from Rust API
  const extractorUrl = process.env.EXTRACTOR_API_URL ?? "http://localhost:3002";
  let extractions: { id: string; source_file: string; readable_id?: string; summary: string }[] = [];
  let datasets: { id: string; source_file: string; summary: string; status: string }[] = [];

  try {
    const [extRes, dsRes] = await Promise.all([
      fetch(`${extractorUrl}/extractions`, { cache: "no-store" }),
      fetch(`${extractorUrl}/datasets`, { cache: "no-store" }),
    ]);
    if (extRes.ok) extractions = await extRes.json();
    if (dsRes.ok) datasets = await dsRes.json();
  } catch {
    // Rust API may be down
  }

  return (
    <div className="p-8">
      <Link
        href="/"
        className="mb-4 inline-flex items-center gap-1 text-sm text-zinc-400 hover:text-white"
      >
        <ArrowLeft className="h-4 w-4" />
        Back
      </Link>

      <div className="mb-6 flex items-start justify-between">
        <div>
          <h1 className="text-2xl font-semibold">{typedProject.name}</h1>
          {typedProject.description && (
            <p className="mt-1 text-zinc-400">{typedProject.description}</p>
          )}
        </div>
        <div className="flex items-center gap-2">
          <Link
            href={`/projects/${id}/chat`}
            className="flex items-center gap-2 rounded-lg bg-blue-600 px-4 py-2 text-sm font-medium hover:bg-blue-700"
          >
            <MessageSquare className="h-4 w-4" />
            Chat
          </Link>
          <ProjectActions projectId={id} />
        </div>
      </div>

      {/* Extractions */}
      <section className="mb-8">
        <h2 className="mb-3 flex items-center gap-2 text-lg font-medium">
          <FileText className="h-5 w-5 text-blue-400" />
          Extractions
        </h2>
        {extractions.length === 0 ? (
          <p className="text-sm text-zinc-500">
            No extractions yet. Use the chat to extract documents.
          </p>
        ) : (
          <div className="space-y-2">
            {extractions.map((ext) => (
              <Link
                key={ext.id}
                href={`/extractions/${ext.id}`}
                className="block rounded-lg border border-zinc-800 bg-zinc-900 p-4 transition-colors hover:border-zinc-700"
              >
                <div className="flex items-center justify-between">
                  <span className="font-medium text-white">
                    {ext.readable_id ?? ext.source_file}
                  </span>
                  <span className="text-xs text-zinc-500">{ext.id.slice(0, 12)}...</span>
                </div>
                <p className="mt-1 text-sm text-zinc-400 line-clamp-2">{ext.summary}</p>
              </Link>
            ))}
          </div>
        )}
      </section>

      {/* Datasets */}
      <section>
        <h2 className="mb-3 flex items-center gap-2 text-lg font-medium">
          <Table2 className="h-5 w-5 text-green-400" />
          Datasets
        </h2>
        {datasets.length === 0 ? (
          <p className="text-sm text-zinc-500">
            No datasets yet. Use the chat to extract sheets.
          </p>
        ) : (
          <div className="space-y-2">
            {datasets.map((ds) => (
              <Link
                key={ds.id}
                href={`/datasets/${ds.id}`}
                className="block rounded-lg border border-zinc-800 bg-zinc-900 p-4 transition-colors hover:border-zinc-700"
              >
                <div className="flex items-center justify-between">
                  <span className="font-medium text-white">{ds.source_file}</span>
                  <span
                    className={`text-xs ${
                      ds.status === "completed"
                        ? "text-green-400"
                        : ds.status === "processing"
                        ? "text-yellow-400"
                        : "text-red-400"
                    }`}
                  >
                    {ds.status}
                  </span>
                </div>
                <p className="mt-1 text-sm text-zinc-400 line-clamp-2">{ds.summary}</p>
              </Link>
            ))}
          </div>
        )}
      </section>
    </div>
  );
}
