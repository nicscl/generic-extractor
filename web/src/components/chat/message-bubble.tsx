"use client";

import { Markdown } from "@/components/ui/markdown";
import { User, Bot } from "lucide-react";
import type { ChatMessageUI } from "@/hooks/use-chat";
import { ToolIndicator } from "./tool-indicator";

export function MessageBubble({ message }: { message: ChatMessageUI }) {
  const isUser = message.role === "user";

  return (
    <div className={`flex gap-3 ${isUser ? "justify-end" : ""}`}>
      {!isUser && (
        <div className="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg bg-blue-600/20 text-blue-400">
          <Bot className="h-4 w-4" />
        </div>
      )}

      <div className={`max-w-[80%] space-y-2 ${isUser ? "order-first" : ""}`}>
        {/* Tool calls */}
        {message.toolCalls?.map((tc, i) => (
          <ToolIndicator key={i} toolCall={tc} />
        ))}

        {/* Message content */}
        {message.content && (
          <div
            className={`rounded-xl px-4 py-3 ${
              isUser
                ? "bg-blue-600 text-white"
                : "bg-zinc-800 text-zinc-100"
            }`}
          >
            {isUser ? (
              <p className="whitespace-pre-wrap text-sm">{message.content}</p>
            ) : (
              <Markdown content={message.content} />
            )}
          </div>
        )}

        {/* Streaming indicator */}
        {message.isStreaming && !message.content && !message.toolCalls?.length && (
          <div className="rounded-xl bg-zinc-800 px-4 py-3">
            <div className="flex items-center gap-1">
              <div className="h-2 w-2 animate-bounce rounded-full bg-zinc-500" />
              <div className="h-2 w-2 animate-bounce rounded-full bg-zinc-500 [animation-delay:0.1s]" />
              <div className="h-2 w-2 animate-bounce rounded-full bg-zinc-500 [animation-delay:0.2s]" />
            </div>
          </div>
        )}
      </div>

      {isUser && (
        <div className="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg bg-zinc-700 text-zinc-300">
          <User className="h-4 w-4" />
        </div>
      )}
    </div>
  );
}
