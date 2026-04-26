/**
 * Header dropdown that lets the user pick which terminal app every Launch
 * card opens into. Persists via the `launcher_set_terminal` Tauri command.
 *
 * Unavailable terminals (not installed on this machine) stay in the menu
 * as disabled rows — keeps the selection stable as users install more
 * apps later, and surfaces what VibeAround supports without hiding it.
 */
import { useEffect, useState } from "react";
import { Check, ChevronDown, Terminal as TerminalIcon } from "lucide-react";

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
    <div className="relative">
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        disabled={pending}
        className="flex items-center gap-1 px-2 py-1 text-xs rounded hover:bg-accent text-muted-foreground border border-border disabled:opacity-50"
        title="Choose which terminal app to open on Launch"
      >
        <TerminalIcon className="w-3 h-3" />
        <span className="font-medium">{current?.label ?? "Terminal"}</span>
        <ChevronDown className="w-3 h-3 opacity-60" />
      </button>

      {open && (
        <>
          <div
            className="fixed inset-0 z-10"
            onClick={() => setOpen(false)}
            aria-hidden="true"
          />
          <div className="absolute right-0 top-full mt-1 z-20 w-52 bg-popover border border-border rounded shadow-md text-xs py-1">
            <div className="px-2.5 py-1 text-[10px] uppercase tracking-wider text-muted-foreground/60">
              Open launches in
            </div>
            {prefs.options.map((o) => {
              const isSelected = o.id === prefs.terminal;
              const disabled = !o.installed;
              return (
                <button
                  key={o.id}
                  type="button"
                  onClick={() => !disabled && pick(o.id)}
                  disabled={disabled}
                  className={`flex items-center gap-2 w-full px-2.5 py-1.5 text-left ${
                    disabled
                      ? "opacity-40 cursor-not-allowed"
                      : "hover:bg-accent cursor-pointer"
                  }`}
                >
                  <span className="w-3 h-3 shrink-0">
                    {isSelected && <Check className="w-3 h-3" />}
                  </span>
                  <span className="flex-1">{o.label}</span>
                  {disabled && (
                    <span className="text-[10px] text-muted-foreground/60">
                      not installed
                    </span>
                  )}
                </button>
              );
            })}
            {error && (
              <div className="px-2.5 py-1 text-[10px] text-destructive border-t border-border/50">
                {error}
              </div>
            )}
          </div>
        </>
      )}
    </div>
  );
}
