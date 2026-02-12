import { NextRequest } from "next/server";
import { createClient } from "@/lib/supabase/server";
import { toolDefinitions } from "@/lib/tools/definitions";
import { executeTool } from "@/lib/tools/executor";
import { buildSystemPrompt } from "@/lib/tools/system-prompt";
import type { ChatMessage, ToolCall } from "@/lib/types";

const OPENROUTER_API_KEY = process.env.OPENROUTER_API_KEY!;
const CHAT_MODEL = process.env.CHAT_MODEL ?? "google/gemini-2.5-flash-preview";
const MAX_TOOL_ROUNDS = 10;

interface OpenRouterMessage {
  role: "system" | "user" | "assistant" | "tool";
  content: string | null;
  tool_calls?: ToolCall[];
  tool_call_id?: string;
}

export async function POST(req: NextRequest) {
  const supabase = await createClient();
  const {
    data: { user },
  } = await supabase.auth.getUser();
  if (!user) {
    return new Response(JSON.stringify({ error: "Unauthorized" }), {
      status: 401,
    });
  }

  const body = await req.json();
  const { projectId, messages: clientMessages } = body as {
    projectId: string;
    messages: { role: string; content: string }[];
  };

  // Load project name for system prompt
  let projectName: string | undefined;
  if (projectId) {
    const { data: project } = await supabase
      .from("projects")
      .select("name")
      .eq("id", projectId)
      .single();
    projectName = project?.name;
  }

  // Build conversation
  const conversation: OpenRouterMessage[] = [
    { role: "system", content: buildSystemPrompt(projectName) },
  ];

  // Load chat history from Supabase
  if (projectId) {
    const { data: history } = await supabase
      .from("chat_messages")
      .select("*")
      .eq("project_id", projectId)
      .order("created_at", { ascending: true });

    if (history) {
      for (const msg of history as ChatMessage[]) {
        const orMsg: OpenRouterMessage = {
          role: msg.role as OpenRouterMessage["role"],
          content: msg.content,
        };
        if (msg.tool_calls) orMsg.tool_calls = msg.tool_calls;
        if (msg.tool_call_id) orMsg.tool_call_id = msg.tool_call_id;
        conversation.push(orMsg);
      }
    }
  }

  // Append new user messages
  for (const m of clientMessages) {
    conversation.push({ role: m.role as "user", content: m.content });
  }

  // SSE response
  const encoder = new TextEncoder();
  const stream = new ReadableStream({
    async start(controller) {
      function send(event: string, data: unknown) {
        controller.enqueue(
          encoder.encode(`event: ${event}\ndata: ${JSON.stringify(data)}\n\n`)
        );
      }

      const messagesToPersist: Omit<ChatMessage, "id" | "created_at">[] = [];

      // Persist new user messages
      for (const m of clientMessages) {
        messagesToPersist.push({
          project_id: projectId,
          role: m.role as ChatMessage["role"],
          content: m.content,
          tool_calls: null,
          tool_call_id: null,
          tool_name: null,
        });
      }

      try {
        let rounds = 0;

        while (rounds < MAX_TOOL_ROUNDS) {
          rounds++;

          const orRes = await fetch(
            "https://openrouter.ai/api/v1/chat/completions",
            {
              method: "POST",
              headers: {
                Authorization: `Bearer ${OPENROUTER_API_KEY}`,
                "Content-Type": "application/json",
              },
              body: JSON.stringify({
                model: CHAT_MODEL,
                messages: conversation,
                tools: toolDefinitions,
                tool_choice: "auto",
              }),
            }
          );

          if (!orRes.ok) {
            const errText = await orRes.text();
            send("error", { message: `LLM API error: ${orRes.status} ${errText}` });
            break;
          }

          const orData = await orRes.json();
          const choice = orData.choices?.[0];
          if (!choice) {
            send("error", { message: "No response from LLM" });
            break;
          }

          const assistantMsg = choice.message;

          // Add assistant message to conversation
          conversation.push(assistantMsg);

          if (assistantMsg.tool_calls && assistantMsg.tool_calls.length > 0) {
            // Assistant wants to call tools
            messagesToPersist.push({
              project_id: projectId,
              role: "assistant",
              content: assistantMsg.content ?? "",
              tool_calls: assistantMsg.tool_calls,
              tool_call_id: null,
              tool_name: null,
            });

            for (const tc of assistantMsg.tool_calls) {
              const toolName = tc.function.name;
              send("tool_call", {
                tool_name: toolName,
                arguments: JSON.parse(tc.function.arguments || "{}"),
              });
              send("status", { message: `Calling ${toolName}...` });

              const toolArgs = JSON.parse(tc.function.arguments || "{}");
              const result = await executeTool(toolName, toolArgs);

              send("tool_result", { tool_name: toolName, result });

              // Add tool result to conversation
              const toolMsg: OpenRouterMessage = {
                role: "tool",
                content: result,
                tool_call_id: tc.id,
              };
              conversation.push(toolMsg);

              messagesToPersist.push({
                project_id: projectId,
                role: "tool",
                content: result,
                tool_calls: null,
                tool_call_id: tc.id,
                tool_name: toolName,
              });
            }

            // Continue loop — LLM needs to process tool results
            continue;
          }

          // No tool calls — this is the final text response
          const content = assistantMsg.content ?? "";
          send("message", { content });

          messagesToPersist.push({
            project_id: projectId,
            role: "assistant",
            content,
            tool_calls: null,
            tool_call_id: null,
            tool_name: null,
          });

          break;
        }
      } catch (err) {
        send("error", {
          message: err instanceof Error ? err.message : String(err),
        });
      }

      // Persist all messages
      if (projectId && messagesToPersist.length > 0) {
        await supabase.from("chat_messages").insert(messagesToPersist);
      }

      send("done", {});
      controller.close();
    },
  });

  return new Response(stream, {
    headers: {
      "Content-Type": "text/event-stream",
      "Cache-Control": "no-cache",
      Connection: "keep-alive",
    },
  });
}
