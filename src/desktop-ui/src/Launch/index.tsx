/**
 * Launch tab — list third-party API profiles and launch a CLI for any of
 * them in a system Terminal.app window.
 *
 * Profile cards show concrete CLI launch targets derived from the
 * provider's API kinds.
 */
import { useCallback, useEffect, useState } from "react";
import { DragDropProvider } from "@dnd-kit/react";
import type { DragEndEvent } from "@dnd-kit/react";
import { isSortable, useSortable } from "@dnd-kit/react/sortable";
import { Plus, Rocket } from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  deleteProfile,
  getProfile,
  getLauncherPreferences,
  launchDefault,
  launchDirect,
  launchProfile,
  listCatalog,
  listProfiles,
  reorderProfiles,
  setLauncherDefault,
  upsertProfile,
  type LauncherPreferences,
} from "./api";
import { DirectCards } from "./DirectCards";
import { LaunchSettingsMenu } from "./LaunchSettingsMenu";
import { ProfileCard } from "./ProfileCard";
import { ProfileFormDialog } from "./ProfileFormDialog";
import { WorkspacePicker } from "./WorkspacePicker";
import type { CatalogEntry, ProfileDef, ProfileSummary } from "./types";

export function Launch() {
  const [catalog, setCatalog] = useState<CatalogEntry[]>([]);
  const [profiles, setProfiles] = useState<ProfileSummary[]>([]);
  const [prefs, setPrefs] = useState<LauncherPreferences | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [toast, setToast] = useState<string | null>(null);
  const [editorOpen, setEditorOpen] = useState(false);
  const [editing, setEditing] = useState<ProfileDef | null>(null);
  const [directBusy, setDirectBusy] = useState(false);
  const [reorderBusy, setReorderBusy] = useState(false);

  const refresh = useCallback(async () => {
    setError(null);
    try {
      const [c, p, nextPrefs] = await Promise.all([
        listCatalog(),
        listProfiles(),
        getLauncherPreferences(),
      ]);
      setCatalog(c);
      setProfiles(p);
      setPrefs(nextPrefs);
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

  async function handleLaunchDefault() {
    setError(null);
    setDirectBusy(true);
    try {
      await launchDefault();
      setToast("Quick launch opened");
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setDirectBusy(false);
    }
  }

  async function handleSetDefault(agentId: string, profileId: string | null) {
    setError(null);
    try {
      await setLauncherDefault(agentId, profileId);
      const nextPrefs = await getLauncherPreferences();
      setPrefs(nextPrefs);
      setToast(profileId ? "Quick Launch default updated" : "Direct Quick Launch default updated");
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
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

  function handleProfileDragEnd(event: DragEndEvent) {
    if (event.canceled || reorderBusy) return;

    const { source } = event.operation;
    if (!isSortable(source) || source.initialIndex === source.index) return;

    const previousProfiles = profiles;
    const nextProfiles = moveProfileByIndex(profiles, source.initialIndex, source.index);
    if (nextProfiles === profiles) return;

    void persistProfileOrder(nextProfiles, previousProfiles);
  }

  async function persistProfileOrder(
    nextProfiles: ProfileSummary[],
    previousProfiles: ProfileSummary[],
  ) {
    setProfiles(nextProfiles);
    setError(null);
    setReorderBusy(true);
    try {
      await reorderProfiles(nextProfiles.map((profile) => profile.id));
      setToast("Profile order updated");
    } catch (e) {
      setProfiles(previousProfiles);
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setReorderBusy(false);
    }
  }

  return (
    <div className="h-full flex flex-col">
      <header className="flex items-center justify-between px-3 py-2 border-b border-border shrink-0">
        <div className="flex items-center gap-2">
          <Rocket className="w-4 h-4 text-primary" />
          <span className="text-[13px] font-semibold">Launch</span>
          <span className="text-[11px] text-muted-foreground/70">
            One-click coding agent in your Terminal
          </span>
        </div>
        <div className="flex items-center gap-2">
          <Button
            type="button"
            onClick={openNewEditor}
            size="sm"
            variant="outline"
            className="h-8 text-xs"
          >
            <Plus className="w-3 h-3" /> New profile
          </Button>
          <WorkspacePicker prefs={prefs} onChange={setPrefs} />
          <LaunchSettingsMenu prefs={prefs} onChange={setPrefs} />
          <Button
            type="button"
            onClick={handleLaunchDefault}
            size="sm"
            disabled={directBusy}
            className="h-8 text-xs font-semibold"
            title={quickLaunchTitle(prefs, profiles)}
          >
            <Rocket className="w-3.5 h-3.5" /> Quick launch
          </Button>
        </div>
      </header>

      {error && (
        <div className="px-3 py-1 bg-destructive/10 text-destructive text-xs">
          {error}
        </div>
      )}
      {toast && (
        <div className="px-3 py-1 bg-emerald-500/10 text-emerald-600 text-xs">
          {toast}
        </div>
      )}

      <div className="flex-1 overflow-y-auto p-3 space-y-2">
        {loading ? (
          <p className="text-xs text-muted-foreground">Loading…</p>
        ) : (
          <>
            <DirectCards
              onLaunch={handleLaunchDirect}
              onSetDefault={(agentId) => handleSetDefault(agentId, null)}
              busy={directBusy}
              defaultAgent={prefs?.defaultAgent}
              defaultProfiles={prefs?.defaultProfiles}
            />

            {profiles.length === 0 ? (
              <EmptyState onNew={openNewEditor} />
            ) : (
              <DragDropProvider onDragEnd={handleProfileDragEnd}>
                {profiles.map((p, index) => (
                  <SortableProfileCard
                    key={p.id}
                    profile={p}
                    index={index}
                    reorderBusy={reorderBusy}
                    onLaunch={(t) => handleLaunch(p, t)}
                    onSetDefault={(t) => handleSetDefault(t, p.id)}
                    onEdit={() => handleEdit(p)}
                    onDelete={() => handleDelete(p)}
                    defaultAgent={prefs?.defaultAgent}
                    defaultProfiles={prefs?.defaultProfiles}
                  />
                ))}
              </DragDropProvider>
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

function moveProfileByIndex(
  profiles: ProfileSummary[],
  fromIndex: number,
  toIndex: number,
): ProfileSummary[] {
  if (
    fromIndex === toIndex ||
    fromIndex < 0 ||
    toIndex < 0 ||
    fromIndex >= profiles.length ||
    toIndex >= profiles.length
  ) {
    return profiles;
  }

  const next = [...profiles];
  const [dragged] = next.splice(fromIndex, 1);
  next.splice(toIndex, 0, dragged);
  return next;
}

function SortableProfileCard({
  profile,
  index,
  reorderBusy,
  onLaunch,
  onSetDefault,
  onEdit,
  onDelete,
  defaultAgent,
  defaultProfiles,
}: {
  profile: ProfileSummary;
  index: number;
  reorderBusy: boolean;
  onLaunch: (launchTarget: string) => Promise<void>;
  onSetDefault: (launchTarget: string) => Promise<void>;
  onEdit: () => void;
  onDelete: () => Promise<void>;
  defaultAgent?: string;
  defaultProfiles?: Record<string, string>;
}) {
  const { ref, handleRef, isDragging, isDropTarget } = useSortable({
    id: profile.id,
    index,
    disabled: reorderBusy,
  });

  return (
    <div
      ref={ref}
      className={`relative rounded-md transition-shadow ${
        isDropTarget ? "ring-2 ring-primary/35 shadow-lg shadow-primary/20" : ""
      }`}
    >
      <ProfileCard
        profile={profile}
        onLaunch={onLaunch}
        onSetDefault={onSetDefault}
        onEdit={onEdit}
        onDelete={onDelete}
        defaultAgent={defaultAgent}
        defaultProfiles={defaultProfiles}
        dragHandleRef={handleRef}
        dragHandleDisabled={reorderBusy}
        isDragging={isDragging}
      />
    </div>
  );
}

function quickLaunchTitle(
  prefs: LauncherPreferences | null,
  profiles: ProfileSummary[],
): string {
  if (!prefs) return "Launch the default CLI";
  const profileId = prefs.defaultProfiles[prefs.defaultAgent];
  if (!profileId) return `Launch ${prefs.defaultAgent} directly`;
  const profile = profiles.find((p) => p.id === profileId);
  return `Launch ${prefs.defaultAgent} with ${profile?.label ?? profileId}`;
}

function EmptyState({ onNew }: { onNew: () => void }) {
  return (
    <div className="flex flex-col items-center justify-center text-center py-8 px-3 gap-2.5">
      <div className="w-9 h-9 rounded-full bg-primary/10 flex items-center justify-center">
        <Rocket className="w-4 h-4 text-primary" />
      </div>
      <div>
        <h2 className="text-[13px] font-semibold">No profiles yet</h2>
        <p className="text-xs text-muted-foreground mt-1 max-w-xs">
          Add your provider's API key once. From then on it's one click to
          launch claude or codex with that key already wired up — VibeAround
          opens a fresh Terminal window and stays out of the way.
        </p>
      </div>
      <Button
        type="button"
        onClick={onNew}
        size="sm"
      >
        <Plus className="w-3 h-3" /> Add your first profile
      </Button>
    </div>
  );
}
