import { NextRequest, NextResponse } from "next/server";
import { createClient } from "@/lib/supabase/server";

const EXTRACTOR_URL = process.env.EXTRACTOR_API_URL ?? "http://localhost:3002";

// Proxy GET requests to the Rust API (auth-gated)
export async function GET(
  req: NextRequest,
  { params }: { params: Promise<{ path: string[] }> }
) {
  const supabase = await createClient();
  const { data: { user } } = await supabase.auth.getUser();
  if (!user) return NextResponse.json({ error: "Unauthorized" }, { status: 401 });

  const { path } = await params;
  const targetPath = "/" + path.join("/");
  const url = new URL(req.url);
  const qs = url.search;

  try {
    const res = await fetch(`${EXTRACTOR_URL}${targetPath}${qs}`, {
      headers: { Accept: "application/json" },
    });
    const data = await res.json();
    return NextResponse.json(data, { status: res.status });
  } catch (err) {
    return NextResponse.json(
      { error: (err as Error).message },
      { status: 502 }
    );
  }
}
