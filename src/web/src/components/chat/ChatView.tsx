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
import { agentIdToToolType, getAgentDisplayName } from "@/lib/agents";
import { ChatEventSchema, type AgentInfo } from "@va/client";
import type { SessionNotification } from "@agentclientprotocol/sdk";

export type ChatMessage = {
  role: "user" | "assistant";
  content: string;
  progress?: string;
  mode?: "standalone" | "stream";
};

type ChatMeta = {
  channelId?: string;
  sessionId?: string;
  agentTitle?: string;
  agentVersion?: string;
  agentName?: string;
};

export function ChatView() {
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [input, setInput] = useState("");
  const [connected, setConnected] = useState(false);
  const [streaming, setStreaming] = useState(false);
  const [meta, setMeta] = useState<ChatMeta>({});

  const [agents, setAgents] = useState<AgentInfo[]>([]);
  const [selectedAgent, setSelectedAgent] = useState<string>("claude");
  const wsRef = useRef<WebSocket | null>(null);

  const toolType = agentIdToToolType(selectedAgent);
  const selectedAgentInfo = agents.find((agent) => agent.id === selectedAgent);
  const agentLabel = selectedAgentInfo?.name ?? getAgentDisplayName(selectedAgent);

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

      let parsed;
      try {
        parsed = ChatEventSchema.parse(JSON.parse(event.data));
      } catch (e) {
        console.warn("[ChatView] bad chat frame, dropping:", e);
        return;
      }

      switch (parsed.kind) {
        case "config": {
          setAgents(parsed.agents);
          setMeta((prev) => ({ ...prev, channelId: parsed.channel_id }));
          setSelectedAgent(parsed.default_agent);
          break;
        }
        case "agent_ready": {
          setMeta((prev) => ({
            ...prev,
            agentName: parsed.agent,
            agentVersion: parsed.version,
          }));
          break;
        }
        case "session_ready": {
          setMeta((prev) => ({ ...prev, sessionId: parsed.session_id }));
          break;
        }
        case "system_text": {
          appendStandaloneAssistant(parsed.text);
          break;
        }
        case "error": {
          appendErrorToStream(parsed.error);
          setStreaming(false);
          break;
        }
        case "acp_notification": {
          handleAcpNotification(parsed.payload as SessionNotification);
          break;
        }
        case "command_menu":
        case "permission_request":
          // Not wired into the web chat UI yet.
          break;
      }
    };

    function handleAcpNotification(notif: SessionNotification) {
      const update = notif.update;
      switch (update.sessionUpdate) {
        case "agent_message_chunk": {
          if (update.content.type === "text") {
            appendToStreamAssistant(update.content.text);
          }
          break;
        }
        case "agent_thought_chunk": {
          if (update.content.type === "text") {
            setStreamProgress(update.content.text);
          }
          break;
        }
        case "tool_call":
        case "tool_call_update": {
          const title = "title" in update ? update.title : undefined;
          const status = "status" in update ? update.status : undefined;
          if (status === "completed" || status === "failed") {
            clearStreamProgress();
          } else {
            setStreamProgress(`Using tool: ${title ?? "tool"}…`);
          }
          break;
        }
        // Other ACP update variants (plan, available_commands_update, etc.)
        // are not yet surfaced in the web chat UI. Ignored rather than
        // erroring so future SDK additions don't crash the handler.
        default:
          break;
      }
    }

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
      <div className="border-b border-border/60 bg-muted/20 px-4 py-2 text-xs text-muted-foreground">
        <div className="flex flex-wrap items-center gap-x-4 gap-y-1 font-mono">
          <span>channel: web</span>
          <span>chat: {meta.channelId ?? "-"}</span>
          <span>agent: {meta.agentTitle ?? meta.agentName ?? agentLabel}</span>
          <span>version: {meta.agentVersion ?? "-"}</span>
          <span>sessionId: {meta.sessionId ?? "-"}</span>
        </div>
      </div>
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
