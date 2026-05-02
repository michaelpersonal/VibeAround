import { useState, type ReactNode } from "react";
import { Plus } from "lucide-react";

import { getProfiles, type ProfileLaunchOption } from "@/api/sessions";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { TOOL_OPTIONS, type ToolType } from "@/lib/terminal-types";
import { sessionToName } from "@/lib/session-mappers";

interface AddCliDropdownProps {
  /** "top" = inline next to tabs; "empty" = centered in empty state. */
  variant?: "top" | "empty";
  tmuxAvailable: boolean | null;
  tmuxSessions: string[];
  onAddCli: (tool: ToolType) => void;
  onAddProfileCli: (profileId: string, launchTarget: string) => void;
  onAttachTmux: (sessionName: string) => void;
  onRefreshTmux: () => void;
  trigger?: ReactNode;
}

/**
 * Shared "Add CLI" dropdown used in two places — the tab bar and the
 * empty state. Extracted to avoid duplicating ~55 lines of nearly
 * identical markup (the only differences were outer padding and input
 * font-size).
 */
export function AddCliDropdown({
  variant = "top",
  tmuxAvailable,
  tmuxSessions,
  onAddCli,
  onAddProfileCli,
  onAttachTmux,
  onRefreshTmux,
  trigger,
}: AddCliDropdownProps) {
  const [newTmuxName, setNewTmuxName] = useState("");
  const [profiles, setProfiles] = useState<ProfileLaunchOption[]>([]);
  const align = variant === "top" ? "start" : "center";
  const inputFontClass = variant === "top" ? "text-[11px]" : "text-sm";

  const refreshProfiles = () => {
    getProfiles()
      .then((items) => {
        setProfiles(items.filter((profile) => profile.launch_targets.length > 0));
      })
      .catch((e) => console.error("[VibeAround] getProfiles:", e));
  };

  const submitNewTmux = () => {
    const name = newTmuxName.trim();
    if (!name) return;
    onAttachTmux(name);
    setNewTmuxName("");
  };

  const defaultTrigger = (
    <Button
      variant="ghost"
      size="sm"
      className={
        variant === "top"
          ? "h-auto gap-1 px-2.5 py-2 text-[11px] font-mono text-muted-foreground hover:text-foreground"
          : "gap-1.5 font-mono text-xs text-primary hover:bg-primary/10"
      }
      aria-label="Add CLI session"
    >
      <Plus className="h-3.5 w-3.5" />
      Add CLI
    </Button>
  );

  return (
    <DropdownMenu
      onOpenChange={(open) => {
        if (!open) return;
        onRefreshTmux();
        refreshProfiles();
      }}
    >
      <DropdownMenuTrigger asChild>{trigger ?? defaultTrigger}</DropdownMenuTrigger>
      <DropdownMenuContent className="min-w-[140px] font-mono text-xs" align={align}>
        <DropdownMenuLabel className="text-[10px] uppercase tracking-wider text-muted-foreground">
          New session
        </DropdownMenuLabel>
        {TOOL_OPTIONS.map((tool) => (
          <DropdownMenuItem key={tool} onSelect={() => onAddCli(tool)} className="capitalize">
            {sessionToName(tool)}
          </DropdownMenuItem>
        ))}
        {profiles.length > 0 && (
          <>
            <DropdownMenuSeparator />
            <DropdownMenuLabel className="text-[10px] uppercase tracking-wider text-muted-foreground">
              Profiles
            </DropdownMenuLabel>
            {profiles.flatMap((profile) =>
              profile.launch_targets.map((target) => (
                <DropdownMenuItem
                  key={`${profile.id}:${target.id}`}
                  onSelect={() => onAddProfileCli(profile.id, target.id)}
                >
                  {target.label} · {profile.label}
                </DropdownMenuItem>
              )),
            )}
          </>
        )}
        {tmuxAvailable && (
          <>
            <DropdownMenuSeparator />
            <DropdownMenuLabel className="text-[10px] uppercase tracking-wider text-muted-foreground">
              tmux sessions
            </DropdownMenuLabel>
            {tmuxSessions.map((name) => (
              <DropdownMenuItem key={`tmux-${name}`} onSelect={() => onAttachTmux(name)}>
                ⎈ {name}
              </DropdownMenuItem>
            ))}
            <div
              className="flex items-center gap-1 px-2 py-1.5"
              onKeyDown={(e) => e.stopPropagation()}
            >
              <input
                type="text"
                placeholder="session name…"
                value={newTmuxName}
                onChange={(e) => setNewTmuxName(e.target.value)}
                onKeyDown={(e) => {
                  e.stopPropagation();
                  if (e.key === "Enter") submitNewTmux();
                }}
                className={`flex-1 min-w-0 bg-transparent border border-border/40 rounded px-1.5 py-0.5 ${inputFontClass} font-mono text-foreground placeholder:text-muted-foreground/40 outline-none focus:border-primary/50`}
              />
              <Button
                variant="ghost"
                size="icon-xs"
                className="shrink-0 h-5 w-5 text-muted-foreground/60 hover:text-primary"
                disabled={!newTmuxName.trim()}
                onClick={submitNewTmux}
              >
                <Plus className="h-3 w-3" />
              </Button>
            </div>
          </>
        )}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
