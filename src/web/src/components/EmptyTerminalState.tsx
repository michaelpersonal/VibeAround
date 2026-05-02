import type { ToolType } from "@/lib/terminal-types";
import { AddCliDropdown } from "./AddCliDropdown";

interface EmptyTerminalStateProps {
  tmuxAvailable: boolean | null;
  tmuxSessions: string[];
  onAddCli: (tool: ToolType) => void;
  onAddProfileCli: (profileId: string, launchTarget: string) => void;
  onAttachTmux: (name: string) => void;
  onRefreshTmux: () => void;
}

export function EmptyTerminalState({
  tmuxAvailable,
  tmuxSessions,
  onAddCli,
  onAddProfileCli,
  onAttachTmux,
  onRefreshTmux,
}: EmptyTerminalStateProps) {
  return (
    <div className="flex h-full flex-col items-center justify-center gap-3">
      <p className="text-sm text-muted-foreground/40 font-mono">No sessions yet. Add a CLI to start.</p>
      <AddCliDropdown
        variant="empty"
        tmuxAvailable={tmuxAvailable}
        tmuxSessions={tmuxSessions}
        onAddCli={onAddCli}
        onAddProfileCli={onAddProfileCli}
        onAttachTmux={onAttachTmux}
        onRefreshTmux={onRefreshTmux}
      />
    </div>
  );
}
