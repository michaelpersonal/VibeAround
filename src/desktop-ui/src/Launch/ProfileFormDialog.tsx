/**
 * Two-step modal: pick a provider, then fill its credentials.
 *
 * Step 1 lets the user click any catalog tile. Step 2 builds a form by
 * intersecting the catalog API kinds' `fields[]`. We default to the
 * api_key auth mode and let custom providers multi-select API kinds when
 * one key supports more than one.
 */
import { useEffect, useMemo, useState } from "react";
import { useI18n } from "@va/i18n";

import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  CUSTOM_PROVIDER,
  generateProfileId,
} from "./ProfileFormDialog.constants";
import { FormBody } from "./ProfileFormBody";
import { ProviderGrid } from "./ProfileProviderGrid";
import {
  arraysEqual,
  collectFields,
  pruneOverrides,
  pruneProviderSettings,
  stripEmpty,
} from "./profileFormHelpers";
import type {
  ApiTypeOverrides,
  AuthMode,
  CatalogEntry,
  ProfileDef,
  ProviderSettings,
} from "./types";
import { isProviderApiKind } from "./types";

type Step = "pick-provider" | "fill-form";

interface Props {
  catalog: CatalogEntry[];
  /** Set when editing -- locks step 1 and prefills step 2. */
  initial?: ProfileDef | null;
  onClose: () => void;
  onSave: (profile: ProfileDef) => Promise<void>;
}

