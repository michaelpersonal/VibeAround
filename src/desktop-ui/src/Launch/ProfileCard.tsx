import { useState } from "react";
import { AlertTriangle, MoreVertical, Pencil, Play, Trash2 } from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import type { ProfileSummary } from "./types";
import { apiTypeBadge, apiTypeShort } from "./types";

interface Props {
  profile: ProfileSummary;
  onLaunch: (launchTarget: string) => Promise<void>;
  onEdit: () => void;
  onDelete: () => Promise<void>;
}

export function ProfileCard({ profile, onLaunch, onEdit, onDelete }: Props) {
  const [busy, setBusy] = useState(false);

  async function handleLaunch(launchTarget: string) {
    setBusy(true);
    try {
      await onLaunch(launchTarget);
    } finally {
      setBusy(false);
    }
  }

  async function handleDelete() {
    if (!window.confirm(`Delete profile "${profile.label}"?`)) return;
    setBusy(true);
    try {
      await onDelete();
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="border border-border rounded-md p-2.5 flex flex-col gap-1.5 hover:border-primary/40 transition-colors">
      <div className="flex items-start gap-2">
        {profile.providerIcon && (
          <span className="text-base shrink-0">{profile.providerIcon}</span>
        )}
        <div className="flex-1 min-w-0">
          <div className="text-[13px] font-medium truncate">{profile.label}</div>
          <div className="text-[11px] text-muted-foreground truncate">
            {profile.providerLabel}
          </div>
        </div>
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button
              type="button"
              variant="ghost"
              size="icon-xs"
              className="shrink-0 text-muted-foreground"
              aria-label="More"
            >
              <MoreVertical className="w-3.5 h-3.5" />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end" className="w-32">
            <DropdownMenuItem className="text-xs" onSelect={onEdit}>
              <Pencil className="w-3 h-3" /> Edit
            </DropdownMenuItem>
            <DropdownMenuItem
              className="text-xs"
              variant="destructive"
              onSelect={() => {
                void handleDelete();
              }}
            >
              <Trash2 className="w-3 h-3" /> Delete
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      </div>

      <div className="flex flex-wrap gap-1.5 mt-1">
        {profile.launchTargets.map((target) => {
          const warning = target.warning ?? profile.apiTypeWarnings[target.apiType];
          return (
            <Button
              key={target.id}
              type="button"
              variant="secondary"
              size="xs"
              onClick={() => handleLaunch(target.id)}
              disabled={busy}
              className="h-7 font-mono text-[11px] bg-primary/10 text-primary hover:bg-primary/20"
              title={
                warning
                  ? `⚠ ${warning}\n\n(Click to launch ${target.label} via ${apiTypeShort(target.apiType)} anyway.)`
                  : `Launch ${target.label} via ${apiTypeShort(target.apiType)}`
              }
            >
              <Play className="w-3 h-3" />
              <span>{target.label}</span>
              <Badge className="border-0 bg-transparent p-0 text-[11px] text-primary/55">
                · {apiTypeBadge(target.apiType)}
              </Badge>
              {warning && <AlertTriangle className="w-3 h-3 text-amber-500" />}
            </Button>
          );
        })}
      </div>
    </div>
  );
}
