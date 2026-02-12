export const dynamic = "force-dynamic";

import { createClient } from "@/lib/supabase/server";
import { notFound } from "next/navigation";
import { ChatPanel } from "@/components/chat/chat-panel";
import Link from "next/link";
import { ArrowLeft } from "lucide-react";

export default async function ChatPage({
  params,
}: {
  params: Promise<{ id: string }>;
}) {
  const { id } = await params;
  const supabase = await createClient();

  const { data: project } = await supabase
    .from("projects")
    .select("id, name")
    .eq("id", id)
    .single();

  if (!project) notFound();

  return (
    <div className="relative flex h-full flex-col">
      <div className="flex items-center gap-3 border-b border-zinc-800 px-6 py-3">
        <Link
          href={`/projects/${id}`}
          className="rounded p-1 text-zinc-400 hover:bg-zinc-800 hover:text-white"
        >
          <ArrowLeft className="h-4 w-4" />
        </Link>
        <h1 className="font-medium text-white">{project.name}</h1>
        <span className="text-sm text-zinc-500">Chat</span>
      </div>
      <div className="flex-1">
        <ChatPanel projectId={id} />
      </div>
    </div>
  );
}
