"use client";

import { useRef, useEffect, useState, useCallback } from "react";
import { Send, Paperclip, Square, Upload } from "lucide-react";
import { MessageBubble } from "./message-bubble";
import { useChat } from "@/hooks/use-chat";

export function ChatPanel({ projectId }: { projectId: string }) {
  const { messages, isLoading, status, sendMessage, loadHistory, stopGeneration } =
    useChat(projectId);
  const [input, setInput] = useState("");
  const [pendingFile, setPendingFile] = useState<{
    base64: string;
    name: string;
  } | null>(null);
  const [isDragOver, setIsDragOver] = useState(false);
  const scrollRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    loadHistory();
  }, [loadHistory]);

  useEffect(() => {
    if (scrollRef.current) {
      scrollRef.current.scrollTop = scrollRef.current.scrollHeight;
    }
  }, [messages]);

  const handleSubmit = useCallback(
    (e?: React.FormEvent) => {
      e?.preventDefault();
      const text = input.trim();
      if (!text && !pendingFile) return;

      const msg = pendingFile
        ? `I've uploaded ${pendingFile.name}. ${text || "Please extract and analyze it."}`
        : text;

      sendMessage(msg, pendingFile?.base64, pendingFile?.name);
      setInput("");
      setPendingFile(null);
    },
    [input, pendingFile, sendMessage]
  );

  function handleKeyDown(e: React.KeyboardEvent) {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      handleSubmit();
    }
  }

  async function handleFile(file: File) {
    const formData = new FormData();
    formData.append("file", file);
    const res = await fetch("/api/upload", { method: "POST", body: formData });
    if (res.ok) {
      const data = await res.json();
      setPendingFile({ base64: data.fileBase64, name: data.fileName });
    }
  }

  function handleDrop(e: React.DragEvent) {
    e.preventDefault();
    setIsDragOver(false);
    const file = e.dataTransfer.files[0];
    if (file) handleFile(file);
  }

  return (
    <div
      className="flex h-full flex-col"
      onDragOver={(e) => {
        e.preventDefault();
        setIsDragOver(true);
      }}
      onDragLeave={() => setIsDragOver(false)}
      onDrop={handleDrop}
    >
      {/* Drop overlay */}
      {isDragOver && (
        <div className="absolute inset-0 z-50 flex items-center justify-center bg-blue-600/10 backdrop-blur-sm">
          <div className="flex flex-col items-center gap-2 text-blue-400">
            <Upload className="h-10 w-10" />
            <span className="text-lg font-medium">Drop file to upload</span>
          </div>
        </div>
      )}

      {/* Messages */}
      <div ref={scrollRef} className="flex-1 overflow-y-auto p-6">
        <div className="mx-auto max-w-3xl space-y-4">
          {messages.length === 0 && (
            <div className="flex flex-col items-center justify-center py-20 text-zinc-500">
              <p className="text-lg">Start a conversation</p>
              <p className="mt-1 text-sm">
                Ask me to extract documents, analyze data, or search existing extractions.
              </p>
            </div>
          )}
          {messages.map((msg) => (
            <MessageBubble key={msg.id} message={msg} />
          ))}
        </div>
      </div>

      {/* Status */}
      {status && (
        <div className="border-t border-zinc-800 px-6 py-2">
          <p className="text-xs text-yellow-400">{status}</p>
        </div>
      )}

      {/* Pending file */}
      {pendingFile && (
        <div className="mx-6 flex items-center gap-2 rounded-t-lg border border-b-0 border-zinc-700 bg-zinc-800 px-3 py-2">
          <Paperclip className="h-4 w-4 text-zinc-400" />
          <span className="text-sm text-zinc-300">{pendingFile.name}</span>
          <button
            onClick={() => setPendingFile(null)}
            className="ml-auto text-xs text-zinc-500 hover:text-white"
          >
            Remove
          </button>
        </div>
      )}

      {/* Input */}
      <div className="border-t border-zinc-800 p-4">
        <form
          onSubmit={handleSubmit}
          className="mx-auto flex max-w-3xl items-end gap-2"
        >
          <input
            ref={fileInputRef}
            type="file"
            className="hidden"
            accept=".pdf,.csv,.xlsx,.xlsm,.xlsb,.xls"
            onChange={(e) => {
              const file = e.target.files?.[0];
              if (file) handleFile(file);
              e.target.value = "";
            }}
          />
          <button
            type="button"
            onClick={() => fileInputRef.current?.click()}
            className="rounded-lg p-2 text-zinc-400 hover:bg-zinc-800 hover:text-white"
            title="Upload file"
          >
            <Paperclip className="h-5 w-5" />
          </button>

          <textarea
            ref={inputRef}
            value={input}
            onChange={(e) => setInput(e.target.value)}
            onKeyDown={handleKeyDown}
            placeholder="Type a message..."
            rows={1}
            className="max-h-32 flex-1 resize-none rounded-xl border border-zinc-700 bg-zinc-800 px-4 py-3 text-sm text-white placeholder-zinc-500 focus:border-blue-500 focus:outline-none"
          />

          {isLoading ? (
            <button
              type="button"
              onClick={stopGeneration}
              className="rounded-lg bg-red-600 p-2 text-white hover:bg-red-700"
              title="Stop"
            >
              <Square className="h-5 w-5" />
            </button>
          ) : (
            <button
              type="submit"
              disabled={!input.trim() && !pendingFile}
              className="rounded-lg bg-blue-600 p-2 text-white hover:bg-blue-700 disabled:opacity-50"
              title="Send"
            >
              <Send className="h-5 w-5" />
            </button>
          )}
        </form>
      </div>
    </div>
  );
}
