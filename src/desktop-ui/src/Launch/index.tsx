/**
 * Launch tab — list third-party API profiles and launch a CLI for any of
 * them in a system Terminal.app window.
 *
 * Profile cards show concrete CLI launch targets derived from the
 * provider's API kinds.
 */
import { useCallback, useEffect, useRef, useState } from "react";
import { DndProvider, useDrag, useDrop } from "react-dnd";
import { HTML5Backend } from "react-dnd-html5-backend";
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
import { ProfileCard } from "./ProfileCard";
import { ProfileFormDialog } from "./ProfileFormDialog";
import { TerminalPicker } from "./TerminalPicker";
import { WorkspacePicker } from "./WorkspacePicker";
import type { CatalogEntry, ProfileDef, ProfileSummary } from "./types";

const PROFILE_DND_TYPE = "launch-profile";

type DragProfileItem = {
  id: string;
  index: number;
};

type DragOverProfile = {
  id: string;
  placeAfter: boolean;
};

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
  const [dragOverProfile, setDragOverProfile] = useState<DragOverProfile | null>(null);
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

  function handleProfileHover(targetProfileId: string, placeAfter: boolean) {
    setDragOverProfile((current) => {
      if (current?.id === targetProfileId && current.placeAfter === placeAfter) {
        return current;
      }
      return { id: targetProfileId, placeAfter };
    });
  }

  function clearProfileDragState() {
    setDragOverProfile(null);
  }

  async function handleProfileDrop(
    draggedId: string,
    targetProfileId: string,
    placeAfter: boolean,
  ) {
    setDragOverProfile(null);
    if (!draggedId || draggedId === targetProfileId || reorderBusy) return;

    const nextProfiles = moveProfile(profiles, draggedId, targetProfileId, placeAfter);
    if (nextProfiles === profiles) return;

    const previousProfiles = profiles;
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
          <TerminalPicker />
          <WorkspacePicker />
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
              <DndProvider backend={HTML5Backend}>
                {profiles.map((p, index) => (
                  <SortableProfileCard
                    key={p.id}
                    profile={p}
                    index={index}
                    reorderBusy={reorderBusy}
                    dragOverProfile={dragOverProfile}
                    onDragHover={handleProfileHover}
                    onDragCancel={clearProfileDragState}
                    onProfileDrop={handleProfileDrop}
                    onLaunch={(t) => handleLaunch(p, t)}
                    onSetDefault={(t) => handleSetDefault(t, p.id)}
                    onEdit={() => handleEdit(p)}
                    onDelete={() => handleDelete(p)}
                    defaultAgent={prefs?.defaultAgent}
                    defaultProfiles={prefs?.defaultProfiles}
                  />
                ))}
              </DndProvider>
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

function moveProfile(
  profiles: ProfileSummary[],
  draggedId: string,
  targetId: string,
  placeAfter: boolean,
): ProfileSummary[] {
  const fromIndex = profiles.findIndex((profile) => profile.id === draggedId);
  if (fromIndex === -1) return profiles;

  const next = [...profiles];
  const [dragged] = next.splice(fromIndex, 1);
  const targetIndex = next.findIndex((profile) => profile.id === targetId);
  if (targetIndex === -1) return profiles;

  next.splice(placeAfter ? targetIndex + 1 : targetIndex, 0, dragged);
  return next;
}

function SortableProfileCard({
  profile,
  index,
  reorderBusy,
  dragOverProfile,
  onDragHover,
  onDragCancel,
  onProfileDrop,
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
  dragOverProfile: DragOverProfile | null;
  onDragHover: (targetProfileId: string, placeAfter: boolean) => void;
  onDragCancel: () => void;
  onProfileDrop: (draggedId: string, targetProfileId: string, placeAfter: boolean) => Promise<void>;
  onLaunch: (launchTarget: string) => Promise<void>;
  onSetDefault: (launchTarget: string) => Promise<void>;
  onEdit: () => void;
  onDelete: () => Promise<void>;
  defaultAgent?: string;
  defaultProfiles?: Record<string, string>;
}) {
  const cardRef = useRef<HTMLDivElement>(null);
  const handleRef = useRef<HTMLDivElement>(null);

  const [{ isDragging }, drag, preview] = useDrag(
    () => ({
      type: PROFILE_DND_TYPE,
      item: { id: profile.id, index },
      canDrag: () => !reorderBusy,
      collect: (monitor) => ({
        isDragging: monitor.isDragging(),
      }),
      end: () => {
        onDragCancel();
      },
    }),
    [index, onDragCancel, profile.id, reorderBusy],
  );

  const [{ canDrop, isOver }, drop] = useDrop(
    () => ({
      accept: PROFILE_DND_TYPE,
      canDrop: (item: DragProfileItem) => item.id !== profile.id && !reorderBusy,
      hover: (item: DragProfileItem, monitor) => {
        if (item.id === profile.id || reorderBusy) return;
        const placeAfter = getDropPlacement(cardRef.current, monitor.getClientOffset());
        if (placeAfter === null) return;
        onDragHover(profile.id, placeAfter);
      },
      drop: (item: DragProfileItem, monitor) => {
        if (item.id === profile.id || reorderBusy) return;
        const placeAfter = getDropPlacement(cardRef.current, monitor.getClientOffset());
        void onProfileDrop(item.id, profile.id, placeAfter ?? false);
      },
      collect: (monitor) => ({
        canDrop: monitor.canDrop(),
        isOver: monitor.isOver({ shallow: true }),
      }),
    }),
    [onDragHover, onProfileDrop, profile.id, reorderBusy],
  );

  drag(handleRef);
  drop(cardRef);
  preview(cardRef);

  const showDropCue = canDrop && isOver && dragOverProfile?.id === profile.id;

  return (
    <div
      ref={cardRef}
      className={`relative rounded-md transition-shadow ${
        showDropCue ? "ring-2 ring-primary/35" : ""
      }`}
    >
      {showDropCue && (
        <div
          className={`absolute left-2 right-2 h-0.5 rounded-full bg-primary ${
            dragOverProfile.placeAfter ? "-bottom-1" : "-top-1"
          }`}
        />
      )}
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

function getDropPlacement(
  element: HTMLDivElement | null,
  clientOffset: { x: number; y: number } | null,
): boolean | null {
  if (!element || !clientOffset) return null;
  const rect = element.getBoundingClientRect();
  return clientOffset.y > rect.top + rect.height / 2;
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
