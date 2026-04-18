/**
 * Agents API: fetch enabled agents from backend.
 */

import type { AgentInfo } from "@va/generated/AgentInfo";
import type { AgentsConfig } from "@va/generated/AgentsConfig";

export type { AgentInfo, AgentsConfig };

/** All dashboard routes live under /va/ to keep the root namespace free for
 *  cookie-based dev-server preview proxying. */
const VA_PREFIX = "/va";

function getBaseUrl(): string {
  if (typeof window === "undefined") return `http://127.0.0.1:12358${VA_PREFIX}`;
  return `${window.location.origin}${VA_PREFIX}`;
}

export async function getAgents(): Promise<AgentsConfig> {
  const res = await fetch(`${getBaseUrl()}/api/agents`);
  if (!res.ok) throw new Error(`GET /api/agents: ${res.status}`);
  return res.json();
}
