/**
 * Wire types for the Launch tab — must mirror the serde shapes emitted by
 * `src/desktop/src/profiles/`. ProfileSummary uses camelCase (matches the
 * Rust `#[serde(rename_all = "camelCase")]`); everything else is
 * snake_case to match the catalog JSON the user can read on disk.
 */

export type AuthMode = "api_key" | "oauth_via_cli";

export interface ProfileSummary {
  id: string;
  label: string;
  provider: string;
  providerLabel: string;
  providerIcon: string | null;
  authMode: AuthMode;
  apiTypes: string[];
  /** `api_type → caveat string`. Populated only for api_types whose
   * catalog endpoint has a `compatibility_warning`. UI shows ⚠ on the
   * matching launch button. */
  apiTypeWarnings: Record<string, string>;
}

export interface ApiTypeOverrides {
  base_url?: string | null;
  model?: string | null;
}

export interface ProfileDef {
  id: string;
  label: string;
  provider: string;
  auth_mode: AuthMode;
  api_types: string[];
  credentials: Record<string, string>;
  overrides: Record<string, ApiTypeOverrides>;
}

export interface ModelDef {
  id: string;
  label?: string | null;
}

export interface FieldDef {
  name: string;
  label: string;
  secret: boolean;
  required: boolean;
  placeholder?: string | null;
  validate?: string | null;
}

export interface AuthModeDef {
  mode: string;
  label?: string | null;
  fields: FieldDef[];
  // `render` is a tagged-pass-through — the UI never needs to introspect
  // it, so we keep it as `unknown` to discourage drift with the renderer.
  render?: unknown | null;
}

export interface EndpointDef {
  api_type: string;
  default_base_url: string;
  models: ModelDef[];
  auth_modes: AuthModeDef[];
  compatibility_warning?: string | null;
}

export interface CatalogEntry {
  id: string;
  label: string;
  icon: string | null;
  homepage: string | null;
  endpoints: EndpointDef[];
}

/** Pretty-print a wire api_type token. */
export function apiTypeLabel(api_type: string): string {
  switch (api_type) {
    case "anthropic":
      return "Claude (Anthropic API)";
    case "openai-chat":
      return "Codex (OpenAI API)";
    default:
      return api_type;
  }
}

/** Short pill label inside cards. */
export function apiTypeShort(api_type: string): string {
  switch (api_type) {
    case "anthropic":
      return "claude";
    case "openai-chat":
      return "codex";
    default:
      return api_type;
  }
}
