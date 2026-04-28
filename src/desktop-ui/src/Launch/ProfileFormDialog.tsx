/**
 * Two-step modal: pick a provider, then fill its credentials.
 *
 * Step 1 lets the user click any catalog tile. Step 2 builds a form by
 * intersecting the catalog API kinds' `fields[]`. We default to the
 * api_key auth mode (the only one v1 catalog ships) and let custom
 * providers multi-select API kinds when one key supports more than one.
 */
import { useEffect, useMemo, useState } from "react";

import { Eye, EyeOff, Globe, Search } from "lucide-react";

import { BrandIcon } from "@/components/brand-icon";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import type {
  ApiTypeOverrides,
  AuthMode,
  AuthModeDef,
  CatalogEntry,
  FieldDef,
  ProfileDef,
} from "./types";
import { apiTypeLabel, apiTypeShort, isProviderApiKind } from "./types";

type Step = "pick-provider" | "fill-form";
const PROVIDER_TILE_GRID =
  "grid grid-cols-[repeat(auto-fill,minmax(180px,1fr))] gap-2";

interface Props {
  catalog: CatalogEntry[];
  /** Set when editing — locks step 1 and prefills step 2. */
  initial?: ProfileDef | null;
  onClose: () => void;
  onSave: (profile: ProfileDef) => Promise<void>;
}

/**
 * Synthetic catalog entry for the custom escape hatch. The backend has a
 * matching `catalog::get("custom")` that returns the same shape with the
 * actual render templates; this client-side copy is only used to drive
 * the form (fields list, empty models, empty default base_url).
 */
export const CUSTOM_PROVIDER: CatalogEntry = {
  id: "custom",
  label: "Custom endpoint",
  icon: "✨",
  homepage: null,
  endpoints: [
    {
      api_type: "anthropic",
      default_base_url: "",
      models: [],
      auth_modes: [
        {
          mode: "api_key",
          label: "Use API key",
          fields: [
            {
              name: "api_key",
              label: "API key",
              secret: true,
              required: true,
            },
          ],
        },
      ],
    },
    {
      api_type: "openai-responses",
      default_base_url: "",
      models: [],
      capabilities: {
        reasoning_effort: true,
      },
      auth_modes: [
        {
          mode: "api_key",
          label: "Use API key",
          fields: [
            {
              name: "api_key",
              label: "API key",
              secret: true,
              required: true,
            },
          ],
        },
      ],
    },
    {
      api_type: "openai-chat",
      default_base_url: "",
      models: [],
      auth_modes: [
        {
          mode: "api_key",
          label: "Use API key",
          fields: [
            {
              name: "api_key",
              label: "API key",
              secret: true,
              required: true,
            },
          ],
        },
      ],
    },
  ],
};

/**
 * Generate a fresh profile id. Format: `${provider}-${random8}` so the
 * same provider can host multiple profiles (work key, personal key, …)
 * and the on-disk filename still reflects the provider for at-a-glance
 * inspection. The random suffix uses base36 to stay inside the
 * `[a-z0-9_-]{1,64}` alphabet the backend accepts.
 */
function generateProfileId(providerId: string): string {
  const random = Math.random().toString(36).slice(2, 10).padEnd(8, "0");
  return `${providerId}-${random}`;
}

const INPUT_CLASS = "h-8 text-[13px]";
const MONO_INPUT_CLASS = "h-8 text-[13px] font-mono";
const SECRET_INPUT_CLASS = "h-8 pr-8 text-[13px] font-mono";

