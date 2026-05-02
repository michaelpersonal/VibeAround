import {
  ChevronDown,
  ChevronRight,
  Check,
  CircleDot,
  Loader2,
  Minus,
  Rocket,
  X,
} from "lucide-react";
import { useState } from "react";

import { Button } from "@/components/ui/button";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible";

import type { InstallTaskProgress, StepConfirmProps } from "../types";

export function StepConfirm({
  agents,
  tunnels,
  pluginRegistry,
  enabledAgents,
  tunnelProvider,
  enabledChannels,
  isInstalling,
  installComplete,
  installTasks,
}: StepConfirmProps) {
  if (isInstalling) {
    return (
      <InstallProgressView
        tasks={installTasks}
        complete={installComplete}
      />
    );
  }

  const agentLabels = new Map(agents.map((a) => [a.id, a.display_name]));
  const tunnelLabels = new Map(tunnels.map((t) => [t.id, t.display_name]));

  const agentSummary = Array.from(enabledAgents)
    .map((id) => agentLabels.get(id) ?? id)
    .join(", ");

  const channelNames = Array.from(enabledChannels).map((id) => {
    const registry = pluginRegistry.find((p) => p.id === id);
    return registry?.name ?? id;
  });

  return (
    <div className="space-y-4">
      <div>
        <h2 className="text-base font-semibold flex items-center gap-2">
          <Rocket className="w-4 h-4 text-primary" />
          Ready to Launch
        </h2>
        <p className="text-xs text-muted-foreground mt-1">
          Review your configuration. You can always change these in
          settings.json later.
        </p>
      </div>

      <div className="space-y-2 text-sm">
        <SummaryRow
          label="Quick Launch"
          value={`${agentLabels.get("claude") ?? "Claude Code"} · Direct launch`}
        />
        <SummaryRow label="Workspace" value="~/.vibearound/workspaces" />
        <SummaryRow label="Agents" value={agentSummary} />
        <SummaryRow
          label="Channels"
          value={
            channelNames.length > 0
              ? channelNames.join(", ")
              : "None configured"
          }
        />
        <SummaryRow
          label="Tunnel"
          value={tunnelLabels.get(tunnelProvider) ?? tunnelProvider}
        />
      </div>

      <p className="text-[11px] text-muted-foreground mt-3 leading-relaxed">
        VibeAround will add an MCP server entry to your coding agents' global
        settings and install a handover skill for session transfer between
        devices. Your existing agent settings will not be overwritten.
      </p>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Install progress list
// ---------------------------------------------------------------------------

function InstallProgressView({
  tasks,
  complete,
}: {
  tasks: InstallTaskProgress[];
  complete: boolean;
}) {
  const hasErrors = tasks.some((t) => t.status === "error");
  const hasCancelled = tasks.some((t) => t.status === "cancelled");

  return (
    <div className="space-y-4">
      <div>
        <h2 className="text-base font-semibold flex items-center gap-2">
          {complete ? (
            hasErrors ? (
              <X className="w-4 h-4 text-destructive" />
            ) : (
              <Check className="w-4 h-4 text-primary" />
            )
          ) : (
            <Loader2 className="w-4 h-4 text-primary animate-spin" />
          )}
          {complete
            ? hasCancelled
              ? "Installation Cancelled"
              : hasErrors
                ? "Installation Completed with Errors"
                : "Installation Complete"
            : "Installing VibeAround"}
        </h2>
        <p className="text-xs text-muted-foreground mt-1">
          {complete
            ? "Review the results below."
            : "Setting up your agents and plugins..."}
        </p>
      </div>

      <div className="space-y-1">
        {tasks.map((task) => (
          <TaskRow key={task.id} task={task} />
        ))}
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Individual task row
// ---------------------------------------------------------------------------

function TaskRow({ task }: { task: InstallTaskProgress }) {
  const [expanded, setExpanded] = useState(false);
  const logs = task.logs ?? [];
  const hasLogs = logs.length > 0;
  const latest = task.message ?? logs.at(-1);

  return (
    <Collapsible
      open={expanded}
      onOpenChange={setExpanded}
      className="rounded-md bg-muted/30"
    >
      <div className="flex items-start gap-2.5 py-2 px-3">
        <div className="mt-0.5 shrink-0">
          <StatusIcon status={task.status} />
        </div>
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-2">
            <span
              className={`text-sm ${
                task.status === "done" || task.status === "skipped"
                  ? "text-muted-foreground"
                  : task.status === "error"
                    ? "text-destructive"
                    : "text-foreground"
              }`}
            >
              {task.label}
            </span>
          </div>
          {latest && (
            <p
              className={`text-[11px] mt-0.5 leading-relaxed truncate ${
                task.status === "error"
                  ? "text-destructive/80"
                  : "text-muted-foreground"
              }`}
              title={latest}
            >
              {latest}
            </p>
          )}
        </div>
        {hasLogs && (
          <CollapsibleTrigger asChild>
            <Button
              type="button"
              variant="ghost"
              size="icon-xs"
              className="mt-0.5 shrink-0 text-muted-foreground hover:text-foreground"
              aria-label={expanded ? "Collapse install log" : "Expand install log"}
            >
              {expanded ? (
                <ChevronDown className="h-3.5 w-3.5" />
              ) : (
                <ChevronRight className="h-3.5 w-3.5" />
              )}
            </Button>
          </CollapsibleTrigger>
        )}
      </div>

      <CollapsibleContent>
        <pre className="mx-3 mb-3 max-h-64 overflow-auto whitespace-pre-wrap rounded-md border border-border bg-background px-3 py-2 text-[11px] leading-relaxed text-muted-foreground">
          {logs.join("\n\n")}
        </pre>
      </CollapsibleContent>
    </Collapsible>
  );
}

// ---------------------------------------------------------------------------
// Status icon
// ---------------------------------------------------------------------------

function StatusIcon({ status }: { status: InstallTaskProgress["status"] }) {
  switch (status) {
    case "pending":
      return <CircleDot className="w-3.5 h-3.5 text-muted-foreground/40" />;
    case "running":
      return (
        <Loader2 className="w-3.5 h-3.5 text-primary animate-spin" />
      );
    case "done":
      return <Check className="w-3.5 h-3.5 text-green-500" />;
    case "skipped":
      return <Minus className="w-3.5 h-3.5 text-muted-foreground/50" />;
    case "error":
      return <X className="w-3.5 h-3.5 text-destructive" />;
    case "cancelled":
      return <Minus className="w-3.5 h-3.5 text-muted-foreground/50" />;
  }
}

// ---------------------------------------------------------------------------
// Summary row (used in pre-install review)
// ---------------------------------------------------------------------------

function SummaryRow({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex items-start gap-3 py-2 px-3 rounded-md bg-muted/40">
      <span className="text-xs text-muted-foreground w-20 shrink-0 pt-0.5">
        {label}
      </span>
      <span className="text-sm">{value}</span>
    </div>
  );
}
