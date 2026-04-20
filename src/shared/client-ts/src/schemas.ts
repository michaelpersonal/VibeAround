/**
 * zod schemas for everything the dashboard server returns over HTTP
 * or WebSocket.
 *
 * The source of truth is Rust: look at the `#[derive(Serialize)]` types
 * in `src/core/src/service/snapshot.rs` and `src/server/src/api_types.rs`
 * for the reference shapes. The docstrings on those Rust types carry
 * JSON examples; this file mirrors them. When the Rust side changes,
 * update the matching schema here in the same PR.
 *
 * Usage: call `.parse()` on every wire-crossing value so bad payloads
 * fail fast at the boundary instead of rotting through the UI.
 */

import { z } from "zod";

// ---------------------------------------------------------------------------
// Agent IDs (mirrors resources/agents.json — order not significant)
// ---------------------------------------------------------------------------

/** Every agent ID defined in `resources/agents.json`. Hand-maintained.
 *  When that file adds an entry, add it here too and the `Record<AgentId, ...>`
 *  consumers (display-name maps) will force you to supply the rest. */
export const AGENT_IDS = [
  "claude",
  "gemini",
  "opencode",
  "codex",
  "cursor",
  "kiro",
  "qwen-code",
] as const;

export type AgentId = (typeof AGENT_IDS)[number];

export const AgentIdSchema = z.enum(AGENT_IDS);

// ---------------------------------------------------------------------------
// Constants mirrored from Rust
// ---------------------------------------------------------------------------

/** Mirror of `common::preview_entries::SHARE_TTL_SECS`. */
export const PREVIEW_SHARE_TTL_SECS = 600;

// ---------------------------------------------------------------------------
// GET /api/agents — enabled agent list + default
// ---------------------------------------------------------------------------

export const AgentInfoSchema = z.object({
  id: z.string(),
  name: z.string(),
  description: z.string(),
});
export type AgentInfo = z.infer<typeof AgentInfoSchema>;

export const AgentsConfigSchema = z.object({
  agents: z.array(AgentInfoSchema),
  default_agent: z.string(),
});
export type AgentsConfig = z.infer<typeof AgentsConfigSchema>;

// ---------------------------------------------------------------------------
// Tunnel status — discriminated union matching Rust `TunnelStatus`
// (`src/core/src/tunnels/status.rs`).
// ---------------------------------------------------------------------------

export const TunnelStatusSchema = z.discriminatedUnion("state", [
  z.object({ state: z.literal("running") }),
  z.object({ state: z.literal("stopped"), reason: z.string() }),
  z.object({ state: z.literal("failed"), error: z.string() }),
]);
export type TunnelStatus = z.infer<typeof TunnelStatusSchema>;

// ---------------------------------------------------------------------------
// Per-domain runtime endpoints.
//
// Reference Rust shape lives in `src/server/src/api_types.rs`. Each
// endpoint:
// - HTTP GET returns the `Array<...>` body.
// - WS /ws/<domain> pushes the full array whenever the kernel manager
//   reports a change.
// ---------------------------------------------------------------------------

/** Channel lifecycle states from `ChannelMonitor`. */
export const CHANNEL_STATUS_VALUES = [
  "not_started",
  "spawning",
  "running",
  "crashed",
  "stopped",
] as const;
export const ChannelStatusSchema = z.enum(CHANNEL_STATUS_VALUES);
export type ChannelStatus = z.infer<typeof ChannelStatusSchema>;

export const ChannelRuntimeSchema = z.object({
  kind: z.string(),
  status: ChannelStatusSchema,
  reason: z.string().nullable(),
  crash_count: z.number(),
  last_seen_age_secs: z.number(),
  restart_in_secs: z.number(),
  started_at: z.number(),
});
export type ChannelRuntime = z.infer<typeof ChannelRuntimeSchema>;
export const ChannelRuntimeListSchema = z.array(ChannelRuntimeSchema);

export const TunnelRuntimeSchema = z.object({
  provider: z.string(),
  url: z.string().nullable(),
  status: TunnelStatusSchema,
  uptime_secs: z.number(),
});
export type TunnelRuntime = z.infer<typeof TunnelRuntimeSchema>;
export const TunnelRuntimeListSchema = z.array(TunnelRuntimeSchema);

export const AgentRuntimeSchema = z.object({
  route_key: z.string(),
  channel_kind: z.string(),
  chat_id: z.string(),
  cli_kind: z.string().nullable(),
  profile: z.string().nullable(),
  session_id: z.string().nullable(),
  workspace: z.string().nullable(),
  busy: z.boolean(),
  failed: z.string().nullable(),
  started_at: z.number(),
  agent_name: z.string().nullable(),
  agent_title: z.string().nullable(),
  agent_version: z.string().nullable(),
});
export type AgentRuntime = z.infer<typeof AgentRuntimeSchema>;
export const AgentRuntimeListSchema = z.array(AgentRuntimeSchema);

// ---------------------------------------------------------------------------
// /ws/chat — ChatEvent envelope
//
// Lifecycle events have hand-curated fields; streaming agent output
// rides through `acp_notification` carrying a raw ACP
// `SessionNotification` (from `@agentclientprotocol/sdk`). Consumers
// do a two-level switch: first on the envelope `kind`, then — inside
// `acp_notification` — on `payload.update.sessionUpdate`.
//
// The ACP payload itself isn't re-validated here (we trust the
// agent-client-protocol crate on the server side). If you need
// typed access to specific update variants on the TS side, import
// them from `@agentclientprotocol/sdk` directly.
// ---------------------------------------------------------------------------

export const ChatEventSchema = z.discriminatedUnion("kind", [
  z.object({
    kind: z.literal("config"),
    channel_id: z.string(),
    agents: z.array(AgentInfoSchema),
    default_agent: z.string(),
  }),
  z.object({
    kind: z.literal("agent_ready"),
    agent: z.string(),
    version: z.string(),
  }),
  z.object({
    kind: z.literal("session_ready"),
    session_id: z.string(),
  }),
  z.object({
    kind: z.literal("command_menu"),
    system_commands: z.unknown(),
    agent_commands: z.unknown(),
  }),
  z.object({
    kind: z.literal("permission_request"),
    request_id: z.string(),
    request: z.unknown(),
  }),
  z.object({
    kind: z.literal("system_text"),
    text: z.string(),
  }),
  z.object({
    kind: z.literal("acp_notification"),
    payload: z.unknown(),
  }),
  z.object({
    kind: z.literal("error"),
    error: z.string(),
  }),
]);
export type ChatEvent = z.infer<typeof ChatEventSchema>;