export function ProfileFormDialog({
  catalog,
  initial,
  onClose,
  onSave,
}: Props) {
  const { t } = useI18n();
  const editing = !!initial;

  const { initialProvider, providerMissing } = useMemo(() => {
    if (!initial) return { initialProvider: null, providerMissing: false };
    if (initial.provider === "custom") {
      return { initialProvider: CUSTOM_PROVIDER, providerMissing: false };
    }
    const found = catalog.find((c) => c.id === initial.provider);
    if (!found) {
      return { initialProvider: CUSTOM_PROVIDER, providerMissing: true };
    }
    return { initialProvider: found, providerMissing: false };
  }, [catalog, initial]);

  const [step, setStep] = useState<Step>(
    editing ? "fill-form" : "pick-provider",
  );
  const [provider, setProvider] = useState<CatalogEntry | null>(
    initialProvider,
  );
  const [label, setLabel] = useState(initial?.label ?? "");
  const [selectedApiTypes, setSelectedApiTypes] = useState<string[]>(
    (initial?.api_types ?? []).filter(isProviderApiKind),
  );
  const [credentials, setCredentials] = useState<Record<string, string>>(
    initial?.credentials ?? {},
  );
  const [overrides, setOverrides] = useState<Record<string, ApiTypeOverrides>>(
    initial?.overrides ?? {},
  );
  const [providerSettings, setProviderSettings] = useState<ProviderSettings>(
    initial?.provider_settings ?? {},
  );
  const [revealKeys, setRevealKeys] = useState<Record<string, boolean>>({});
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    if (!provider || editing) return;
    const apiKindEndpoints = provider.endpoints.filter((e) =>
      isProviderApiKind(e.api_type),
    );
    setSelectedApiTypes(apiKindEndpoints.map((e) => e.api_type));
    const next: Record<string, ApiTypeOverrides> = {};
    for (const ep of apiKindEndpoints) {
      next[ep.api_type] = {
        model: ep.models[0]?.id ?? "",
        base_url: ep.default_base_url || undefined,
      };
    }
    setOverrides(next);
    setProviderSettings(
      provider.id === "deepseek"
        ? {
            deepseek: {
              thinking: true,
              replay_reasoning_content: true,
            },
          }
        : {},
    );
  }, [provider, editing]);

  useEffect(() => {
    if (!provider || provider.id === "custom") return;
    const apiKinds = provider.endpoints
      .filter((e) => isProviderApiKind(e.api_type))
      .map((e) => e.api_type);
    setSelectedApiTypes((current) =>
      arraysEqual(current, apiKinds) ? current : apiKinds,
    );
  }, [provider]);

  function handlePickProvider(c: CatalogEntry) {
    setProvider(c);
    if (!label) setLabel(c.label);
    setStep("fill-form");
  }

  async function handleSave() {
    setError(null);
    if (!provider) return;
    if (!label.trim()) {
      setError(t("Label is required"));
      return;
    }
    if (selectedApiTypes.length === 0) {
      setError(t("Pick at least one API type"));
      return;
    }

    const fieldDefs = collectFields(provider, selectedApiTypes, "api_key");
    for (const f of fieldDefs) {
      if (f.required && !credentials[f.name]?.trim()) {
        setError(t("{{field}} is required", { field: t(f.label) }));
        return;
      }
    }

    for (const apiType of selectedApiTypes) {
      const ep = provider.endpoints.find((e) => e.api_type === apiType);
      if (!ep) continue;
      const ov = overrides[apiType];
      if (!ov?.model?.trim()) {
        setError(t("Model is required for {{apiType}}", { apiType }));
        return;
      }
      if (ep.default_base_url) continue;
      if (!ov?.base_url?.trim()) {
        setError(t("Base URL is required for {{apiType}}", { apiType }));
        return;
      }
    }

    const profile: ProfileDef = {
      id: initial?.id ?? generateProfileId(provider.id),
      label: label.trim(),
      provider: provider.id,
      auth_mode: "api_key" as AuthMode,
      api_types: selectedApiTypes,
      credentials: stripEmpty(credentials),
      overrides: pruneOverrides(overrides, selectedApiTypes, provider),
      provider_settings: pruneProviderSettings(provider.id, providerSettings),
    };

    setSaving(true);
    try {
      await onSave(profile);
      onClose();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      setSaving(false);
    }
  }

  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) onClose();
      }}
    >
      <DialogContent>
        <DialogHeader className="border-b border-border pr-10">
          <DialogTitle>
            {editing
              ? t("Edit profile · {{label}}", { label: initial!.label })
              : step === "pick-provider"
                ? t("Pick a provider")
                : t("New profile · {{provider}}", { provider: provider?.label ?? "" })}
          </DialogTitle>
          <DialogDescription className="sr-only">
            {t("Configure a Quick Launch provider profile.")}
          </DialogDescription>
        </DialogHeader>

        <div className="flex-1 overflow-y-auto p-4">
          {step === "pick-provider" ? (
            <ProviderGrid catalog={catalog} onPick={handlePickProvider} />
          ) : provider ? (
            <FormBody
              provider={provider}
              label={label}
              setLabel={setLabel}
              selectedApiTypes={selectedApiTypes}
              setSelectedApiTypes={setSelectedApiTypes}
              credentials={credentials}
              setCredentials={setCredentials}
              overrides={overrides}
              setOverrides={setOverrides}
              providerSettings={providerSettings}
              setProviderSettings={setProviderSettings}
              revealKeys={revealKeys}
              setRevealKeys={setRevealKeys}
            />
          ) : null}
        </div>

        {providerMissing && (
          <div className="px-4 py-2 bg-amber-500/10 text-amber-700 text-xs border-t border-amber-500/20">
            ⚠ {t("The provider {{provider}} is no longer in the catalog. Form fell back to a custom endpoint — re-pick a provider via Back, or edit the URL/key and save.", {
              provider: initial?.provider ?? "",
            })}
          </div>
        )}
        {error && (
          <div className="px-4 py-2 bg-destructive/10 text-destructive text-xs border-t border-destructive/20">
            {error}
          </div>
        )}

        <DialogFooter className="justify-between border-t border-border">
          <div>
            {step === "fill-form" && !editing && (
              <Button
                type="button"
                variant="ghost"
                size="sm"
                onClick={() => setStep("pick-provider")}
              >
                {t("Back")}
              </Button>
            )}
          </div>
          <div className="flex items-center gap-2">
            <Button type="button" variant="ghost" size="sm" onClick={onClose}>
              {t("Cancel")}
            </Button>
            {step === "fill-form" && (
              <Button
                type="button"
                size="sm"
                onClick={handleSave}
                disabled={saving}
              >
                {saving
                  ? t("Saving…")
                  : editing
                    ? t("Save changes")
                    : t("Create profile")}
              </Button>
            )}
          </div>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
