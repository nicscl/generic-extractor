"use client";

import { useRouter } from "next/navigation";
import { Trash2 } from "lucide-react";

export function ProjectActions({ projectId }: { projectId: string }) {
  const router = useRouter();

  async function handleDelete() {
    if (!confirm("Delete this project? This cannot be undone.")) return;
    const res = await fetch(`/api/projects/${projectId}`, { method: "DELETE" });
    if (res.ok) {
      router.push("/");
      router.refresh();
    }
  }

  return (
    <button
      onClick={handleDelete}
      className="rounded-lg border border-zinc-700 p-2 text-zinc-400 hover:border-red-700 hover:bg-red-950 hover:text-red-400"
      title="Delete project"
    >
      <Trash2 className="h-4 w-4" />
    </button>
  );
}
