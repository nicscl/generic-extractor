"use client";

import Link from "next/link";
import { usePathname, useRouter } from "next/navigation";
import { createClient } from "@/lib/supabase/client";
import type { Project } from "@/lib/types";
import {
  FolderOpen,
  Plus,
  LogOut,
  FileText,
  LayoutDashboard,
} from "lucide-react";
import clsx from "clsx";

interface SidebarProps {
  projects: Project[];
  userEmail: string;
}

export function Sidebar({ projects, userEmail }: SidebarProps) {
  const pathname = usePathname();
  const router = useRouter();
  const supabase = createClient();

  async function handleLogout() {
    await supabase.auth.signOut();
    router.push("/auth/login");
    router.refresh();
  }

  return (
    <aside className="flex h-screen w-64 flex-col border-r border-zinc-800 bg-zinc-900">
      {/* Logo */}
      <div className="flex items-center gap-2 border-b border-zinc-800 px-4 py-4">
        <FileText className="h-5 w-5 text-blue-400" />
        <span className="text-lg font-semibold text-white">Extractor</span>
      </div>

      {/* Nav */}
      <nav className="flex-1 overflow-y-auto p-3">
        <Link
          href="/"
          className={clsx(
            "mb-1 flex items-center gap-2 rounded-lg px-3 py-2 text-sm transition-colors",
            pathname === "/"
              ? "bg-zinc-800 text-white"
              : "text-zinc-400 hover:bg-zinc-800/50 hover:text-white"
          )}
        >
          <LayoutDashboard className="h-4 w-4" />
          Dashboard
        </Link>

        <div className="mb-1 mt-4 flex items-center justify-between px-3">
          <span className="text-xs font-medium uppercase tracking-wider text-zinc-500">
            Projects
          </span>
          <Link
            href="/?new=1"
            className="rounded p-0.5 text-zinc-500 hover:bg-zinc-800 hover:text-white"
          >
            <Plus className="h-3.5 w-3.5" />
          </Link>
        </div>

        {projects.map((p) => (
          <Link
            key={p.id}
            href={`/projects/${p.id}`}
            className={clsx(
              "mb-0.5 flex items-center gap-2 rounded-lg px-3 py-2 text-sm transition-colors",
              pathname.startsWith(`/projects/${p.id}`)
                ? "bg-zinc-800 text-white"
                : "text-zinc-400 hover:bg-zinc-800/50 hover:text-white"
            )}
          >
            <FolderOpen className="h-4 w-4 shrink-0" />
            <span className="truncate">{p.name}</span>
          </Link>
        ))}

        {projects.length === 0 && (
          <p className="px-3 py-2 text-xs text-zinc-600">No projects yet</p>
        )}
      </nav>

      {/* User */}
      <div className="border-t border-zinc-800 p-3">
        <div className="flex items-center justify-between">
          <span className="truncate text-sm text-zinc-400">{userEmail}</span>
          <button
            onClick={handleLogout}
            className="rounded p-1.5 text-zinc-500 hover:bg-zinc-800 hover:text-white"
            title="Sign out"
          >
            <LogOut className="h-4 w-4" />
          </button>
        </div>
      </div>
    </aside>
  );
}
