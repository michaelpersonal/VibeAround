import { Bot, Check, FolderOpen, Plus, Sparkles, Star } from "lucide-react";

import { Button } from "@/components/ui/button";
import { BrandIcon } from "@/components/brand-icon";
import type { AgentSummary, StepAgentsProps } from "../types";

const RECOMMENDED_AGENT_IDS = new Set(["claude", "codex"]);
const NO_HANDOVER_AGENTS = new Set(["opencode"]);
const DIRECT_VALUE = "__direct__";

export function StepAgents({
  agents,
  profiles,
  enabled,
  defaultAgent,
  defaultProfiles,
  onToggle,
  onSetDefault,
  onSetDefaultProfile,
  onCreateProfile,
}: StepAgentsProps) {
  const recommended = agents.filter((agent) => RECOMMENDED_AGENT_IDS.has(agent.id));
  const others = agents.filter((agent) => !RECOMMENDED_AGENT_IDS.has(agent.id));
  const profileOptions = profiles.map((profile) => ({
    profile,
    compatible: profile.launchTargets.some((target) => target.id === defaultAgent),
  }));
  const selectedProfile = defaultProfiles[defaultAgent] ?? DIRECT_VALUE;
  const selectedProfileIsVisible = profileOptions.some(
    ({ profile, compatible }) => compatible && profile.id === selectedProfile,
  );
  const activeProfile = selectedProfileIsVisible ? selectedProfile : DIRECT_VALUE;

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

      <section className="rounded-lg border border-border bg-card p-3 space-y-3">
        <div className="flex items-center gap-2 text-xs font-medium">
          <Star className="w-3.5 h-3.5 text-primary" />
          Best recommended
        </div>
        <AgentGrid
          agents={recommended}
          enabled={enabled}
          defaultAgent={defaultAgent}
          onToggle={onToggle}
          onSetDefault={onSetDefault}
        />
      </section>

      {others.length > 0 && (
        <section className="rounded-lg border border-border bg-card p-3 space-y-3">
          <div className="text-xs font-medium text-muted-foreground">Other CLIs</div>
          <AgentGrid
            agents={others}
            enabled={enabled}
            defaultAgent={defaultAgent}
            onToggle={onToggle}
            onSetDefault={onSetDefault}
          />
        </section>
      )}

      <section className="rounded-lg border border-border bg-card p-3 space-y-3">
        <div className="flex items-center justify-between gap-3">
          <div>
            <div className="text-xs font-medium">Default API profile</div>
            <p className="text-[11px] text-muted-foreground mt-0.5">
              Optional. Direct launch uses the CLI's existing login.
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

        <div className="grid grid-cols-[repeat(auto-fill,minmax(178px,220px))] gap-2">
          <button
            type="button"
            onClick={() => onSetDefaultProfile(defaultAgent, null)}
            className={`flex min-h-[54px] items-center gap-2 rounded-md border p-2 text-left transition-colors ${
              activeProfile === DIRECT_VALUE
                ? "border-primary/40 bg-primary/5"
                : "border-border hover:border-border/80"
            }`}
          >
            <span className="flex h-7 w-7 shrink-0 items-center justify-center rounded-md border border-border/70 bg-card text-primary">
              <Sparkles className="h-3.5 w-3.5" />
            </span>
            <span className="min-w-0 flex-1">
              <span className="block truncate text-[13px] font-medium">Direct launch</span>
              <span className="block truncate text-[10px] text-muted-foreground">
                Existing CLI login
              </span>
            </span>
            {activeProfile === DIRECT_VALUE && (
              <Check className="h-3.5 w-3.5 shrink-0 text-primary" />
            )}
          </button>

          {profileOptions.map(({ profile, compatible }) => {
            const selected = activeProfile === profile.id;
            return (
              <button
                key={profile.id}
                type="button"
                disabled={!compatible}
                onClick={() => onSetDefaultProfile(defaultAgent, profile.id)}
                title={
                  compatible
                    ? `Use ${profile.label} with ${defaultAgent}`
                    : `${profile.label} does not support ${defaultAgent}`
                }
                className={`flex min-h-[54px] items-center gap-2 rounded-md border p-2 text-left transition-colors disabled:cursor-not-allowed disabled:opacity-55 ${
                  selected
                    ? "border-primary/40 bg-primary/5"
                    : "border-border hover:border-border/80"
                }`}
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
                    {compatible ? profile.providerLabel : `Not for ${defaultAgent}`}
                  </span>
                </span>
                {selected && <Check className="h-3.5 w-3.5 shrink-0 text-primary" />}
              </button>
            );
          })}
        </div>

        {profileOptions.length === 0 && (
          <p className="text-[11px] text-muted-foreground">
            No saved profile supports {defaultAgent} yet.
          </p>
        )}
      </section>

      <section className="rounded-lg border border-border bg-card p-3">
        <div className="flex items-center gap-2">
          <FolderOpen className="w-4 h-4 text-primary" />
          <div>
            <div className="text-xs font-medium">Default workspace</div>
            <div className="text-[11px] text-muted-foreground font-mono">
              ~/.vibearound/workspaces
            </div>
          </div>
        </div>
      </section>
    </div>
  );
}

function AgentGrid({
  agents,
  enabled,
  defaultAgent,
  onToggle,
  onSetDefault,
}: {
  agents: AgentSummary[];
  enabled: Set<string>;
  defaultAgent: string;
  onToggle: (id: string) => void;
  onSetDefault: (id: string) => void;
}) {
  return (
    <div className="grid grid-cols-[repeat(auto-fill,minmax(178px,220px))] gap-2">
      {agents.map((agent) => {
        const isEnabled = enabled.has(agent.id);
        const isDefault = defaultAgent === agent.id;
        const noHandover = NO_HANDOVER_AGENTS.has(agent.id);
        return (
          <div
            key={agent.id}
            className={`relative flex min-h-[68px] cursor-pointer gap-2 rounded-md border p-2 pr-8 text-left transition-colors ${
              isEnabled
                ? "border-primary/40 bg-primary/5"
                : "border-border hover:border-border/80"
            }`}
            onClick={() => onToggle(agent.id)}
          >
            <BrandIcon
              kind="cli"
              id={agent.id}
              label={agent.display_name}
              className="h-7 w-7"
            />
            <div className="min-w-0 flex-1">
              <div
                className={`truncate text-[13px] font-medium ${
                  isEnabled ? "text-foreground" : "text-muted-foreground"
                }`}
              >
                {agent.display_name}
              </div>
              {noHandover && (
                <div className="mt-1">
                  <span className="rounded bg-muted px-1 py-0.5 font-mono text-[9px] leading-none text-muted-foreground/60">
                    no handover
                  </span>
                </div>
              )}
              {isEnabled && (
                <button
                  type="button"
                  onClick={(event) => {
                    event.stopPropagation();
                    onSetDefault(agent.id);
                  }}
                  className={`mt-2 rounded px-1.5 py-0.5 font-mono text-[10px] leading-none transition-colors ${
                    isDefault
                      ? "bg-primary text-primary-foreground"
                      : "bg-muted text-muted-foreground hover:bg-accent"
                  }`}
                >
                  {isDefault ? "default" : "set default"}
                </button>
              )}
            </div>
            <div
              className={`absolute right-2.5 top-2.5 flex h-4 w-4 shrink-0 items-center justify-center rounded border transition-colors ${
                isEnabled ? "border-primary bg-primary" : "border-muted-foreground/30"
              }`}
            >
              {isEnabled && <Check className="h-3 w-3 text-primary-foreground" />}
            </div>
          </div>
        );
      })}
    </div>
  );
}
