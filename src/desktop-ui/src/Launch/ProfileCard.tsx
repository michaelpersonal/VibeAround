import { useState } from "react";
import type { Ref } from "react";
import { AlertTriangle, GripVertical, MoreVertical, Pencil, Star, Trash2 } from "lucide-react";

import { BrandIcon } from "@/components/brand-icon";
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
  onSetDefault: (launchTarget: string) => Promise<void>;
  onEdit: () => void;
  onDelete: () => Promise<void>;
  defaultAgent?: string;
  defaultProfiles?: Record<string, string>;
  dragHandleRef?: Ref<HTMLDivElement>;
  dragHandleDisabled?: boolean;
  isDragging?: boolean;
}

export function ProfileCard({
  profile,
  onLaunch,
  onSetDefault,
  onEdit,
  onDelete,
  defaultAgent,
  defaultProfiles = {},
  dragHandleRef,
  dragHandleDisabled = false,
  isDragging = false,
}: Props) {
  const [busy, setBusy] = useState(false);
  const [defaultBusy, setDefaultBusy] = useState<string | null>(null);
  const isDefaultProfile = profile.launchTargets.some(
    (target) => defaultAgent === target.id && defaultProfiles[target.id] === profile.id,
  );

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

  async function handleSetDefault(launchTarget: string) {
    setBusy(true);
    try {
      await onSetDefault(launchTarget);
    } finally {
      setBusy(false);
    }
  }

  return (
    <div
      className={`border rounded-md p-2.5 flex flex-col gap-1.5 transition-colors ${
        isDefaultProfile
          ? "border-emerald-500/70 bg-emerald-500/5 hover:border-emerald-500"
          : "border-border bg-card hover:border-primary/40"
      } ${
        isDragging ? "opacity-55" : ""
      }`}
    >
      <div className="flex items-start gap-2">
        {dragHandleRef && (
          <div
            ref={dragHandleRef}
            role="button"
            tabIndex={0}
            aria-label={`Reorder ${profile.label}`}
            aria-disabled={dragHandleDisabled}
            className={`mt-0.5 h-7 w-5 shrink-0 rounded text-muted-foreground/60 hover:bg-accent hover:text-foreground inline-flex items-center justify-center select-none ${
              dragHandleDisabled ? "cursor-not-allowed opacity-40" : "cursor-grab active:cursor-grabbing"
            }`}
          >
            <GripVertical className="w-3.5 h-3.5" />
          </div>
        )}
        <BrandIcon
          kind="provider"
          id={profile.provider}
          label={profile.providerLabel}
          fallback={profile.providerIcon}
          className="h-7 w-7"
        />
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
          const isDefault =
            defaultAgent === target.id && defaultProfiles[target.id] === profile.id;
          return (
            <span key={target.id} className="inline-flex h-7 overflow-hidden rounded-md bg-primary/10 text-primary">
              <Button
                type="button"
                variant="ghost"
                size="xs"
                onClick={() => handleLaunch(target.id)}
                disabled={busy}
                className={`h-7 rounded-none bg-transparent px-2 font-mono text-[11px] text-primary hover:bg-primary/15 hover:text-primary ${
                  isDefault ? "" : "pr-1.5"
                }`}
                title={
                  warning
                    ? `⚠ ${warning}\n\n(Click to launch ${target.label} via ${apiTypeShort(target.apiType)} anyway.)`
                    : `Launch ${target.label} via ${apiTypeShort(target.apiType)}`
                }
              >
                <BrandIcon
                  kind="cli"
                  id={target.id}
                  label={target.label}
                  framed={false}
                  className="h-3.5 w-3.5"
                />
                <span>{target.label}</span>
                <Badge className="border-0 bg-transparent p-0 text-[11px] text-primary/55">
                  · {apiTypeBadge(target.apiType)}
                </Badge>
                {isDefault && <Star className="w-3 h-3 fill-current" />}
                {warning && <AlertTriangle className="w-3 h-3 text-amber-500" />}
              </Button>
              {!isDefault && (
                <Button
                  type="button"
                  variant="ghost"
                  size="icon-xs"
                  disabled={busy || defaultBusy === target.id}
                  onClick={async () => {
                    setDefaultBusy(target.id);
                    try {
                      await handleSetDefault(target.id);
                    } finally {
                      setDefaultBusy(null);
                    }
                  }}
                  title={`Use ${target.label} with ${profile.label} as Quick Launch default`}
                  className="h-7 w-6 rounded-none border-l border-primary/15 bg-transparent text-primary/60 hover:bg-primary/15 hover:text-primary"
                >
                  <Star className="w-3 h-3" />
                </Button>
              )}
            </span>
          );
        })}
      </div>
    </div>
  );
}
