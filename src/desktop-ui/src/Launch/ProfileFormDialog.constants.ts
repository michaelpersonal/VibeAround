import type { CatalogEntry } from "./types";

export const PROVIDER_TILE_GRID =
  "grid grid-cols-[repeat(auto-fill,minmax(180px,1fr))] gap-2";

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
 * same provider can host multiple profiles and the on-disk filename still
 * reflects the provider for at-a-glance inspection.
 */
export function generateProfileId(providerId: string): string {
  const random = Math.random().toString(36).slice(2, 10).padEnd(8, "0");
  return `${providerId}-${random}`;
}

export const INPUT_CLASS = "h-8 text-[13px]";
export const MONO_INPUT_CLASS = "h-8 text-[13px] font-mono";
export const SECRET_INPUT_CLASS = "h-8 pr-8 text-[13px] font-mono";
