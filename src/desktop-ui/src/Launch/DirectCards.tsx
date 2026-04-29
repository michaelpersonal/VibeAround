import { useEffect, useState } from "react";
import { Sparkles, Star } from "lucide-react";

import { BrandIcon } from "@/components/brand-icon";
import { Button } from "@/components/ui/button";
import { listAgents, type AgentSummary } from "./api";

const AGENT_DISPLAY_ORDER = ["claude", "codex", "gemini", "opencode", "cursor", "kiro", "qwen-code"];

interface Props {
  onLaunch: (agentId: string) => Promise<void>;
  onSetDefault: (agentId: string) => Promise<void>;
  busy: boolean;
  defaultAgent?: string;
  defaultProfiles?: Record<string, string>;
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
export function DirectCards({
  onLaunch,
  onSetDefault,
  busy,
  defaultAgent,
  defaultProfiles = {},
}: Props) {
  const [agents, setAgents] = useState<AgentSummary[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [defaultBusy, setDefaultBusy] = useState<string | null>(null);

  useEffect(() => {
    void listAgents()
      .then((items) => {
        const rank = new Map(AGENT_DISPLAY_ORDER.map((id, index) => [id, index]));
        setAgents([...items].sort((a, b) => (rank.get(a.id) ?? 999) - (rank.get(b.id) ?? 999)));
      })
      .catch((e) => setError(e instanceof Error ? e.message : String(e)));
  }, []);

  return (
    <div className="border border-border rounded-md p-2.5 flex flex-col gap-1.5">
      <div className="flex items-start gap-2">
        <span className="w-7 h-7 rounded bg-primary/10 text-primary flex items-center justify-center shrink-0">
          <Sparkles className="w-3.5 h-3.5" />
        </span>
        <div className="flex-1 min-w-0">
          <div className="text-[13px] font-medium">Direct launch</div>
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
          {(agents ?? []).map((a) => {
            const isDefault = defaultAgent === a.id && !defaultProfiles[a.id];
            return (
              <span key={a.id} className="inline-flex h-7 overflow-hidden rounded-md bg-primary/10 text-primary">
                <Button
                  type="button"
                  variant="ghost"
                  size="xs"
                  onClick={() => onLaunch(a.id)}
                  disabled={busy}
                  className={`h-7 rounded-none bg-transparent px-2 text-[11px] text-primary hover:bg-primary/15 hover:text-primary ${
                    isDefault ? "" : "pr-1.5"
                  }`}
                  title={`${a.display_name} — ${a.description}`}
                >
                  <BrandIcon
                    kind="cli"
                    id={a.id}
                    label={a.display_name}
                    framed={false}
                    className="h-3.5 w-3.5"
                  />
                  {a.display_name}
                  {isDefault && <Star className="w-3 h-3 fill-current" />}
                </Button>
                {!isDefault && (
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon-xs"
                    disabled={busy || defaultBusy === a.id}
                    onClick={async () => {
                      setDefaultBusy(a.id);
                      try {
                        await onSetDefault(a.id);
                      } finally {
                        setDefaultBusy(null);
                      }
                    }}
                    title={`Use ${a.display_name} as Quick Launch default without a profile`}
                    className="h-7 w-6 rounded-none border-l border-primary/15 bg-transparent text-primary/60 hover:bg-primary/15 hover:text-primary"
                  >
                    <Star className="w-3 h-3" />
                  </Button>
                )}
              </span>
            );
          })}
        </div>
      )}
    </div>
  );
}
