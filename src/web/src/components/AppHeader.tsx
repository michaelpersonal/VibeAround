import { LayoutGrid, MessageSquare, Moon, Rows3, Sun } from "lucide-react";
import { useI18n } from "@va/i18n";

import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import type { Theme } from "@/lib/theme";
import type { ViewMode } from "@/lib/terminal-types";
import type { AppPage } from "@/lib/session-mappers";
import { LanguageMenu } from "./LanguageMenu";

interface AppHeaderProps {
  page: AppPage;
  onPageChange: (page: AppPage) => void;
  viewMode: ViewMode;
  onViewModeChange: (mode: ViewMode) => void;
  theme: Theme;
  onThemeToggle: () => void;
  totalSessions: number;
  runningSessions: number;
  pingMs: number | null;
}

export function AppHeader({
  page,
  onPageChange,
  viewMode,
  onViewModeChange,
  theme,
  onThemeToggle,
  totalSessions,
  runningSessions,
  pingMs,
}: AppHeaderProps) {
  const { t } = useI18n();

  return (
    <header className="flex items-center justify-between px-3 py-1.5 shrink-0 bg-muted/50 dark:bg-background border-b border-border">
      <div className="flex items-center gap-3">
        <div className="flex items-center gap-2">
          <span className="inline-block h-2 w-2 rounded-sm bg-primary" />
          <h1 className="text-xs font-semibold text-foreground font-mono tracking-tight">VibeAround</h1>
        </div>
        <span className="text-[9px] text-muted-foreground/40 font-mono">v0.1.0</span>
        <ToggleGroup
          type="single"
          value={page}
          onValueChange={(v) => v && onPageChange(v as AppPage)}
          className="flex items-center gap-0.5 rounded-md p-0.5 border-l border-border/20 ml-3 pl-3 font-mono text-xs bg-muted/80 dark:bg-muted"
        >
          <ToggleGroupItem
            value="terminal"
            aria-label={t("Terminal")}
            className="rounded px-2 py-1 gap-1.5 data-[state=on]:bg-primary/15 data-[state=on]:text-primary text-muted-foreground/50 hover:text-foreground"
          >
            <Rows3 className="h-3 w-3" />
            {t("Terminal")}
          </ToggleGroupItem>
          <ToggleGroupItem
            value="chat"
            aria-label={t("Chat")}
            className="rounded px-2 py-1 gap-1.5 data-[state=on]:bg-primary/15 data-[state=on]:text-primary text-muted-foreground/50 hover:text-foreground"
          >
            <MessageSquare className="h-3 w-3" />
            {t("Chat")}
          </ToggleGroupItem>
        </ToggleGroup>
        <div
          className={`hidden items-center gap-3 border-l border-border/20 pl-3 sm:flex ${
            page === "terminal" ? "" : "hidden"
          }`}
        >
          <span className="text-[10px] text-muted-foreground/50 font-mono">
            {t("{{running}}/{{total}} active", {
              running: runningSessions,
              total: totalSessions,
            })}
          </span>
          <span className="text-[10px] text-emerald-400/80 font-mono flex items-center gap-1.5">
            <span className="inline-block h-1.5 w-1.5 rounded-full bg-emerald-400 animate-pulse" />
            {t("connected")}
            {pingMs !== null ? (
              <span className="text-muted-foreground/70">· {pingMs} ms</span>
            ) : (
              <span className="text-muted-foreground/50">· — ms</span>
            )}
          </span>
        </div>
      </div>
      <div className="flex items-center gap-1">
        <button
          type="button"
          onClick={onThemeToggle}
          className="rounded-md p-1.5 text-muted-foreground hover:text-foreground hover:bg-accent transition-colors"
          aria-label={theme === "dark" ? t("Switch to light theme") : t("Switch to dark theme")}
        >
          {theme === "dark" ? <Sun className="h-4 w-4" /> : <Moon className="h-4 w-4" />}
        </button>
        <LanguageMenu />
        <ToggleGroup
          type="single"
          value={viewMode}
          onValueChange={(v) => v && onViewModeChange(v as ViewMode)}
          className={`flex items-center gap-0.5 rounded-md p-0.5 font-mono text-xs bg-muted/80 dark:bg-muted ${
            page === "terminal" ? "" : "hidden"
          }`}
        >
          <ToggleGroupItem
            value="tabs"
            aria-label={t("Tab view")}
            className="rounded px-2 py-1 data-[state=on]:bg-primary/15 data-[state=on]:text-primary text-muted-foreground/50 hover:text-foreground"
          >
            <Rows3 className="h-3 w-3" />
          </ToggleGroupItem>
          <ToggleGroupItem
            value="grid"
            aria-label={t("Grid view")}
            className="rounded px-2 py-1 data-[state=on]:bg-primary/15 data-[state=on]:text-primary text-muted-foreground/50 hover:text-foreground"
          >
            <LayoutGrid className="h-3 w-3" />
          </ToggleGroupItem>
        </ToggleGroup>
      </div>
    </header>
  );
}
