/**
 * Header dropdown that lets the user pick which terminal app every Launch
 * card opens into. Persists via the `launcher_set_terminal` Tauri command.
 *
 * Unavailable terminals (not installed on this machine) stay in the menu
 * as disabled rows — keeps the selection stable as users install more
 * apps later, and surfaces what VibeAround supports without hiding it.
 */
import { useEffect, useState } from "react";
import { ChevronDown, Terminal as TerminalIcon } from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuShortcut,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
  getLauncherPreferences,
  setLauncherTerminal,
  type LauncherPreferences,
  type TerminalOption,
} from "./api";

export function TerminalPicker() {
  const [prefs, setPrefs] = useState<LauncherPreferences | null>(null);
  const [open, setOpen] = useState(false);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    void getLauncherPreferences()
      .then(setPrefs)
      .catch((e) => setError(e instanceof Error ? e.message : String(e)));
  }, []);

  if (!prefs) return null;

  const current: TerminalOption | undefined =
    prefs.options.find((o) => o.id === prefs.terminal) ?? prefs.options[0];

  async function pick(id: string) {
    if (!prefs) return;
    if (id === prefs.terminal) {
      setOpen(false);
      return;
    }
    setPending(true);
    setError(null);
    try {
      await setLauncherTerminal(id);
      const next = await getLauncherPreferences();
      setPrefs(next);
      setOpen(false);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setPending(false);
    }
  }

  return (
    <DropdownMenu open={open} onOpenChange={setOpen}>
      <DropdownMenuTrigger asChild>
        <Button
          type="button"
          variant="outline"
          size="sm"
          disabled={pending}
          title="Choose which terminal app to open on Launch"
          className="h-8 px-2.5 text-xs"
        >
          <TerminalIcon className="w-3 h-3" />
          <span>{current?.label ?? "Terminal"}</span>
          <ChevronDown className="w-3 h-3 opacity-60" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-56">
        <DropdownMenuLabel className="text-[10px] font-medium uppercase text-muted-foreground/60">
          Open launches in
        </DropdownMenuLabel>
        <DropdownMenuRadioGroup
          value={prefs.terminal}
          onValueChange={(id) => {
            void pick(id);
          }}
        >
          {prefs.options.map((o) => (
            <DropdownMenuRadioItem
              key={o.id}
              value={o.id}
              disabled={!o.installed || pending}
              className="text-xs"
            >
              <span>{o.label}</span>
              {!o.installed && (
                <DropdownMenuShortcut className="normal-case tracking-normal">
                  not installed
                </DropdownMenuShortcut>
              )}
            </DropdownMenuRadioItem>
          ))}
        </DropdownMenuRadioGroup>
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
