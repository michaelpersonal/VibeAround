import type {
  ApiTypeOverrides,
  AuthModeDef,
  CatalogEntry,
  FieldDef,
  ProviderSettings,
} from "./types";
import { apiTypeLabel, apiTypeShort, isProviderApiKind } from "./types";

/**
 * Walk the selected api_types and union their auth-mode-matching `fields[]`
 * by `name`. Two endpoints of the same provider should declare the same
 * field for a given credential, so this dedupes on the catalog side rather
 * than asking the user to re-enter the same api_key for each protocol.
 */
export function collectFields(
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

export function hostnameOf(url: string): string {
  try {
    return new URL(url).hostname;
  } catch {
    return url;
  }
}

export function providerSearchText(provider: CatalogEntry): string {
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

export function stripEmpty(map: Record<string, string>): Record<string, string> {
  const out: Record<string, string> = {};
  for (const [k, v] of Object.entries(map)) {
    if (v) out[k] = v;
  }
  return out;
}

export function arraysEqual(a: string[], b: string[]): boolean {
  return a.length === b.length && a.every((item, index) => item === b[index]);
}

export function shouldShowBaseUrl(
  provider: CatalogEntry,
  endpoint: { default_base_url: string },
  overrides: ApiTypeOverrides,
): boolean {
  if (provider.id === "custom") return true;
  if (!endpoint.default_base_url) return true;
  return !!overrides.base_url && overrides.base_url !== endpoint.default_base_url;
}

export function apiKindHint(
  provider: CatalogEntry,
  apiType: string,
): string | undefined {
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
 * Strip override values that match the catalog default. This keeps
 * profile.json minimal and lets future catalog updates flow through
 * automatically.
 */
export function pruneOverrides(
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
    if (ov.base_url && ov.base_url.length > 0 && ov.base_url !== defaultBaseUrl) {
      trimmed.base_url = ov.base_url;
    }
    if (Object.keys(trimmed).length > 0) out[apiType] = trimmed;
  }
  return out;
}

export function pruneProviderSettings(
  providerId: string,
  settings: ProviderSettings,
): ProviderSettings {
  if (providerId !== "deepseek") return {};

  const deepseek = settings.deepseek ?? {};
  const trimmed = {
    ...(deepseek.thinking ? { thinking: true } : {}),
    ...(deepseek.replay_reasoning_content
      ? { replay_reasoning_content: true }
      : {}),
  };

  return Object.keys(trimmed).length > 0 ? { deepseek: trimmed } : {};
}
