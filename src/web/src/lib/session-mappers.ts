import type {
  TerminalGroup,
  TerminalSession,
  TerminalStatus,
  ToolType,
} from "./terminal-types";
import { getToolDisplayName } from "./agents";
import type { PtyRunState, SessionListItem } from "@va/client";

export const DEFAULT_GROUP_ID = "default";

export const DEFAULT_GROUPS: TerminalGroup[] = [
  { id: DEFAULT_GROUP_ID, label: "CLI", color: "#64748b", sessions: [] },
];

export type AppPage = "terminal" | "chat";

/**
 * Estimate terminal cols/rows from viewport so PTY spawns at the right
 * size (avoids TUI rendering glitches on mobile).
 */
export function estimateTerminalSize(): { cols: number; rows: number } {
  const charW = 7.2; // approx px per char at fontSize 11–12 with JetBrains Mono
  const lineH = 16.2; // approx px per line at lineHeight ~1.35
  const padH = 48; // header + padding
  const padW = 16; // horizontal padding
  const cols = Math.max(20, Math.floor((window.innerWidth - padW) / charW));
  const rows = Math.max(5, Math.floor((window.innerHeight - padH) / lineH));
  return { cols, rows };
}

export function sessionToName(tool: string): string {
  return getToolDisplayName(tool);
}

function mapApiStatus(s: PtyRunState): TerminalStatus {
  if (s.type === "running") return "running";
  if (s.type === "exited") return s.exit_code === 0 ? "stopped" : "error";
  return "idle";
}

function mapApiTool(s: string): ToolType {
  const t = s.toLowerCase();
  if (t === "claude" || t === "codex" || t === "gemini" || t === "opencode" || t === "generic") {
    return t as ToolType;
  }
  return "generic";
}

export function sessionListItemToSession(item: SessionListItem): TerminalSession {
  const baseName = item.profile_label
    ? `${sessionToName(item.launch_target ?? item.tool)} · ${item.profile_label}`
    : sessionToName(item.tool);
  return {
    id: item.session_id,
    name: item.tmux_session ? `tmux: ${item.tmux_session}` : baseName,
    group: DEFAULT_GROUP_ID,
    tool: mapApiTool(item.tool),
    status: mapApiStatus(item.status),
    command: item.tool,
    cwd: item.project_path ?? "—",
    startedAt: item.created_at * 1000,
    createdAt: item.created_at,
    profileId: item.profile_id ?? undefined,
    profileLabel: item.profile_label ?? undefined,
    launchTarget: item.launch_target ?? undefined,
    tmuxSession: item.tmux_session ?? undefined,
  };
}