export function ProfileFormDialog({
  catalog,
  initial,
  onClose,
  onSave,
}: Props) {
  const editing = !!initial;

  // When editing a profile whose provider was removed from the catalog
  // (rename, deprecation, etc.), fall through to CUSTOM_PROVIDER so the
  // form is still functional — the user keeps their api_key + can pick
  // a different provider via Back. Without this fallback the dialog
  // would render an empty form with no fields.
  const { initialProvider, providerMissing } = useMemo(() => {
    if (!initial) return { initialProvider: null, providerMissing: false };
    if (initial.provider === "custom") {
      return { initialProvider: CUSTOM_PROVIDER, providerMissing: false };
    }
    const found = catalog.find((c) => c.id === initial.provider);
    if (!found) return { initialProvider: CUSTOM_PROVIDER, providerMissing: true };
    return { initialProvider: found, providerMissing: false };
  }, [catalog, initial]);

  const [step, setStep] = useState<Step>(editing ? "fill-form" : "pick-provider");
  const [provider, setProvider] = useState<CatalogEntry | null>(initialProvider);

  // ID is system-generated for new profiles (so the same provider can
  // host multiple — e.g. "DeepSeek work" and "DeepSeek personal") and
  // preserved unchanged when editing.
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
  const [revealKeys, setRevealKeys] = useState<Record<string, boolean>>({});
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  // When user lands on step 2 fresh, default api_types to all provider API
  // kinds (most users want the credential to be usable every supported way) and
  // pre-fill model + base_url with the catalog defaults so the values are
  // visible (not just placeholder) — these are public docs anyway and the
  // "tick a model + paste api_key" flow is the goal.
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
      setError("Label is required");
      return;
    }
    if (selectedApiTypes.length === 0) {
      setError("Pick at least one API type");
      return;
    }

    // Validate required fields across the selected api types' api_key auth.
    const fieldDefs = collectFields(provider, selectedApiTypes, "api_key");
    for (const f of fieldDefs) {
      if (f.required && !credentials[f.name]?.trim()) {
        setError(`${f.label} is required`);
        return;
      }
    }

    // Custom endpoints have no `default_base_url` baked in, so the user
    // must supply one per selected api_type. Skip the check when the
    // catalog provides a default — render-time falls back to it.
    for (const apiType of selectedApiTypes) {
      const ep = provider.endpoints.find((e) => e.api_type === apiType);
      if (!ep) continue;
      const ov = overrides[apiType];
      if (!ov?.model?.trim()) {
        setError(`Model is required for ${apiType}`);
        return;
      }
      if (ep.default_base_url) continue;
      if (!ov?.base_url?.trim()) {
        setError(`Base URL is required for ${apiType}`);
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
              ? `Edit profile · ${initial!.label}`
              : step === "pick-provider"
              ? "Pick a provider"
              : `New profile · ${provider?.label}`}
          </DialogTitle>
          <DialogDescription className="sr-only">
            Configure a Quick Launch provider profile.
          </DialogDescription>
        </DialogHeader>

        <div className="flex-1 overflow-y-auto p-4">
          {step === "pick-provider" ? (
            <ProviderGrid catalog={catalog} onPick={handlePickProvider} />
          ) : provider ? (
            <FormBody
              provider={provider}
              editing={editing}
              label={label}
              setLabel={setLabel}
              selectedApiTypes={selectedApiTypes}
              setSelectedApiTypes={setSelectedApiTypes}
              credentials={credentials}
              setCredentials={setCredentials}
              overrides={overrides}
              setOverrides={setOverrides}
              revealKeys={revealKeys}
              setRevealKeys={setRevealKeys}
            />
          ) : null}
        </div>

        {providerMissing && (
          <div className="px-4 py-2 bg-amber-500/10 text-amber-700 text-xs border-t border-amber-500/20">
            ⚠ The provider <code>{initial?.provider}</code> is no longer in the
            catalog. Form fell back to a custom endpoint — re-pick a provider
            via Back, or edit the URL/key and save.
          </div>
        )}
        {error && (
          <div className="px-4 py-2 bg-destructive/10 text-destructive text-xs border-t border-destructive/20">
            {error}
          </div>
        )}

        <DialogFooter className="border-t border-border">
          {step === "fill-form" && !editing && (
            <Button
              type="button"
              variant="ghost"
              size="sm"
              onClick={() => setStep("pick-provider")}
            >
              Back
            </Button>
          )}
          <Button type="button" variant="ghost" size="sm" onClick={onClose}>
            Cancel
          </Button>
          {step === "fill-form" && (
            <Button type="button" size="sm" onClick={handleSave} disabled={saving}>
              {saving ? "Saving…" : editing ? "Save changes" : "Create profile"}
            </Button>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

// ---------------------------------------------------------------------------
// Step 1: provider grid
// ---------------------------------------------------------------------------

function ProviderGrid({
  catalog,
  onPick,
}: {
  catalog: CatalogEntry[];
  onPick: (c: CatalogEntry) => void;
}) {
  const [query, setQuery] = useState("");
  const normalizedQuery = query.trim().toLowerCase();
  const filteredCatalog = useMemo(() => {
    if (!normalizedQuery) return catalog;
    return catalog.filter((provider) =>
      providerSearchText(provider).includes(normalizedQuery),
    );
  }, [catalog, normalizedQuery]);

  if (catalog.length === 0) {
    return (
      <p className="text-xs text-muted-foreground">
        No providers found. The catalog ships with the desktop binary; if you
        see this, the install is broken.
      </p>
    );
  }
  return (
    <div className="space-y-3">
      <div className="relative">
        <Search className="absolute left-2.5 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground" />
        <Input
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="Search providers"
          className="h-8 pl-8 text-[13px]"
          autoFocus
        />
      </div>

      <div className="space-y-2">
        <div className="flex items-center justify-between">
          <div className="text-xs font-semibold">Preset providers</div>
          <Badge variant="muted" className="tabular-nums">
            {filteredCatalog.length}
          </Badge>
        </div>
        {filteredCatalog.length === 0 ? (
          <div className="rounded-md border border-dashed border-border px-3 py-6 text-center text-xs text-muted-foreground">
            No matching providers
          </div>
        ) : (
          <div className={PROVIDER_TILE_GRID}>
            {filteredCatalog.map((provider) => (
              <ProviderTile
                key={provider.id}
                provider={provider}
                onPick={() => onPick(provider)}
              />
            ))}
          </div>
        )}
      </div>

      <div className="space-y-2 border-t border-border/60 pt-3">
        <div className="text-xs font-semibold">Custom</div>
        <div className={PROVIDER_TILE_GRID}>
          <ProviderTile
            provider={CUSTOM_PROVIDER}
            onPick={() => onPick(CUSTOM_PROVIDER)}
            dashed
            description="Bring your own URL + key"
          />
        </div>
      </div>
    </div>
  );
}

function ProviderTile({
  provider,
  onPick,
  dashed,
  description,
}: {
  provider: CatalogEntry;
  onPick: () => void;
  dashed?: boolean;
  description?: string;
}) {
  const endpoints = provider.endpoints.filter((e) =>
    isProviderApiKind(e.api_type),
  );
  const subtitle = description ?? (provider.homepage ? hostnameOf(provider.homepage) : null);

  return (
    <button
      type="button"
      onClick={onPick}
      className={`flex min-h-[102px] w-full flex-col items-start gap-1 rounded-md border p-2 text-left transition-colors hover:border-primary hover:bg-accent/30 ${
        dashed ? "border-dashed border-border" : "border-border"
      }`}
    >
      <div className="flex items-center gap-2">
        <BrandIcon
          kind="provider"
          id={provider.id}
          label={provider.label}
          fallback={provider.icon}
          className="h-6 w-6"
        />
        <span className="text-[13px] font-medium">{provider.label}</span>
      </div>
      <div className="flex flex-wrap gap-1 mt-1">
        {endpoints.map((e) => (
          <Badge key={e.api_type} variant="muted" className="text-[10px]">
            {apiTypeShort(e.api_type)}
          </Badge>
        ))}
      </div>
      {subtitle && (
        <span className="text-[10px] text-muted-foreground/60 truncate w-full">
          {subtitle}
        </span>
      )}
    </button>
  );
}

// ---------------------------------------------------------------------------
// Step 2: form body
// ---------------------------------------------------------------------------

interface FormBodyProps {
  provider: CatalogEntry;
  editing: boolean;
  label: string;
  setLabel: (v: string) => void;
  selectedApiTypes: string[];
  setSelectedApiTypes: (v: string[]) => void;
  credentials: Record<string, string>;
  setCredentials: (v: Record<string, string>) => void;
  overrides: Record<string, ApiTypeOverrides>;
  setOverrides: (v: Record<string, ApiTypeOverrides>) => void;
  revealKeys: Record<string, boolean>;
  setRevealKeys: (v: Record<string, boolean>) => void;
}

function FormBody(props: FormBodyProps) {
  const {
    provider,
    label,
    setLabel,
    selectedApiTypes,
    setSelectedApiTypes,
    credentials,
    setCredentials,
    overrides,
    setOverrides,
    revealKeys,
    setRevealKeys,
  } = props;

  const fieldDefs = collectFields(provider, selectedApiTypes, "api_key");
  const apiKindEndpoints = provider.endpoints.filter((e) =>
    isProviderApiKind(e.api_type),
  );
  const apiKindsEditable = provider.id === "custom";

  return (
    <div className="space-y-3">
      <FormSection title="Profile">
        <FieldRow label="Label" hint="Visible name for this profile.">
          <Input
            type="text"
            value={label}
            onChange={(e) => setLabel(e.target.value)}
            placeholder={`${provider.label} (work)`}
            className={INPUT_CLASS}
          />
        </FieldRow>

        <ApiKindsField
          endpoints={apiKindEndpoints}
          editable={apiKindsEditable}
          selectedApiTypes={selectedApiTypes}
          setSelectedApiTypes={setSelectedApiTypes}
        />
      </FormSection>

      {fieldDefs.length > 0 && (
        <FormSection title="Credentials">
          {fieldDefs.map((f) => (
            <CredentialField
              key={f.name}
              field={f}
              value={credentials[f.name] ?? ""}
              reveal={revealKeys[f.name] ?? false}
              onChange={(v) => setCredentials({ ...credentials, [f.name]: v })}
              onToggleReveal={() =>
                setRevealKeys({ ...revealKeys, [f.name]: !revealKeys[f.name] })
              }
            />
          ))}
        </FormSection>
      )}

      {selectedApiTypes.length > 0 && (
        <FormSection title="Model settings">
          <div className="space-y-2">
            {selectedApiTypes.map((apiType) => {
              const ep = provider.endpoints.find((e) => e.api_type === apiType);
              if (!ep) return null;
              const ov = overrides[apiType] ?? {};
              return (
                <div key={apiType} className="border border-border/60 rounded-md p-2.5 space-y-2">
                  <div className="flex items-center gap-2 text-xs">
                    <span className="font-mono px-1.5 py-0.5 rounded bg-muted">
                      {apiTypeShort(apiType)}
                    </span>
                    <span className="text-muted-foreground">{apiType}</span>
                  </div>
                  {shouldShowBaseUrl(provider, ep, ov) && (
                    <FieldRow
                      label={provider.id === "azure" ? "Endpoint" : "Base URL"}
                      required={ep.default_base_url === ""}
                      hint={
                        ep.default_base_url
                          ? "Leave blank to use the catalog default."
                          : provider.id === "custom"
                          ? "Required for custom endpoints."
                          : "Endpoint URL from the provider dashboard."
                      }
                    >
                      <Input
                        type="text"
                        value={ov.base_url ?? ""}
                        onChange={(e) =>
                          setOverrides({
                            ...overrides,
                            [apiType]: { ...ov, base_url: e.target.value },
                          })
                        }
                        placeholder={
                          ep.default_base_url ||
                          (provider.id === "azure"
                            ? "https://your-resource.openai.azure.com/openai/v1"
                            : "https://your-endpoint.example.com/v1")
                        }
                        className={MONO_INPUT_CLASS}
                      />
                    </FieldRow>
                  )}
                  <FieldRow
                    label={provider.id === "azure" ? "Deployment name" : "Model"}
                    hint={apiKindHint(provider, apiType)}
                  >
                    {ep.models.length > 0 ? (
                      <Select
                        value={ov.model ?? ""}
                        onValueChange={(value) =>
                          setOverrides({
                            ...overrides,
                            [apiType]: { ...ov, model: value },
                          })
                        }
                      >
                        <SelectTrigger size="sm" className="h-8 w-full text-[13px]">
                          <SelectValue placeholder="Select a model" />
                        </SelectTrigger>
                        <SelectContent>
                          {ep.models.map((m) => (
                            <SelectItem key={m.id} value={m.id} className="text-xs">
                              {m.label ?? m.id}
                            </SelectItem>
                          ))}
                        </SelectContent>
                      </Select>
                    ) : (
                      <Input
                        type="text"
                        value={ov.model ?? ""}
                        onChange={(e) =>
                          setOverrides({
                            ...overrides,
                            [apiType]: { ...ov, model: e.target.value },
                          })
                        }
                        placeholder="model id (e.g. gpt-4o, claude-sonnet-4-6)"
                        className={MONO_INPUT_CLASS}
                      />
                    )}
                  </FieldRow>
                  {ep.capabilities?.reasoning_effort && (
                    <FieldRow label="Reasoning effort">
                      <Select
                        value={ov.reasoning_effort ?? "medium"}
                        onValueChange={(value) =>
                          setOverrides({
                            ...overrides,
                            [apiType]: { ...ov, reasoning_effort: value },
                          })
                        }
                      >
                        <SelectTrigger size="sm" className="h-8 w-full text-[13px]">
                          <SelectValue />
                        </SelectTrigger>
                        <SelectContent>
                          <SelectItem value="low" className="text-xs">low</SelectItem>
                          <SelectItem value="medium" className="text-xs">medium</SelectItem>
                          <SelectItem value="high" className="text-xs">high</SelectItem>
                          <SelectItem value="xhigh" className="text-xs">xhigh</SelectItem>
                        </SelectContent>
                      </Select>
                    </FieldRow>
                  )}
                </div>
              );
            })}
          </div>
        </FormSection>
      )}

      {provider.homepage && (
        <a
          href={provider.homepage}
          target="_blank"
          rel="noopener noreferrer"
          className="text-[11px] text-primary hover:underline flex items-center gap-1"
        >
          <Globe className="w-3 h-3" /> {provider.homepage}
        </a>
      )}
    </div>
  );
}

function FormSection({
  title,
  children,
}: {
  title: string;
  children: React.ReactNode;
}) {
  return (
    <section className="space-y-3 border-t border-border/60 pt-3 first:border-t-0 first:pt-0">
      <div className="text-xs font-semibold">{title}</div>
      {children}
    </section>
  );
}

function ApiKindsField({
  endpoints,
  editable,
  selectedApiTypes,
  setSelectedApiTypes,
}: {
  endpoints: CatalogEntry["endpoints"];
  editable: boolean;
  selectedApiTypes: string[];
  setSelectedApiTypes: (v: string[]) => void;
}) {
  return (
    <div>
      <div className="text-[11px] font-medium text-muted-foreground mb-1">
        API kinds
      </div>
      <div className="flex flex-wrap gap-1.5">
        {endpoints.map((ep) => {
          const checked = selectedApiTypes.includes(ep.api_type);
          if (editable) {
            return (
              <label
                key={ep.api_type}
                className={`h-8 flex items-center gap-2 px-2.5 border rounded-md cursor-pointer text-xs ${
                  checked ? "border-primary bg-primary/10" : "border-border hover:bg-accent/30"
                }`}
              >
                <input
                  type="checkbox"
                  checked={checked}
                  onChange={(e) => {
                    if (e.target.checked) {
                      setSelectedApiTypes([...selectedApiTypes, ep.api_type]);
                    } else {
                      setSelectedApiTypes(
                        selectedApiTypes.filter((a) => a !== ep.api_type),
                      );
                    }
                  }}
                  className="h-3.5 w-3.5 accent-primary"
                />
                <span className="font-mono">{apiTypeShort(ep.api_type)}</span>
                <span className="text-muted-foreground/70">
                  · {apiTypeLabel(ep.api_type)}
                </span>
              </label>
            );
          }
          return (
            <div
              key={ep.api_type}
              className="h-8 flex items-center gap-2 px-2.5 border border-primary bg-primary/10 rounded-md text-xs"
            >
              <span className="font-mono">{apiTypeShort(ep.api_type)}</span>
              <span className="text-muted-foreground/70">
                · {apiTypeLabel(ep.api_type)}
              </span>
            </div>
          );
        })}
      </div>
      {editable && (
        <p className="text-[10px] text-muted-foreground/60 mt-1">
          Select every API shape this endpoint supports.
        </p>
      )}
    </div>
  );
}

function CredentialField({
  field,
  value,
  reveal,
  onChange,
  onToggleReveal,
}: {
  field: FieldDef;
  value: string;
  reveal: boolean;
  onChange: (v: string) => void;
  onToggleReveal: () => void;
}) {
  return (
    <FieldRow label={field.label} required={field.required}>
      <div className="relative">
        <Input
          type={field.secret && !reveal ? "password" : "text"}
          value={value}
          onChange={(e) => onChange(e.target.value)}
          placeholder={field.placeholder ?? undefined}
          className={SECRET_INPUT_CLASS}
        />
        {field.secret && (
          <button
            type="button"
            onClick={onToggleReveal}
            className="absolute right-1 top-1/2 -translate-y-1/2 p-1 text-muted-foreground hover:text-foreground"
            aria-label={reveal ? "Hide" : "Reveal"}
          >
            {reveal ? <EyeOff className="w-3 h-3" /> : <Eye className="w-3 h-3" />}
          </button>
        )}
      </div>
    </FieldRow>
  );
}

function FieldRow({
  label,
  hint,
  required,
  children,
}: {
  label: string;
  hint?: string;
  required?: boolean;
  children: React.ReactNode;
}) {
  return (
    <label className="block">
      <div className="text-[11px] font-medium text-muted-foreground mb-0.5">
        {label}
        {required && <span className="text-destructive ml-0.5">*</span>}
      </div>
      {children}
      {hint && (
        <div className="text-[10px] text-muted-foreground/60 mt-0.5">{hint}</div>
      )}
    </label>
  );
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/**
 * Walk the selected api_types and union their auth-mode-matching `fields[]`
 * by `name`. Two endpoints of the same provider should declare the same
 * field for a given credential, so this dedupes on the catalog side rather
 * than asking the user to re-enter the same api_key for each protocol.
 */
function collectFields(
  provider: CatalogEntry,
  apiTypes: string[],
  mode: string,
): FieldDef[] {
  const seen = new Map<string, FieldDef>();
  for (const apiType of apiTypes) {
    const ep = provider.endpoints.find((e) => e.api_type === apiType);
    if (!ep) continue;
    const auth = ep.auth_modes.find((a: AuthModeDef) => a.mode === mode);
    if (!auth) continue;
    for (const f of auth.fields) {
      if (!seen.has(f.name)) seen.set(f.name, f);
    }
  }
  return Array.from(seen.values());
}

function hostnameOf(url: string): string {
  try {
    return new URL(url).hostname;
  } catch {
    return url;
  }
}

function providerSearchText(provider: CatalogEntry): string {
  const parts = [
    provider.id,
    provider.label,
    provider.homepage ?? "",
    ...provider.endpoints
      .filter((endpoint) => isProviderApiKind(endpoint.api_type))
      .flatMap((endpoint) => [
        endpoint.api_type,
        apiTypeShort(endpoint.api_type),
        apiTypeLabel(endpoint.api_type),
      ]),
  ];
  return parts.join(" ").toLowerCase();
}

function stripEmpty(map: Record<string, string>): Record<string, string> {
  const out: Record<string, string> = {};
  for (const [k, v] of Object.entries(map)) {
    if (v) out[k] = v;
  }
  return out;
}

function arraysEqual(a: string[], b: string[]): boolean {
  return a.length === b.length && a.every((item, index) => item === b[index]);
}

function shouldShowBaseUrl(
  provider: CatalogEntry,
  endpoint: { default_base_url: string },
  overrides: ApiTypeOverrides,
): boolean {
  if (provider.id === "custom") return true;
  if (!endpoint.default_base_url) return true;
  return !!overrides.base_url && overrides.base_url !== endpoint.default_base_url;
}

function apiKindHint(provider: CatalogEntry, apiType: string): string | undefined {
  if (provider.id !== "azure") return undefined;
  if (apiType === "openai-responses") {
    return "Used by Codex and OpenCode for reasoning/tools. Must be an Azure deployment that supports the Responses API.";
  }
  if (apiType === "openai-chat") {
    return "Chat Completions fallback for CLIs/providers that cannot use Responses.";
  }
  return undefined;
}

/**
 * Strip override values that match the catalog default — keeps profile.json
 * minimal AND lets future catalog updates flow through automatically. If
 * we always saved the visible value (which the form pre-fills), users
 * who never touched base_url would still be locked to the URL that was
 * default when they created the profile, missing later catalog fixes.
 */
function pruneOverrides(
  overrides: Record<string, ApiTypeOverrides>,
  apiTypes: string[],
  provider: CatalogEntry,
): Record<string, ApiTypeOverrides> {
  const out: Record<string, ApiTypeOverrides> = {};
  for (const apiType of apiTypes) {
    const ov = overrides[apiType];
    if (!ov) continue;
    const ep = provider.endpoints.find((e) => e.api_type === apiType);
    const defaultBaseUrl = ep?.default_base_url ?? "";
    const trimmed: ApiTypeOverrides = {};
    if (ov.model && ov.model.length > 0) trimmed.model = ov.model;
    if (ep?.capabilities?.reasoning_effort && ov.reasoning_effort) {
      trimmed.reasoning_effort = ov.reasoning_effort;
    }
    // Only persist base_url if user changed it from the catalog default —
    // otherwise leave it for render-time fallback.
    if (ov.base_url && ov.base_url.length > 0 && ov.base_url !== defaultBaseUrl) {
      trimmed.base_url = ov.base_url;
    }
    if (Object.keys(trimmed).length > 0) out[apiType] = trimmed;
  }
  return out;
}
