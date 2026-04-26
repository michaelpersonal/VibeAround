/**
 * Launch tab — list third-party API profiles and launch a CLI for any of
 * them in a system Terminal.app window.
 *
 * Profile cards show concrete CLI launch targets derived from the
 * provider's API kinds.
 */
import { useCallback, useEffect, useState } from "react";
import { Plus, Rocket } from "lucide-react";

import {
  deleteProfile,
  getProfile,
  launchDirect,
  launchProfile,
  listCatalog,
  listProfiles,
  upsertProfile,
} from "./api";
import { DirectCards } from "./DirectCards";
import { ProfileCard } from "./ProfileCard";
import { ProfileFormDialog } from "./ProfileFormDialog";
import { TerminalPicker } from "./TerminalPicker";
import type { CatalogEntry, ProfileDef, ProfileSummary } from "./types";

export function Launch() {
  const [catalog, setCatalog] = useState<CatalogEntry[]>([]);
  const [profiles, setProfiles] = useState<ProfileSummary[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [toast, setToast] = useState<string | null>(null);
  const [editorOpen, setEditorOpen] = useState(false);
  const [editing, setEditing] = useState<ProfileDef | null>(null);
  const [directBusy, setDirectBusy] = useState(false);

  const refresh = useCallback(async () => {
    setError(null);
    try {
      const [c, p] = await Promise.all([listCatalog(), listProfiles()]);
      setCatalog(c);
      setProfiles(p);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  // Auto-clear the toast banner after 2.5s — keeps the page from
  // accumulating stale "Terminal opened" notices on rapid launches.
  useEffect(() => {
    if (!toast) return;
    const t = setTimeout(() => setToast(null), 2500);
    return () => clearTimeout(t);
  }, [toast]);

  async function handleLaunch(profile: ProfileSummary, launchTarget: string) {
    setError(null);
    try {
      await launchProfile(profile.id, launchTarget);
      setToast(`Terminal opened for ${profile.label}`);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }

  async function handleLaunchDirect(agentId: string) {
    setError(null);
    setDirectBusy(true);
    try {
      await launchDirect(agentId);
      setToast(`${agentId} launched (no env injected)`);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setDirectBusy(false);
    }
  }

  function openNewEditor() {
    setEditing(null);
    setEditorOpen(true);
  }

  async function handleEdit(summary: ProfileSummary) {
    try {
      const full = await getProfile(summary.id);
      setEditing(full);
      setEditorOpen(true);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }

  async function handleDelete(profile: ProfileSummary) {
    try {
      await deleteProfile(profile.id);
      await refresh();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }

  async function handleSave(profile: ProfileDef) {
    await upsertProfile(profile);
    await refresh();
  }

  return (
    <div className="h-full flex flex-col">
      <header className="flex items-center justify-between px-4 py-3 border-b border-border shrink-0">
        <div className="flex items-center gap-2">
          <Rocket className="w-4 h-4 text-primary" />
          <span className="text-sm font-semibold">Launch</span>
          <span className="text-[11px] text-muted-foreground/70">
            One-click coding agent in your Terminal
          </span>
        </div>
        <div className="flex items-center gap-2">
          <TerminalPicker />
          <button
            type="button"
            onClick={openNewEditor}
            className="flex items-center gap-1 px-2.5 py-1 rounded bg-primary text-primary-foreground text-xs hover:bg-primary/90"
          >
            <Plus className="w-3 h-3" /> New profile
          </button>
        </div>
      </header>

      {error && (
        <div className="px-4 py-1.5 bg-destructive/10 text-destructive text-xs">
          {error}
        </div>
      )}
      {toast && (
        <div className="px-4 py-1.5 bg-emerald-500/10 text-emerald-600 text-xs">
          {toast}
        </div>
      )}

      <div className="flex-1 overflow-y-auto p-4 space-y-2">
        {loading ? (
          <p className="text-xs text-muted-foreground">Loading…</p>
        ) : (
          <>
            <DirectCards onLaunch={handleLaunchDirect} busy={directBusy} />

            {profiles.length === 0 ? (
              <EmptyState onNew={openNewEditor} />
            ) : (
              profiles.map((p) => (
                <ProfileCard
                  key={p.id}
                  profile={p}
                  onLaunch={(t) => handleLaunch(p, t)}
                  onEdit={() => handleEdit(p)}
                  onDelete={() => handleDelete(p)}
                />
              ))
            )}
          </>
        )}
      </div>

      {editorOpen && (
        <ProfileFormDialog
          catalog={catalog}
          initial={editing}
          onClose={() => {
            setEditorOpen(false);
            setEditing(null);
          }}
          onSave={handleSave}
        />
      )}
    </div>
  );
}

function EmptyState({ onNew }: { onNew: () => void }) {
  return (
    <div className="flex flex-col items-center justify-center text-center py-12 px-4 gap-3">
      <div className="w-10 h-10 rounded-full bg-primary/10 flex items-center justify-center">
        <Rocket className="w-5 h-5 text-primary" />
      </div>
      <div>
        <h2 className="text-sm font-semibold">No profiles yet</h2>
        <p className="text-xs text-muted-foreground mt-1 max-w-xs">
          Add your provider's API key once. From then on it's one click to
          launch claude or codex with that key already wired up — VibeAround
          opens a fresh Terminal window and stays out of the way.
        </p>
      </div>
      <button
        type="button"
        onClick={onNew}
        className="flex items-center gap-1 px-3 py-1.5 rounded bg-primary text-primary-foreground text-xs hover:bg-primary/90"
      >
        <Plus className="w-3 h-3" /> Add your first profile
      </button>
    </div>
  );
}
