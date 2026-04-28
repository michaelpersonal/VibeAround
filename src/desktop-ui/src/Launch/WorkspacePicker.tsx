import { useEffect, useMemo, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { ChevronDown, FolderOpen, Plus } from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
  getLauncherPreferences,
  setLauncherWorkspace,
  type LauncherPreferences,
} from "./api";

export function WorkspacePicker() {
  const [prefs, setPrefs] = useState<LauncherPreferences | null>(null);
  const [openMenu, setOpenMenu] = useState(false);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function refresh() {
    const next = await getLauncherPreferences();
    setPrefs(next);
  }

  useEffect(() => {
    void refresh().catch((e) => setError(e instanceof Error ? e.message : String(e)));
  }, []);

  const current = useMemo(() => {
    if (!prefs) return null;
    return (
      prefs.workspaceOptions.find((option) => option.path === prefs.workspace) ?? {
        path: prefs.workspace,
        label: shortPathLabel(prefs.workspace),
        detail: prefs.workspace,
        kind: "selected",
        isDefault: false,
      }
    );
  }, [prefs]);

  async function pick(path: string) {
    if (!prefs) return;
    if (path === prefs.workspace) {
      setOpenMenu(false);
      return;
    }
    setPending(true);
    setError(null);
    try {
      await setLauncherWorkspace(path);
      await refresh();
      setOpenMenu(false);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setPending(false);
    }
  }

  async function browse() {
    setPending(true);
    setError(null);
    try {
      const selected = await open({
        directory: true,
        multiple: false,
        title: "Choose Launch Workspace",
      });
      const path = Array.isArray(selected) ? selected[0] : selected;
      if (!path) return;
      await setLauncherWorkspace(path);
      await refresh();
      setOpenMenu(false);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setPending(false);
    }
  }

  if (!prefs) return null;

  return (
    <DropdownMenu open={openMenu} onOpenChange={setOpenMenu}>
      <DropdownMenuTrigger asChild>
        <Button
          type="button"
          variant="outline"
          size="sm"
          disabled={pending}
          title={current?.detail ?? "Choose launch workspace"}
          className="h-8 px-2.5 text-xs max-w-[220px]"
        >
          <FolderOpen className="w-3 h-3" />
          <span className="truncate">{current?.label ?? "Workspace"}</span>
          <ChevronDown className="w-3 h-3 opacity-60" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-72">
        <DropdownMenuLabel className="text-[10px] font-medium uppercase text-muted-foreground/60">
          Open launches in
        </DropdownMenuLabel>
        <DropdownMenuRadioGroup
          value={prefs.workspace}
          onValueChange={(path) => {
            void pick(path);
          }}
        >
          {prefs.workspaceOptions.map((option) => (
            <DropdownMenuRadioItem
              key={option.path}
              value={option.path}
              disabled={pending}
              className="items-start text-xs"
            >
              <span className="flex min-w-0 flex-col">
                <span className="truncate">{option.label}</span>
                <span className="truncate text-[10px] text-muted-foreground/60">
                  {option.detail}
                </span>
              </span>
            </DropdownMenuRadioItem>
          ))}
        </DropdownMenuRadioGroup>
        <DropdownMenuSeparator />
        <DropdownMenuItem
          className="text-xs"
          disabled={pending}
          onSelect={(event) => {
            event.preventDefault();
            void browse();
          }}
        >
          <Plus className="w-3 h-3" />
          Choose folder…
        </DropdownMenuItem>
        {error && (
          <>
            <DropdownMenuSeparator />
            <div className="px-2 py-1 text-[10px] text-destructive">{error}</div>
          </>
        )}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

function shortPathLabel(path: string): string {
  const parts = path.split(/[\\/]/).filter(Boolean);
  return parts.at(-1) ?? path;
}
