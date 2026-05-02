import { useCallback, useEffect, useState } from "react";

import { createSession, deleteSession, getSessions } from "@/api/sessions";
import type { TerminalGroup, TerminalStatus, ToolType } from "@/lib/terminal-types";
import type { Theme } from "@/lib/theme";
import {
  DEFAULT_GROUPS,
  DEFAULT_GROUP_ID,
  estimateTerminalSize,
  sessionListItemToSession,
} from "@/lib/session-mappers";

interface UseSessionsInput {
  /** Only load when `page === "terminal"` — avoids spamming the backend while on chat. */
  active: boolean;
  theme: Theme;
  /** Invoked after a successful tmux attach so the caller can refresh its tmux list. */
  onTmuxAttached?: () => void;
}

interface UseSessionsResult {
  groups: TerminalGroup[];
  activeTabId: string | null;
  setActiveTabId: (id: string | null) => void;
  maximizedSession: string | null;
  sessionsLoading: boolean;
  addCli: (tool: ToolType) => Promise<void>;
  addProfileCli: (profileId: string, launchTarget: string) => Promise<void>;
  attachTmux: (sessionName: string) => Promise<void>;
  closeSession: (sessionId: string) => Promise<void>;
  setSessionState: (sessionId: string, tool: ToolType, status: TerminalStatus) => void;
  toggleMaximize: (sessionId: string) => void;
  clearMaximized: () => void;
}

/**
 * Owns the terminal-session collection: fetching on mount, creating CLI /
 * tmux sessions, closing, tab activation, maximize toggling, and applying
 * runtime status updates coming from each TerminalPanel.
 */
export function useSessions({ active, theme, onTmuxAttached }: UseSessionsInput): UseSessionsResult {
  const [groups, setGroups] = useState<TerminalGroup[]>(DEFAULT_GROUPS);
  const [activeTabId, setActiveTabId] = useState<string | null>(null);
  const [maximizedSession, setMaximizedSession] = useState<string | null>(null);
  const [sessionsLoading, setSessionsLoading] = useState(true);

  useEffect(() => {
    if (!active) return;
    setSessionsLoading(true);
    getSessions()
      .then((list) => {
        const sessions = list.map(sessionListItemToSession);
        setGroups((prev) => {
          const g = prev.find((x) => x.id === DEFAULT_GROUP_ID) ?? DEFAULT_GROUPS[0];
          return [{ ...g, sessions }];
        });
        if (sessions.length > 0) {
          setActiveTabId((prev) => (prev && sessions.some((s) => s.id === prev) ? prev : sessions[0].id));
        } else {
          setActiveTabId(null);
        }
      })
      .catch((e) => console.error("[VibeAround] getSessions:", e))
      .finally(() => setSessionsLoading(false));
  }, [active]);

  const closeSession = useCallback(
    async (sessionId: string) => {
      try {
        await deleteSession(sessionId);
        setGroups((prev) =>
          prev.map((g) => ({
            ...g,
            sessions: g.sessions.filter((s) => s.id !== sessionId),
          })),
        );
        setActiveTabId((prev) => {
          if (prev !== sessionId) return prev;
          const remaining = groups.flatMap((g) => g.sessions).filter((s) => s.id !== sessionId);
          return remaining[0]?.id ?? null;
        });
        setMaximizedSession((m) => (m === sessionId ? null : m));
      } catch (e) {
        console.error("[VibeAround] deleteSession:", e);
      }
    },
    [groups],
  );

  const addCli = useCallback(
    async (tool: ToolType) => {
      try {
        const { cols, rows } = estimateTerminalSize();
        const res = await createSession({ tool, theme, cols, rows });
        const session = sessionListItemToSession({
          session_id: res.session_id,
          tool: res.tool,
          status: { type: "running", tool: res.tool },
          created_at: res.created_at,
          project_path: res.project_path,
          profile_id: res.profile_id,
          profile_label: res.profile_label,
          launch_target: res.launch_target,
          tmux_session: null,
        });
        setGroups((prev) =>
          prev.map((g) =>
            g.id === DEFAULT_GROUP_ID ? { ...g, sessions: [...g.sessions, session] } : g,
          ),
        );
        setActiveTabId(session.id);
      } catch (e) {
        console.error("[VibeAround] createSession:", e);
      }
    },
    [theme],
  );

  const addProfileCli = useCallback(
    async (profileId: string, launchTarget: string) => {
      try {
        const { cols, rows } = estimateTerminalSize();
        const res = await createSession({
          profile_id: profileId,
          launch_target: launchTarget,
          theme,
          cols,
          rows,
        });
        const session = sessionListItemToSession({
          session_id: res.session_id,
          tool: res.tool,
          status: { type: "running", tool: res.tool },
          created_at: res.created_at,
          project_path: res.project_path,
          profile_id: res.profile_id,
          profile_label: res.profile_label,
          launch_target: res.launch_target,
          tmux_session: null,
        });
        setGroups((prev) =>
          prev.map((g) =>
            g.id === DEFAULT_GROUP_ID ? { ...g, sessions: [...g.sessions, session] } : g,
          ),
        );
        setActiveTabId(session.id);
      } catch (e) {
        console.error("[VibeAround] create profile session:", e);
      }
    },
    [theme],
  );

  const attachTmux = useCallback(
    async (sessionName: string) => {
      try {
        const allSessions = groups.flatMap((g) => g.sessions);
        const existingTab = allSessions.find(
          (s) => s.tmuxSession === sessionName && s.status === "running",
        );
        if (existingTab) {
          setActiveTabId(existingTab.id);
          return;
        }

        const { cols, rows } = estimateTerminalSize();
        const res = await createSession({
          tool: "generic",
          tmux_session: sessionName,
          theme,
          cols,
          rows,
        });
        const session = sessionListItemToSession({
          session_id: res.session_id,
          tool: "generic",
          status: { type: "running", tool: "generic" },
          created_at: res.created_at,
          project_path: res.project_path,
          profile_id: res.profile_id,
          profile_label: res.profile_label,
          launch_target: res.launch_target,
          tmux_session: sessionName,
        });
        setGroups((prev) =>
          prev.map((g) =>
            g.id === DEFAULT_GROUP_ID ? { ...g, sessions: [...g.sessions, session] } : g,
          ),
        );
        setActiveTabId(session.id);
        onTmuxAttached?.();
      } catch (e) {
        console.error("[VibeAround] attachTmux:", e);
      }
    },
    [groups, theme, onTmuxAttached],
  );

  const setSessionState = useCallback(
    (sessionId: string, tool: ToolType, status: TerminalStatus) => {
      setGroups((prev) =>
        prev.map((g) => ({
          ...g,
          sessions: g.sessions.map((s) => (s.id === sessionId ? { ...s, tool, status } : s)),
        })),
      );
    },
    [],
  );

  const toggleMaximize = useCallback((sessionId: string) => {
    setMaximizedSession((prev) => (prev === sessionId ? null : sessionId));
  }, []);

  const clearMaximized = useCallback(() => setMaximizedSession(null), []);

  return {
    groups,
    activeTabId,
    setActiveTabId,
    maximizedSession,
    sessionsLoading,
    addCli,
    addProfileCli,
    attachTmux,
    closeSession,
    setSessionState,
    toggleMaximize,
    clearMaximized,
  };
}
