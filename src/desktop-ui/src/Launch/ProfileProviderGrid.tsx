import { useMemo, useState } from "react";

import { Search } from "lucide-react";

import { BrandIcon } from "@/components/brand-icon";
import { Badge } from "@/components/ui/badge";
import { Input } from "@/components/ui/input";
import {
  CUSTOM_PROVIDER,
  PROVIDER_TILE_GRID,
} from "./ProfileFormDialog.constants";
import { hostnameOf, providerSearchText } from "./profileFormHelpers";
import type { CatalogEntry } from "./types";
import { apiTypeShort, isProviderApiKind } from "./types";

export function ProviderGrid({
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
  const subtitle =
    description ?? (provider.homepage ? hostnameOf(provider.homepage) : null);

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
