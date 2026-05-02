import { MessageSquare, Download, ExternalLink, Loader2 } from "lucide-react";
import { QRCodeSVG } from "qrcode.react";

import { Alert } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";

import type { StepChannelsProps, ConfigSchemaProperty } from "../types";

/** Determine if a config field should use password input. */
function isSecretField(key: string): boolean {
  const lower = key.toLowerCase();
  return lower.includes("token") || lower.includes("secret") || lower.includes("password") || lower.includes("key");
}

/** Generate a human-readable label from a JSON schema property. */
function fieldLabel(key: string, prop: ConfigSchemaProperty): string {
  if (prop.description) return prop.description;
  return key.replace(/_/g, " ").replace(/\b\w/g, (c) => c.toUpperCase());
}

export function StepChannels({
  pluginRegistry,
  discoveredPlugins,
  enabledChannels,
  channelConfigs,
  installingPlugins,
  authStates,
  onToggleChannel,
  onConfigChange,
  onInstallPlugin,
  onStartAuth,
  onCancelAuth,
}: StepChannelsProps) {
  const discoveredMap = new Map(discoveredPlugins.map((p) => [p.id, p]));

  return (
    <div className="space-y-5">
      <div>
        <h2 className="text-base font-semibold flex items-center gap-2">
          <MessageSquare className="w-4 h-4 text-primary" />
          IM Channels
        </h2>
        <p className="text-xs text-muted-foreground mt-1">
          Connect messaging bots to vibe code from your phone. Install plugins
          from the registry, then configure and enable them.
        </p>
      </div>

      {pluginRegistry.map((entry) => {
        const discovered = discoveredMap.get(entry.id);
        const installing = installingPlugins.has(entry.id);
        const isReady = !!discovered;
        const enabled = enabledChannels.has(entry.id);
        const config = channelConfigs[entry.id] ?? {};
        const authState = authStates[entry.id];
        return (
          <PluginCard
            key={entry.id}
            pluginId={entry.id}
            name={entry.name}
            description={entry.description}
            githubUrl={entry.github}
            isReady={isReady}
            installing={installing}
            enabled={enabled}
            discovered={discovered}
            config={config}
            authState={authState}

            onToggle={(v) => onToggleChannel(entry.id, v)}
            onConfigChange={(k, v) => onConfigChange(entry.id, k, v)}
            onInstall={() => onInstallPlugin(entry.id, entry.github)}
            onStartAuth={() => onStartAuth(entry.id)}
            onCancelAuth={() => onCancelAuth(entry.id)}
          />
        );
      })}
    </div>
  );
}

// ---------------------------------------------------------------------------
// PluginCard — renders install state, config form, or auth flow
// ---------------------------------------------------------------------------

interface PluginCardProps {
  pluginId: string;
  name: string;
  description: string;
  githubUrl: string;
  isReady: boolean;
  installing: boolean;
  enabled: boolean;
  discovered?: StepChannelsProps["discoveredPlugins"][number];
  config: Record<string, string>;
  authState?: StepChannelsProps["authStates"][string];

  onToggle: (enabled: boolean) => void;
  onConfigChange: (key: string, value: string) => void;
  onInstall: () => void;
  onStartAuth: () => void;
  onCancelAuth: () => void;
}

