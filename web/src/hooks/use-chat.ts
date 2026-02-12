"use client";

import { useState, useCallback, useRef } from "react";
import type { ChatSSEEvent } from "@/lib/types";

export interface ChatMessageUI {
  id: string;
  role: "user" | "assistant" | "system" | "tool";
  content: string;
  toolCalls?: { name: string; args: Record<string, unknown>; result?: string }[];
  isStreaming?: boolean;
}

export function useChat(projectId: string) {
  const [messages, setMessages] = useState<ChatMessageUI[]>([]);
  const [isLoading, setIsLoading] = useState(false);
  const [status, setStatus] = useState<string | null>(null);
  const abortRef = useRef<AbortController | null>(null);

  const loadHistory = useCallback(async () => {
    try {
      const res = await fetch(`/api/projects/${projectId}/messages`);
      if (!res.ok) return;
      const data = await res.json();
      const loaded: ChatMessageUI[] = [];
      for (const msg of data) {
        if (msg.role === "tool") continue; // Tool messages embedded in assistant toolCalls
        const uiMsg: ChatMessageUI = {
          id: msg.id,
          role: msg.role,
          content: msg.content,
        };
        if (msg.tool_calls) {
          uiMsg.toolCalls = msg.tool_calls.map(
            (tc: { function: { name: string; arguments: string } }) => ({
              name: tc.function.name,
              args: JSON.parse(tc.function.arguments || "{}"),
            })
          );
        }
        loaded.push(uiMsg);
      }
      setMessages(loaded);
    } catch {
      // ignore
    }
  }, [projectId]);

  const sendMessage = useCallback(
    async (content: string, fileBase64?: string, fileName?: string) => {
      setIsLoading(true);
      setStatus(null);

      const userMsg: ChatMessageUI = {
        id: `tmp-${Date.now()}`,
        role: "user",
        content,
      };
      setMessages((prev) => [...prev, userMsg]);

      const assistantMsg: ChatMessageUI = {
        id: `tmp-${Date.now()}-assistant`,
        role: "assistant",
        content: "",
        toolCalls: [],
        isStreaming: true,
      };
      setMessages((prev) => [...prev, assistantMsg]);

      const newMessages: { role: string; content: string }[] = [
        { role: "user", content },
      ];

      // If file was uploaded, prepend info
      if (fileBase64 && fileName) {
        newMessages[0].content = `[Uploaded file: ${fileName}]\n\n${content}`;
      }

      abortRef.current = new AbortController();

      try {
        const res = await fetch("/api/chat", {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ projectId, messages: newMessages }),
          signal: abortRef.current.signal,
        });

        if (!res.ok || !res.body) {
          setMessages((prev) =>
            prev.map((m) =>
              m.id === assistantMsg.id
                ? { ...m, content: "Error: Failed to get response", isStreaming: false }
                : m
            )
          );
          setIsLoading(false);
          return;
        }

        const reader = res.body.getReader();
        const decoder = new TextDecoder();
        let buffer = "";

        while (true) {
          const { done, value } = await reader.read();
          if (done) break;

          buffer += decoder.decode(value, { stream: true });
          const lines = buffer.split("\n");
          buffer = lines.pop() ?? "";

          let eventType = "";
          for (const line of lines) {
            if (line.startsWith("event: ")) {
              eventType = line.slice(7);
            } else if (line.startsWith("data: ")) {
              const data = line.slice(6);
              try {
                const parsed = JSON.parse(data);
                parsed.type = eventType;
                handleEvent(parsed as ChatSSEEvent, assistantMsg.id);
              } catch {
                // Malformed JSON, skip
              }
            }
          }
        }

        // Mark streaming done
        setMessages((prev) =>
          prev.map((m) =>
            m.id === assistantMsg.id ? { ...m, isStreaming: false } : m
          )
        );
      } catch (err) {
        if ((err as Error).name !== "AbortError") {
          setMessages((prev) =>
            prev.map((m) =>
              m.id === assistantMsg.id
                ? { ...m, content: `Error: ${(err as Error).message}`, isStreaming: false }
                : m
            )
          );
        }
      }

      setIsLoading(false);
      setStatus(null);
    },
    [projectId]
  );

  function handleEvent(event: ChatSSEEvent, assistantId: string) {
    switch (event.type) {
      case "status":
        setStatus(event.message);
        break;

      case "tool_call":
        setMessages((prev) =>
          prev.map((m) =>
            m.id === assistantId
              ? {
                  ...m,
                  toolCalls: [
                    ...(m.toolCalls ?? []),
                    { name: event.tool_name, args: event.arguments as Record<string, unknown> },
                  ],
                }
              : m
          )
        );
        break;

      case "tool_result":
        setMessages((prev) =>
          prev.map((m) => {
            if (m.id !== assistantId) return m;
            const toolCalls = [...(m.toolCalls ?? [])];
            const idx = toolCalls.findLastIndex(
              (tc) => tc.name === event.tool_name && !tc.result
            );
            if (idx >= 0) toolCalls[idx] = { ...toolCalls[idx], result: event.result };
            return { ...m, toolCalls };
          })
        );
        break;

      case "message":
        setMessages((prev) =>
          prev.map((m) =>
            m.id === assistantId ? { ...m, content: event.content } : m
          )
        );
        break;

      case "error":
        setMessages((prev) =>
          prev.map((m) =>
            m.id === assistantId
              ? { ...m, content: `Error: ${event.message}`, isStreaming: false }
              : m
          )
        );
        break;

      case "done":
        break;
    }
  }

  function stopGeneration() {
    abortRef.current?.abort();
    setIsLoading(false);
    setStatus(null);
  }

  return { messages, isLoading, status, sendMessage, loadHistory, stopGeneration };
}
