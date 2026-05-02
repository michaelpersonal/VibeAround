import { useEffect } from "react";
import { Languages } from "lucide-react";
import { LOCALE_LABELS, LOCALES, useI18n, type Locale } from "@va/i18n";
import { invoke } from "@tauri-apps/api/core";

import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";

export function LanguageMenu() {
  const { locale, setLocale, t } = useI18n();

  useEffect(() => {
    void invoke("set_ui_locale", { locale }).catch((error) => {
      console.warn("[desktop-ui] set_ui_locale failed:", error);
    });
  }, [locale]);

  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button
          type="button"
          variant="ghost"
          size="icon-xs"
          title={t("Language")}
          aria-label={t("Language")}
        >
          <Languages className="w-3.5 h-3.5 text-muted-foreground" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-40">
        <DropdownMenuLabel className="text-[11px] font-medium">
          {t("Language")}
        </DropdownMenuLabel>
        <DropdownMenuRadioGroup
          value={locale}
          onValueChange={(value) => setLocale(value as Locale)}
        >
          {LOCALES.map((item) => (
            <DropdownMenuRadioItem key={item} value={item} className="text-xs">
              {LOCALE_LABELS[item]}
            </DropdownMenuRadioItem>
          ))}
        </DropdownMenuRadioGroup>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
