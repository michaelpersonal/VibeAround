import { useState } from "react";
import {
  ChevronDown,
  Network,
  SlidersHorizontal,
  Terminal as TerminalIcon,
} from "lucide-react";
import { useI18n } from "@va/i18n";

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
  setLauncherCompatibilityProxy,
  setLauncherTerminal,
  type LauncherPreferences,
  type TerminalOption,
} from "./api";
import type { CompatibilityProxyMode } from "./types";

interface Props {
  prefs: LauncherPreferences | null;
  onChange: (prefs: LauncherPreferences) => void;
}

const PROXY_LABELS: Record<CompatibilityProxyMode, string> = {
  auto: "Auto",
  on: "Force on",
  off: "Off",
};

export function LaunchSettingsMenu({ prefs, onChange }: Props) {
  const { t } = useI18n();
  const [openMenu, setOpenMenu] = useState(false);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const currentTerminal: TerminalOption | undefined =
    prefs?.options.find((option) => option.id === prefs.terminal) ??
    prefs?.options[0];

  const title = prefs
    ? [
        t("Terminal: {{terminal}}", { terminal: currentTerminal?.label ?? t("Terminal") }),
        t("API proxy: {{mode}}", { mode: t(PROXY_LABELS[prefs.compatibilityProxy]) }),
      ].join("\n")
    : t("Launch settings");

  async function refresh() {
    onChange(await getLauncherPreferences());
  }

  async function pickTerminal(id: string) {
    if (!prefs) return;
    if (id === prefs.terminal) {
      setOpenMenu(false);
      return;
    }
    setPending(true);
    setError(null);
    try {
      await setLauncherTerminal(id);
      await refresh();
      setOpenMenu(false);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setPending(false);
    }
  }

  async function pickProxy(mode: CompatibilityProxyMode) {
    if (!prefs) return;
    if (mode === prefs.compatibilityProxy) {
      setOpenMenu(false);
      return;
    }
    setPending(true);
    setError(null);
    try {
      await setLauncherCompatibilityProxy(mode);
      await refresh();
      setOpenMenu(false);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setPending(false);
    }
  }

  return (
    <DropdownMenu open={openMenu} onOpenChange={setOpenMenu}>
      <DropdownMenuTrigger asChild>
        <Button
          type="button"
          variant="outline"
          size="sm"
          disabled={!prefs || pending}
          title={title}
          className="h-8 px-2.5 text-xs"
        >
          <SlidersHorizontal className="w-3 h-3" />
          {t("Launch settings")}
          <ChevronDown className="w-3 h-3 opacity-60" />
        </Button>
      </DropdownMenuTrigger>
      {prefs && (
        <DropdownMenuContent align="end" className="w-80">
          <DropdownMenuLabel className="flex items-center gap-2 text-[11px] font-medium">
            <TerminalIcon className="w-3 h-3" />
            {t("Terminal app")}
          </DropdownMenuLabel>
          <DropdownMenuRadioGroup
            value={prefs.terminal}
            onValueChange={(id) => {
              void pickTerminal(id);
            }}
          >
            {prefs.options.map((option) => (
              <DropdownMenuRadioItem
                key={option.id}
                value={option.id}
                disabled={!option.installed || pending}
                className="text-xs"
              >
                <span>{option.label}</span>
                {!option.installed && (
                  <DropdownMenuShortcut className="normal-case tracking-normal">
                    {t("not installed")}
                  </DropdownMenuShortcut>
                )}
              </DropdownMenuRadioItem>
            ))}
          </DropdownMenuRadioGroup>

          <DropdownMenuSeparator />
          <DropdownMenuLabel className="flex items-center gap-2 text-[11px] font-medium">
            <Network className="w-3 h-3" />
            {t("API proxy")}
          </DropdownMenuLabel>
          <DropdownMenuRadioGroup
            value={prefs.compatibilityProxy}
            onValueChange={(mode) => {
              void pickProxy(mode as CompatibilityProxyMode);
            }}
          >
            <DropdownMenuRadioItem value="auto" disabled={pending} className="text-xs">
              {t("Auto")}
            </DropdownMenuRadioItem>
            <DropdownMenuRadioItem value="on" disabled={pending} className="text-xs">
              {t("Force on")}
            </DropdownMenuRadioItem>
            <DropdownMenuRadioItem value="off" disabled={pending} className="text-xs">
              {t("Off")}
            </DropdownMenuRadioItem>
          </DropdownMenuRadioGroup>

          {error && (
            <>
              <DropdownMenuSeparator />
              <div className="px-2 py-1 text-[10px] text-destructive">{error}</div>
            </>
          )}
        </DropdownMenuContent>
      )}
    </DropdownMenu>
  );
}
