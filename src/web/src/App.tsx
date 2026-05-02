import { useState } from "react";

import { AppFooter } from "@/components/AppFooter";
import { AppHeader } from "@/components/AppHeader";
import { ChatView } from "@/components/chat";
import { EmptyTerminalState } from "@/components/EmptyTerminalState";
import { TabBar } from "@/components/TabBar";
import { TerminalGridView } from "@/components/TerminalGridView";
import { TerminalPanel } from "@/components/TerminalPanel";
import { usePing } from "@/hooks/usePing";
import { useSessions } from "@/hooks/useSessions";
import { useTmux } from "@/hooks/useTmux";
import type { AppPage } from "@/lib/session-mappers";
import type { ViewMode } from "@/lib/terminal-types";
import { ThemeContext, getResolvedTheme, toggleTheme as applyThemeToggle, type Theme } from "@/lib/theme";

function App() {
  const [page, setPage] = useState<AppPage>("terminal");
  const [viewMode, setViewMode] = useState<ViewMode>("tabs");
  const [theme, setTheme] = useState<Theme>(() => getResolvedTheme());

  const pingMs = usePing();
  const tmux = useTmux();
  const {
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
  } = useSessions({
    active: page === "terminal",
    theme,
    onTmuxAttached: tmux.refresh,
  });

  const activeSession = groups.flatMap((g) => g.sessions).find((s) => s.id === activeTabId);
  const maximizedSessionData = maximizedSession
    ? groups.flatMap((g) => g.sessions).find((s) => s.id === maximizedSession)
    : null;
  const totalSessions = groups.reduce((sum, g) => sum + g.sessions.length, 0);
  const runningSessions = groups.reduce(
    (sum, g) => sum + g.sessions.filter((s) => s.status === "running").length,
    0,
  );

  const handleActivateTab = (sessionId: string) => {
    setActiveTabId(sessionId);
    clearMaximized();
  };

  return (
    <ThemeContext.Provider value={theme}>
      <div className="flex h-full min-h-0 flex-col overflow-hidden bg-background">
        <AppHeader
          page={page}
          onPageChange={setPage}
          viewMode={viewMode}
          onViewModeChange={setViewMode}
          theme={theme}
          onThemeToggle={() => setTheme(applyThemeToggle(theme))}
          totalSessions={totalSessions}
          runningSessions={runningSessions}
          pingMs={pingMs}
        />

        {page === "terminal" && viewMode === "tabs" && (
          <TabBar
            groups={groups}
            activeTabId={activeTabId}
            onActivate={handleActivateTab}
            onClose={closeSession}
            tmuxAvailable={tmux.available}
            tmuxSessions={tmux.sessions}
            onAddCli={addCli}
            onAddProfileCli={addProfileCli}
            onAttachTmux={attachTmux}
            onRefreshTmux={tmux.refresh}
          />
        )}

        <main className="flex-1 min-h-0 overflow-hidden">
          {page === "chat" ? (
            <ChatView />
          ) : maximizedSessionData ? (
            <div className="h-full p-2">
              <TerminalPanel
                session={maximizedSessionData}
                isActive
                isMaximized
                viewMode={viewMode}
                onToggleMaximize={() => toggleMaximize(maximizedSessionData.id)}
                onClose={() => closeSession(maximizedSessionData.id)}
                onSessionState={(tool, status) =>
                  setSessionState(maximizedSessionData.id, tool, status)
                }
              />
            </div>
          ) : viewMode === "tabs" ? (
            <div className="h-full p-2">
              {activeSession ? (
                <TerminalPanel
                  session={activeSession}
                  isActive
                  viewMode={viewMode}
                  onToggleMaximize={() => toggleMaximize(activeSession.id)}
                  onClose={() => closeSession(activeSession.id)}
                  onSessionState={(tool, status) => setSessionState(activeSession.id, tool, status)}
                />
              ) : sessionsLoading ? (
                <div className="flex h-full items-center justify-center">
                  <p className="text-sm text-muted-foreground/40 font-mono">Loading sessions…</p>
                </div>
              ) : (
                <EmptyTerminalState
                  tmuxAvailable={tmux.available}
                  tmuxSessions={tmux.sessions}
                  onAddCli={addCli}
                  onAddProfileCli={addProfileCli}
                  onAttachTmux={attachTmux}
                  onRefreshTmux={tmux.refresh}
                />
              )}
            </div>
          ) : (
            <TerminalGridView
              groups={groups}
              viewMode={viewMode}
              onToggleMaximize={toggleMaximize}
              onSessionState={setSessionState}
              onCloseSession={closeSession}
            />
          )}
        </main>

        <AppFooter
          viewMode={viewMode}
          runningSessions={runningSessions}
          maximizedSession={maximizedSession}
          onExitMaximized={clearMaximized}
        />
      </div>
    </ThemeContext.Provider>
  );
}

export default App;
