"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import {
  Conversation,
  ConversationContent,
  ConversationEmptyState,
  ConversationScrollButton,
} from "./Conversation";
import { Message, MessageContent } from "./Message";
import { MessageResponse } from "./MessageResponse";
import { ChatInput } from "./ChatInput";

import { getWebSocketUrl } from "@/lib/ws-url";
import type { ToolType } from "@/lib/terminal-types";
import { toolThemes } from "@/lib/terminal-types";
import type { AgentInfo } from "@/api/agents";

function agentIdToToolType(id: string): ToolType {
  if (id in toolThemes) return id as ToolType;
  return "generic";
}

function capitalize(s: string): string {
  return s.charAt(0).toUpperCase() + s.slice(1);
}

export type ChatMessage = {
  role: "user" | "assistant";
  content: string;
  progress?: string;
  mode?: "standalone" | "stream";
};

export function ChatView() {
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [input, setInput] = useState("");
  const [connected, setConnected] = useState(false);
  const [streaming, setStreaming] = useState(false);

  const [agents, setAgents] = useState<AgentInfo[]>([]);
  const [selectedAgent, setSelectedAgent] = useState<string>("claude");
  const wsRef = useRef<WebSocket | null>(null);

  const toolType = agentIdToToolType(selectedAgent);
  const agentLabel = capitalize(selectedAgent);

  useEffect(() => {
    const ws = new WebSocket(getWebSocketUrl("/ws/chat"));
    wsRef.current = ws;

    ws.onopen = () => setConnected(true);
    ws.onclose = () => {
      setConnected(false);
      setStreaming(false);
    };
    ws.onerror = () => setConnected(false);

    ws.onmessage = (event) => {
      if (typeof event.data !== "string") return;
      const s = event.data as string;

      let j: Record<string, unknown>;
      try {
        j = JSON.parse(s);
      } catch {
        appendToStreamAssistant(s);
        return;
      }

      if (j.type === "config" && Array.isArray(j.agents)) {
        setAgents(j.agents as AgentInfo[]);
        if (typeof j.default_agent === "string") {
          setSelectedAgent(j.default_agent as string);
        }
        return;
      }

      if (j.kind === "start") {
        return;
      }

      if (j.kind === "turn_complete") {
        clearStreamProgress();
        setStreaming(false);
        return;
      }

      if (j.kind === "error" && typeof j.error === "string") {
        appendErrorToStream(j.error as string);
        setStreaming(false);
        return;
      }

      if (j.kind === "thinking" && typeof j.text === "string") {
        setStreamProgress(j.text as string);
        return;
      }

      if (j.kind === "tool_use" && typeof j.tool === "string") {
        setStreamProgress(`Using tool: ${j.tool}...`);
        return;
      }

      if (j.kind === "tool_result") {
        clearStreamProgress();
        return;
      }

      if (j.kind === "text" && typeof j.text === "string") {
        appendStandaloneAssistant(j.text as string);
        return;
      }

      if (j.kind === "token" && typeof j.delta === "string") {
        appendToStreamAssistant(j.delta as string);
        return;
      }

      if (typeof j.text === "string") {
        appendToStreamAssistant(j.text as string);
        return;
      }
    };

    function appendStandaloneAssistant(text: string) {
      if (!text) return;
      setMessages((prev) => {
        const next = [...prev];
        const last = next[next.length - 1];
        if (last?.role === "assistant" && last.mode === "stream" && last.content === "" && !last.progress) {
          next.pop();
        }
        next.push({ role: "assistant", content: text, mode: "standalone" });
        return next;
      });
    }

    function appendToStreamAssistant(text: string) {
      if (!text) return;
      setMessages((prev) => {
        if (prev.length === 0) return [{ role: "assistant", content: text, mode: "stream" }];
        const last = prev[prev.length - 1];
        if (last.role !== "assistant" || last.mode !== "stream") {
          return [...prev, { role: "assistant", content: text, mode: "stream" }];
        }
        const next = [...prev];
        next[next.length - 1] = { ...last, content: last.content + text, progress: undefined, mode: "stream" };
        return next;
      });
    }

    function setStreamProgress(progress: string) {
      setMessages((prev) => {
        const last = prev[prev.length - 1];
        if (!last || last.role !== "assistant" || last.mode !== "stream") {
          return [...prev, { role: "assistant", content: "", progress, mode: "stream" }];
        }
        const next = [...prev];
        next[next.length - 1] = { ...last, progress, mode: "stream" };
        return next;
      });
    }

    function clearStreamProgress() {
      setMessages((prev) => {
        const last = prev[prev.length - 1];
        if (!last || last.role !== "assistant" || last.mode !== "stream" || !last.progress) {
          return prev;
        }
        const next = [...prev];
        next[next.length - 1] = { ...last, progress: undefined, mode: "stream" };
        return next;
      });
    }

    function appendErrorToStream(error: string) {
      setMessages((prev) => {
        const last = prev[prev.length - 1];
        if (!last || last.role !== "assistant" || last.mode !== "stream") {
          return [...prev, { role: "assistant", content: `Error: ${error}`, mode: "stream" }];
        }
        const next = [...prev];
        next[next.length - 1] = {
          ...last,
          content: last.content + (last.content ? "\n\n" : "") + `Error: ${error}`,
          progress: undefined,
          mode: "stream",
        };
        return next;
      });
    }

    return () => {
      ws.close();
      wsRef.current = null;
    };
  }, []);

  const sendMessage = useCallback(async () => {
    const text = input.trim();
    if (!text || !wsRef.current || wsRef.current.readyState !== WebSocket.OPEN) return;

    setInput("");
    setMessages((prev) => [
      ...prev,
      { role: "user", content: text },
      { role: "assistant", content: "", mode: "stream" },
    ]);
    setStreaming(true);

    wsRef.current.send(JSON.stringify({ type: "message", text, agent: selectedAgent }));
  }, [input, selectedAgent]);

  const handleAgentChange = useCallback((agentId: string) => {
    setSelectedAgent(agentId);
  }, []);

  return (
    <div className="flex h-full flex-col overflow-hidden bg-background">
      <Conversation className="flex-1">
        <ConversationContent>
          {messages.length === 0 ? (
            <ConversationEmptyState
              title={`Chat with ${agentLabel}`}
              description="Send a message to start."
            />
          ) : (
            messages.map((msg, i) => (
              <Message key={i} from={msg.role}>
                <MessageContent
                  className={
                    msg.role === "user"
                      ? "rounded-lg bg-primary/15 px-4 py-3 text-foreground"
                      : msg.mode === "standalone"
                        ? "rounded-lg border border-border/60 bg-muted/30 px-4 py-3 text-muted-foreground"
                        : "rounded-lg bg-muted/50 px-4 py-3 text-foreground"
                  }
                >
                  {msg.role === "user" ? (
                    <p className="whitespace-pre-wrap text-sm">{msg.content}</p>
                  ) : msg.mode === "standalone" ? (
                    <p className="whitespace-pre-wrap text-sm leading-7">{msg.content}</p>
                  ) : (
                    <>
                      <MessageResponse
                        content={msg.content}
                        isStreaming={streaming && i === messages.length - 1}
                      />
                      {msg.progress && (
                        <span className="text-xs text-muted-foreground/60 font-mono animate-pulse">
                          {msg.progress}
                        </span>
                      )}
                    </>
                  )}
                </MessageContent>
              </Message>
            ))
          )}
        </ConversationContent>
        <ConversationScrollButton />
      </Conversation>

      <ChatInput
        value={input}
        onChange={setInput}
        onSubmit={() => {
          void sendMessage();
        }}
        disabled={!connected}
        isStreaming={streaming}
        placeholder={connected ? `Message ${agentLabel}…` : "Connecting…"}
        targetLabel={agentLabel}
        targetTool={toolType}
        agents={agents}
        onAgentChange={handleAgentChange}
      />
    </div>
  );
}