function PluginCard({
  pluginId: _pluginId,
  name,
  description,
  githubUrl,
  isReady,
  installing,
  enabled,
  discovered,
  config,
  authState,
  onToggle,
  onConfigChange,
  onInstall,
  onStartAuth,
  onCancelAuth,
}: PluginCardProps) {
  const supportsAuth = discovered?.supportsQrcodeLogin ?? false;
  const schema = discovered?.configSchema;
  const properties = schema?.properties ?? {};
  const required = new Set(schema?.required ?? []);
  const visibleFields = Object.entries(properties).filter(
    ([, prop]) => !prop.hidden
  );

  return (
    <section className="rounded-xl border border-border bg-card overflow-hidden scroll-mt-4">
      <div className="flex items-start justify-between gap-4 px-4 py-4">
        <div className="space-y-1 min-w-0">
          <div className="flex items-center gap-2">
            <span className="text-sm font-medium">{name}</span>
            <a
              href={githubUrl}
              target="_blank"
              rel="noopener noreferrer"
              className="text-muted-foreground hover:text-primary transition-colors"
              title="View on GitHub"
            >
              <ExternalLink className="w-3 h-3" />
            </a>
          </div>
          <div className="text-xs text-muted-foreground max-w-xl">{description}</div>
        </div>

        {isReady ? (
          <Switch
            checked={enabled}
            onCheckedChange={onToggle}
            aria-label={`Toggle ${name}`}
          />
        ) : installing ? (
          <Button
            key="installing"
            disabled
            size="sm"
            className="text-xs"
          >
            <Loader2 className="w-3 h-3 animate-spin" />
            Installing…
          </Button>
        ) : (
          <Button
            key="install"
            type="button"
            onClick={onInstall}
            size="sm"
            className="text-xs"
          >
            <Download className="w-3 h-3" />
            Install
          </Button>
        )}
      </div>

      {/* Config form + auth (only when installed AND enabled) */}
      {isReady && enabled && (
        <div className="border-t border-border px-4 py-4 space-y-3">
          {/* Dynamic config fields from configSchema */}
          {visibleFields.length > 0 && (
            <div className="space-y-2">
              {visibleFields.map(([key, prop]) => (
                <label key={key} className="block">
                  <span className="text-xs text-muted-foreground">
                    {fieldLabel(key, prop)}
                    {required.has(key) && <span className="text-destructive ml-0.5">*</span>}
                  </span>
                  <Input
                    type={isSecretField(key) ? "password" : "text"}
                    value={config[key] ?? prop.default ?? ""}
                    onChange={(e) => onConfigChange(key, e.target.value)}
                    placeholder={prop.default ?? ""}
                    className="mt-1"
                  />
                </label>
              ))}
            </div>
          )}

          {/* Auth flow (QR login) */}
          {supportsAuth && (
            <AuthFlowSection
              authState={authState}
              onStart={onStartAuth}
              onCancel={onCancelAuth}
            />
          )}
        </div>
      )}
    </section>
  );
}

// ---------------------------------------------------------------------------
// AuthFlowSection — QR code login / pairing code
// ---------------------------------------------------------------------------

function AuthFlowSection({
  authState,
  onStart,
  onCancel,
}: {
  authState?: { status: string; message: string; qrCodeUrl?: string };
  onStart: () => void;
  onCancel: () => void;
}) {
  const status = authState?.status ?? "idle";
  const isBusy = status === "generating" || status === "waiting";

  return (
    <Card className="space-y-3 bg-muted/20 p-3">
      <div className="flex items-center justify-between gap-3">
        <div>
          <div className="text-sm font-medium">QR Login</div>
          <div className="text-xs text-muted-foreground mt-1">
            Generate a QR code, scan it with the app, then wait for authorization.
          </div>
        </div>
        <div className="flex items-center gap-2">
          {isBusy && (
            <Button
              type="button"
              onClick={onCancel}
              variant="outline"
              size="sm"
              className="text-xs"
            >
              Cancel
            </Button>
          )}
          <Button
            type="button"
            onClick={onStart}
            disabled={isBusy}
            size="sm"
            className="text-xs"
          >
            {status === "connected"
              ? "Reconnect"
              : isBusy
                ? "Waiting…"
                : "Connect"}
          </Button>
        </div>
      </div>

      {authState?.message && (
        <Alert
          variant={
            status === "error"
              ? "destructive"
              : status === "connected"
                ? "success"
                : "default"
          }
        >
          {authState.message}
        </Alert>
      )}

      {authState?.qrCodeUrl && (
        <div className="flex flex-col items-center gap-2 pt-1 scroll-mt-6">
          <div className="rounded-lg border bg-white p-3 shadow-sm">
            <QRCodeSVG
              value={authState.qrCodeUrl}
              size={176}
              bgColor="#ffffff"
              fgColor="#111111"
              level="M"
              includeMargin
              title="QR code"
            />
          </div>
          <div className="text-[11px] text-muted-foreground text-center">
            Scan with the app and confirm on your phone.
          </div>
        </div>
      )}
    </Card>
  );
}
