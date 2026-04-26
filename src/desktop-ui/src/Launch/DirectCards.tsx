import { useEffect, useState } from "react";
import { Play, Sparkles } from "lucide-react";

import { listAgents, type AgentSummary } from "./api";

interface Props {
  onLaunch: (agentId: string) => Promise<void>;
  busy: boolean;
}

/**
 * "直接启动" — fire any of the coder CLIs registered in `agents.json`
 * with no env at all. The CLI does its own thing (OAuth, cached token,
 * provider config, …); VibeAround does not touch credentials.
 *
 * Buttons are populated dynamically from the daemon's agents.json so
 * adding a new CLI is a one-file edit on the agents side; this card
 * picks it up automatically without a UI release.
 */
export function DirectCards({ onLaunch, busy }: Props) {
  const [agents, setAgents] = useState<AgentSummary[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    void listAgents()
      .then(setAgents)
      .catch((e) => setError(e instanceof Error ? e.message : String(e)));
  }, []);

  return (
    <div className="border border-border rounded-md p-3 flex flex-col gap-2">
      <div className="flex items-start gap-2">
        <span className="w-7 h-7 rounded bg-primary/10 text-primary flex items-center justify-center shrink-0">
          <Sparkles className="w-3.5 h-3.5" />
        </span>
        <div className="flex-1 min-w-0">
          <div className="text-sm font-medium">Direct launch</div>
          <div className="text-[11px] text-muted-foreground truncate">
            No profile — uses each CLI's existing login session
          </div>
        </div>
      </div>
      {error && <div className="text-[11px] text-destructive">{error}</div>}
      {agents === null && !error ? (
        <div className="text-[11px] text-muted-foreground">Loading…</div>
      ) : (
        <div className="flex flex-wrap gap-1.5 mt-1">
          {(agents ?? []).map((a) => (
            <button
              key={a.id}
              type="button"
              onClick={() => onLaunch(a.id)}
              disabled={busy}
              className="flex items-center gap-1 px-2 py-1 rounded text-[11px] font-mono bg-primary/10 text-primary hover:bg-primary/20 disabled:opacity-50 transition-colors"
              title={`${a.display_name} — ${a.description}`}
            >
              <Play className="w-3 h-3" />
              {a.id}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
