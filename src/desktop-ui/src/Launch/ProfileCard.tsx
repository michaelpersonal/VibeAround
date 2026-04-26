import { useState } from "react";
import { AlertTriangle, MoreVertical, Pencil, Play, Trash2 } from "lucide-react";

import type { ProfileSummary } from "./types";
import { apiTypeShort } from "./types";

interface Props {
  profile: ProfileSummary;
  onLaunch: (apiType: string) => Promise<void>;
  onEdit: () => void;
  onDelete: () => Promise<void>;
}

export function ProfileCard({ profile, onLaunch, onEdit, onDelete }: Props) {
  const [menuOpen, setMenuOpen] = useState(false);
  const [busy, setBusy] = useState(false);

  async function handleLaunch(apiType: string) {
    setBusy(true);
    try {
      await onLaunch(apiType);
    } finally {
      setBusy(false);
    }
  }

  async function handleDelete() {
    setMenuOpen(false);
    if (!window.confirm(`Delete profile "${profile.label}"?`)) return;
    setBusy(true);
    try {
      await onDelete();
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="border border-border rounded-md p-3 flex flex-col gap-2 hover:border-primary/40 transition-colors">
      <div className="flex items-start gap-2">
        {profile.providerIcon && (
          <span className="text-base shrink-0">{profile.providerIcon}</span>
        )}
        <div className="flex-1 min-w-0">
          <div className="text-sm font-medium truncate">{profile.label}</div>
          <div className="text-[11px] text-muted-foreground truncate">
            {profile.providerLabel}
          </div>
        </div>
        <div className="relative shrink-0">
          <button
            type="button"
            onClick={() => setMenuOpen((v) => !v)}
            className="p-1 rounded hover:bg-accent text-muted-foreground"
            aria-label="More"
          >
            <MoreVertical className="w-3.5 h-3.5" />
          </button>
          {menuOpen && (
            <>
              {/* Click-away catcher */}
              <div
                className="fixed inset-0 z-10"
                onClick={() => setMenuOpen(false)}
                aria-hidden="true"
              />
              <div className="absolute right-0 top-full mt-1 z-20 bg-popover border border-border rounded shadow-md text-xs py-1 w-32">
                <button
                  type="button"
                  onClick={() => {
                    setMenuOpen(false);
                    onEdit();
                  }}
                  className="flex items-center gap-2 w-full px-2.5 py-1.5 hover:bg-accent text-left"
                >
                  <Pencil className="w-3 h-3" /> Edit
                </button>
                <button
                  type="button"
                  onClick={handleDelete}
                  className="flex items-center gap-2 w-full px-2.5 py-1.5 hover:bg-accent text-destructive text-left"
                >
                  <Trash2 className="w-3 h-3" /> Delete
                </button>
              </div>
            </>
          )}
        </div>
      </div>

      <div className="flex flex-wrap gap-1.5 mt-1">
        {profile.apiTypes.map((apiType) => {
          const warning = profile.apiTypeWarnings[apiType];
          return (
            <button
              key={apiType}
              type="button"
              onClick={() => handleLaunch(apiType)}
              disabled={busy}
              className="flex items-center gap-1 px-2 py-1 rounded text-[11px] font-mono bg-primary/10 text-primary hover:bg-primary/20 disabled:opacity-50 transition-colors"
              // `title` is the only tooltip surface available without
              // pulling in a popover lib; warning text wraps natively in
              // the system OS tooltip.
              title={
                warning
                  ? `⚠ ${warning}\n\n(Click to launch ${apiTypeShort(apiType)} anyway.)`
                  : `Launch ${apiTypeShort(apiType)} with this profile`
              }
            >
              <Play className="w-3 h-3" />
              {apiTypeShort(apiType)}
              {warning && <AlertTriangle className="w-3 h-3 text-amber-500" />}
            </button>
          );
        })}
      </div>
    </div>
  );
}
