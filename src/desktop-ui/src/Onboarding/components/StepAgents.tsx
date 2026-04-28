import { Bot, FolderOpen, Plus, Star, Trash2 } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { Checkbox } from "@/components/ui/checkbox";
import { BrandIcon } from "@/components/brand-icon";
import type { AgentSummary, StepAgentsProps } from "../types";

const RECOMMENDED_AGENT_IDS = new Set(["claude", "codex"]);

export function StepAgents({
  agents,
  profiles,
  enabled,
  onToggle,
  onCreateProfile,
  onDeleteProfile,
}: StepAgentsProps) {
  const recommended = agents.filter((agent) => RECOMMENDED_AGENT_IDS.has(agent.id));
  const others = agents.filter((agent) => !RECOMMENDED_AGENT_IDS.has(agent.id));

  return (
    <div className="space-y-4">
      <div>
        <h2 className="text-base font-semibold flex items-center gap-2">
          <Bot className="w-4 h-4 text-primary" />
          Quick Launch
        </h2>
        <p className="text-xs text-muted-foreground mt-1">
          Pick the CLI VibeAround should start from Launch and IM messages.
        </p>
      </div>

      <Card className="p-3 space-y-3">
        <div className="flex items-center gap-2 text-xs font-medium">
          <Star className="w-3.5 h-3.5 text-primary" />
          Best recommended
        </div>
        <AgentGrid
          agents={recommended}
          enabled={enabled}
          onToggle={onToggle}
        />
      </Card>

      {others.length > 0 && (
        <Card className="p-3 space-y-3">
          <div className="text-xs font-medium text-muted-foreground">Other CLIs</div>
          <AgentGrid
            agents={others}
            enabled={enabled}
            onToggle={onToggle}
          />
        </Card>
      )}

      <Card className="p-3 space-y-3">
        <div className="flex items-center justify-between gap-3">
          <div>
            <div className="text-xs font-medium">API profiles</div>
            <p className="text-[11px] text-muted-foreground mt-0.5">
              Optional. Save API keys now; choose launch defaults later in Launch.
            </p>
          </div>
          <Button
            type="button"
            size="sm"
            variant="outline"
            className="h-8 text-xs"
            onClick={onCreateProfile}
          >
            <Plus className="w-3 h-3" />
            Add API profile
          </Button>
        </div>

        {profiles.length > 0 ? (
          <div className="grid grid-cols-[repeat(auto-fill,minmax(178px,220px))] gap-2">
            {profiles.map((profile) => (
              <div
                key={profile.id}
                className="flex min-h-[54px] items-center gap-2 rounded-md border border-border p-2 text-left"
              >
                <BrandIcon
                  kind="provider"
                  id={profile.provider}
                  label={profile.providerLabel}
                  fallback={profile.providerIcon}
                  className="h-7 w-7"
                />
                <span className="min-w-0 flex-1">
                  <span className="block truncate text-[13px] font-medium">
                    {profile.label}
                  </span>
                  <span className="block truncate text-[10px] text-muted-foreground">
                    {profile.providerLabel}
                  </span>
                </span>
                <Button
                  type="button"
                  variant="ghost"
                  size="icon-xs"
                  className="h-7 w-7 shrink-0 text-muted-foreground hover:text-destructive"
                  title={`Delete ${profile.label}`}
                  onClick={() => onDeleteProfile(profile.id)}
                >
                  <Trash2 className="h-3.5 w-3.5" />
                </Button>
              </div>
            ))}
          </div>
        ) : (
          <p className="text-[11px] text-muted-foreground">
            No API profiles yet. You can add one now or skip this step.
          </p>
        )}
      </Card>

      <Card className="p-3">
        <div className="flex items-center gap-2">
          <FolderOpen className="w-4 h-4 text-primary" />
          <div>
            <div className="text-xs font-medium">Default workspace</div>
            <div className="text-[11px] text-muted-foreground font-mono">
              ~/.vibearound/workspaces
            </div>
          </div>
        </div>
      </Card>
    </div>
  );
}

function AgentGrid({
  agents,
  enabled,
  onToggle,
}: {
  agents: AgentSummary[];
  enabled: Set<string>;
  onToggle: (id: string) => void;
}) {
  return (
    <div className="grid grid-cols-[repeat(auto-fill,minmax(178px,220px))] gap-2">
      {agents.map((agent) => {
        const isEnabled = enabled.has(agent.id);
        return (
          <div
            key={agent.id}
            role="checkbox"
            aria-checked={isEnabled}
            tabIndex={0}
            className={`relative flex min-h-[54px] cursor-pointer items-center gap-2 rounded-md border p-2 pr-8 text-left transition-colors ${
              isEnabled
                ? "border-primary/40 bg-primary/5"
                : "border-border hover:border-border/80"
            }`}
            onClick={() => onToggle(agent.id)}
            onKeyDown={(event) => {
              if (event.key === "Enter" || event.key === " ") {
                event.preventDefault();
                onToggle(agent.id);
              }
            }}
          >
            <BrandIcon
              kind="cli"
              id={agent.id}
              label={agent.display_name}
              className="h-7 w-7"
            />
            <div className="flex min-w-0 flex-1 items-center">
              <div
                className={`truncate text-[13px] font-medium ${
                  isEnabled ? "text-foreground" : "text-muted-foreground"
                }`}
              >
                {agent.display_name}
              </div>
            </div>
            <Checkbox
              checked={isEnabled}
              aria-hidden="true"
              tabIndex={-1}
              className="pointer-events-none absolute right-2.5 top-1/2 -translate-y-1/2"
            />
          </div>
        );
      })}
    </div>
  );
}
