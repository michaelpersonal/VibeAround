import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { ChevronLeft, ChevronRight, Rocket } from "lucide-react";

import { ALL_AGENTS, STEPS } from "./constants";
import { StepAgents } from "./components/StepAgents";
import { StepChannels } from "./components/StepChannels";
import { StepConfirm } from "./components/StepConfirm";
import { StepTunnel } from "./components/StepTunnel";
import { StepWelcome } from "./components/StepWelcome";
import type {
  AuthFlowState,
  DiscoveredChannelPlugin,
  Settings,
} from "./types";
import type { AgentId, OnboardingStep, TunnelProvider } from "./constants";

export default function Onboarding() {
  const [step, setStep] = useState(0);
  const [settings, setSettings] = useState<Settings>({});
  const [discoveredPlugins, setDiscoveredPlugins] = useState<DiscoveredChannelPlugin[]>([]);
  const [loaded, setLoaded] = useState(false);

  // Agents
  const [enabledAgents, setEnabledAgents] = useState<Set<AgentId>>(new Set(ALL_AGENTS));
  const [defaultAgent, setDefaultAgent] = useState<AgentId>("claude");

  // Channels — generic state for all plugins
  const [enabledChannels, setEnabledChannels] = useState<Set<string>>(new Set());
  const [channelConfigs, setChannelConfigs] = useState<Record<string, Record<string, string>>>({});
  const [installingPlugins, setInstallingPlugins] = useState<Set<string>>(new Set());
  const [authStates, setAuthStates] = useState<Record<string, AuthFlowState>>({});

  // Tunnel
  const [tunnelProvider, setTunnelProvider] = useState<TunnelProvider>("none");
  const [ngrokToken, setNgrokToken] = useState("");
  const [ngrokDomain, setNgrokDomain] = useState("");
  const [cfToken, setCfToken] = useState("");
  const [cfHostname, setCfHostname] = useState("");

  const [finishing, setFinishing] = useState(false);

  // ---- Load existing settings ----
  useEffect(() => {
    Promise.all([
      invoke<Settings>("get_settings"),
      invoke<DiscoveredChannelPlugin[]>("list_channel_plugins"),
    ])
      .then(([loadedSettings, plugins]) => {
        setSettings(loadedSettings);
        setDiscoveredPlugins(plugins);

        // Agents
        if (loadedSettings.enabled_agents?.length) {
          setEnabledAgents(new Set(loadedSettings.enabled_agents as AgentId[]));
        }
        if (loadedSettings.default_agent) {
          setDefaultAgent(loadedSettings.default_agent as AgentId);
        }

        // Channels — load from existing settings
        const channels = loadedSettings.channels ?? {};
        const enabled = new Set<string>();
        const configs: Record<string, Record<string, string>> = {};
        for (const [id, channelConfig] of Object.entries(channels)) {
          enabled.add(id);
          const configMap: Record<string, string> = {};
          for (const [key, value] of Object.entries(channelConfig)) {
            if (key !== "verbose" && typeof value === "string") {
              configMap[key] = value;
            }
          }
          configs[id] = configMap;
        }
        setEnabledChannels(enabled);
        setChannelConfigs(configs);

        // Tunnel
        const provider = loadedSettings.tunnel?.provider;
        if (provider === "cloudflare" || provider === "ngrok" || provider === "localtunnel") {
          setTunnelProvider(provider);
        }
        if (loadedSettings.tunnel?.ngrok?.auth_token) setNgrokToken(loadedSettings.tunnel.ngrok.auth_token);
        if (loadedSettings.tunnel?.ngrok?.domain) setNgrokDomain(loadedSettings.tunnel.ngrok.domain);
        if (loadedSettings.tunnel?.cloudflare?.tunnel_token) setCfToken(loadedSettings.tunnel.cloudflare.tunnel_token);
        if (loadedSettings.tunnel?.cloudflare?.hostname) setCfHostname(loadedSettings.tunnel.cloudflare.hostname);

        setLoaded(true);
      })
      .catch(() => setLoaded(true));
  }, []);

  // ---- Channel handlers ----
  const toggleChannel = useCallback((pluginId: string, enabled: boolean) => {
    setEnabledChannels((prev) => {
      const next = new Set(prev);
      if (enabled) next.add(pluginId);
      else next.delete(pluginId);
      return next;
    });
  }, []);

  const updateChannelConfig = useCallback((pluginId: string, key: string, value: string) => {
    setChannelConfigs((prev) => ({
      ...prev,
      [pluginId]: { ...(prev[pluginId] ?? {}), [key]: value },
    }));
  }, []);

  const [installErrors, setInstallErrors] = useState<Record<string, string>>({});

  const installPlugin = useCallback(async (pluginId: string, githubUrl: string) => {
    setInstallingPlugins((prev) => new Set(prev).add(pluginId));
    setInstallErrors((prev) => {
      const next = { ...prev };
      delete next[pluginId];
      return next;
    });
    try {
      const result = await invoke<{ success: boolean; message: string; actualPluginId?: string }>(
        "install_plugin",
        { request: { pluginId, githubUrl } },
      );

      const plugins = await invoke<DiscoveredChannelPlugin[]>("list_channel_plugins");
      setDiscoveredPlugins(plugins);

      // Check whether the plugin actually appeared in discovery
      const found = plugins.some((p) => p.id === pluginId || p.dirName === pluginId);
      if (!found && result.actualPluginId) {
        console.warn(
          `[install] plugin.json id "${result.actualPluginId}" differs from registry id "${pluginId}"; using dirName fallback`,
        );
      }
      if (!found && !result.actualPluginId) {
        setInstallErrors((prev) => ({
          ...prev,
          [pluginId]: "Installed but not detected — check plugin.json has kind \"channel\".",
        }));
      }
    } catch (error) {
      const msg = error instanceof Error ? error.message : String(error);
      console.error(`Failed to install plugin ${pluginId}:`, msg);
      setInstallErrors((prev) => ({ ...prev, [pluginId]: msg }));
    } finally {
      setInstallingPlugins((prev) => {
        const next = new Set(prev);
        next.delete(pluginId);
        return next;
      });
    }
  }, []);

  // ---- Auth flow (generic for any plugin with QR login) ----
  const startAuth = useCallback(async (pluginId: string) => {
    setAuthStates((prev) => ({
      ...prev,
      [pluginId]: { status: "generating", message: "Connecting…" },
    }));

    try {
      // Build config from current channel config + schema defaults
      const discovered = discoveredPlugins.find((p) => p.id === pluginId || p.dirName === pluginId);
      const schemaProps = discovered?.configSchema?.properties ?? {};
      const configForAuth: Record<string, string> = {};
      for (const [key, prop] of Object.entries(schemaProps)) {
        configForAuth[key] = channelConfigs[pluginId]?.[key] ?? prop.default ?? "";
      }

      const result = await invoke<Record<string, unknown>>("plugin_auth_start", {
        request: { pluginId, config: configForAuth },
      });

      if (result.alreadyConnected) {
        setAuthStates((prev) => ({
          ...prev,
          [pluginId]: { status: "connected", message: String(result.message ?? "Already authenticated.") },
        }));
        // Store auth result data back into channel config
        if (result.botToken) updateChannelConfig(pluginId, "bot_token", String(result.botToken));
        if (result.accountId) updateChannelConfig(pluginId, "account_id", String(result.accountId));
        return;
      }

      const qrUrl = result.qrcodeUrl as string | undefined;
      setAuthStates((prev) => ({
        ...prev,
        [pluginId]: {
          status: qrUrl ? "waiting" : "error",
          message: String(result.message ?? "Scan the QR code."),
          qrCodeUrl: qrUrl,
          sessionKey: result.sessionKey as string | undefined,
        },
      }));

      if (!qrUrl) return;

      // Wait for auth completion
      try {
        const waitResult = await invoke<Record<string, unknown>>("plugin_auth_wait", {
          request: {
            pluginId,
            params: {
              sessionKey: result.sessionKey,
              timeoutMs: 480000,
            },
          },
        });

        if (waitResult.connected) {
          setAuthStates((prev) => ({
            ...prev,
            [pluginId]: { status: "connected", message: String(waitResult.message ?? "Connected successfully.") },
          }));
          if (waitResult.botToken) updateChannelConfig(pluginId, "bot_token", String(waitResult.botToken));
          if (waitResult.accountId) updateChannelConfig(pluginId, "account_id", String(waitResult.accountId));
        } else {
          setAuthStates((prev) => ({
            ...prev,
            [pluginId]: { status: "idle", message: String(waitResult.message ?? "Not confirmed.") },
          }));
        }
      } catch {
        setAuthStates((prev) => ({
          ...prev,
          [pluginId]: { status: "error", message: "Connection lost. Try again." },
        }));
      }
    } catch (error) {
      setAuthStates((prev) => ({
        ...prev,
        [pluginId]: { status: "error", message: error instanceof Error ? error.message : String(error) },
      }));
    }
  }, [discoveredPlugins, channelConfigs, updateChannelConfig]);

  const cancelAuth = useCallback(async (pluginId: string) => {
    setAuthStates((prev) => ({
      ...prev,
      [pluginId]: { status: "idle", message: "Cancelled." },
    }));
    try {
      await invoke("plugin_auth_cancel", { request: { pluginId } });
    } catch {
      // ignore
    }
  }, []);

  // ---- Cancel active auth when leaving Channels step ----
  useEffect(() => {
    const currentStep = STEPS[step] as OnboardingStep;
    if (currentStep !== "Channels") {
      for (const [pluginId, state] of Object.entries(authStates)) {
        if (state.status === "generating" || state.status === "waiting") {
          void invoke("plugin_auth_cancel", { request: { pluginId } }).catch(() => {});
        }
      }
    }
  }, [step, authStates]);

  // ---- Build settings ----
  const buildSettings = useCallback((): Settings => {
    const result: Settings = {
      ...settings,
      enabled_agents: Array.from(enabledAgents),
      default_agent: defaultAgent,
    };

    // Channels
    const channels: Record<string, Record<string, unknown>> = {};
    for (const id of enabledChannels) {
      const config: Record<string, unknown> = {};
      const userConfig = channelConfigs[id] ?? {};

      // Merge user config
      for (const [key, value] of Object.entries(userConfig)) {
        if (value) config[key] = value;
      }

      // Fill defaults from schema for hidden fields
      const discovered = discoveredPlugins.find((p) => p.id === id || p.dirName === id);
      if (discovered?.configSchema?.properties) {
        for (const [key, prop] of Object.entries(discovered.configSchema.properties)) {
          if (prop.hidden && prop.default && !config[key]) {
            config[key] = prop.default;
          }
        }
      }

      // Preserve verbose settings
      const existingVerbose = (settings.channels as Record<string, Record<string, unknown>> | undefined)?.[id]?.verbose;
      config.verbose = existingVerbose ?? { show_thinking: false, show_tool_use: false };

      channels[id] = config;
    }

    if (Object.keys(channels).length > 0) {
      result.channels = channels;
    } else {
      delete result.channels;
    }

    // Tunnel
    if (tunnelProvider !== "none") {
      const tunnel: Settings["tunnel"] = { provider: tunnelProvider };
      // localtunnel has no config fields
      if (tunnelProvider === "ngrok") {
        tunnel.ngrok = {};
        if (ngrokToken.trim()) tunnel.ngrok.auth_token = ngrokToken.trim();
        if (ngrokDomain.trim()) tunnel.ngrok.domain = ngrokDomain.trim();
      }
      if (tunnelProvider === "cloudflare") {
        tunnel.cloudflare = {};
        if (cfToken.trim()) tunnel.cloudflare.tunnel_token = cfToken.trim();
        if (cfHostname.trim()) tunnel.cloudflare.hostname = cfHostname.trim();
      }
      result.tunnel = tunnel;
    } else {
      delete result.tunnel;
    }

    return result;
  }, [
    settings,
    enabledAgents,
    defaultAgent,
    enabledChannels,
    channelConfigs,
    discoveredPlugins,
    tunnelProvider,
    ngrokToken,
    ngrokDomain,
    cfToken,
    cfHostname,
  ]);

  const handleFinish = async () => {
    setFinishing(true);
    try {
      const finalSettings = buildSettings();
      await invoke("finish_onboarding", { settings: finalSettings });
      window.location.replace("/");
    } catch (error) {
      console.error("finish_onboarding failed:", error);
      setFinishing(false);
    }
  };

  const toggleAgent = (id: AgentId) => {
    setEnabledAgents((previous) => {
      const next = new Set(previous);
      if (next.has(id)) {
        if (next.size > 1) next.delete(id);
      } else {
        next.add(id);
      }
      if (!next.has(defaultAgent)) {
        setDefaultAgent(Array.from(next)[0]);
      }
      return next;
    });
  };

  if (!loaded) {
    return (
      <div className="flex items-center justify-center h-full">
        <span className="text-sm text-muted-foreground animate-pulse">Loading…</span>
      </div>
    );
  }

  const currentStep = STEPS[step];
  const isLast = step === STEPS.length - 1;
  const canNext = currentStep !== "Agents" || enabledAgents.size > 0;

  return (
    <div className="flex flex-col h-full bg-background">
      <div className="flex items-center gap-1 px-6 pt-5 pb-2">
        {STEPS.map((label, index) => (
          <div key={label} className="flex items-center gap-1 flex-1">
            <div
              className={`h-1 flex-1 rounded-full transition-colors ${
                index <= step ? "bg-primary" : "bg-border"
              }`}
            />
          </div>
        ))}
      </div>
      <div className="px-6 pb-3">
        <span className="text-[10px] text-muted-foreground font-mono uppercase tracking-wider">
          Step {step + 1} of {STEPS.length} — {currentStep}
        </span>
      </div>

      <div className="flex-1 overflow-y-auto px-6 pb-4">
        {currentStep === "Welcome" && <StepWelcome />}
        {currentStep === "Agents" && (
          <StepAgents
            enabled={enabledAgents}
            defaultAgent={defaultAgent}
            onToggle={toggleAgent}
            onSetDefault={setDefaultAgent}
          />
        )}
        {currentStep === "Channels" && (
          <StepChannels
            discoveredPlugins={discoveredPlugins}
            enabledChannels={enabledChannels}
            channelConfigs={channelConfigs}
            installingPlugins={installingPlugins}
            installErrors={installErrors}
            authStates={authStates}
            onToggleChannel={toggleChannel}
            onConfigChange={updateChannelConfig}
            onInstallPlugin={installPlugin}
            onStartAuth={startAuth}
            onCancelAuth={cancelAuth}
          />
        )}
        {currentStep === "Tunnel" && (
          <StepTunnel
            provider={tunnelProvider}
            onProvider={setTunnelProvider}
            ngrokToken={ngrokToken}
            onNgrokToken={setNgrokToken}
            ngrokDomain={ngrokDomain}
            onNgrokDomain={setNgrokDomain}
            cfToken={cfToken}
            onCfToken={setCfToken}
            cfHostname={cfHostname}
            onCfHostname={setCfHostname}
          />
        )}
        {currentStep === "Confirm" && (
          <StepConfirm
            enabledAgents={enabledAgents}
            defaultAgent={defaultAgent}
            tunnelProvider={tunnelProvider}
            enabledChannels={enabledChannels}
            discoveredPlugins={discoveredPlugins}
          />
        )}
      </div>

      <div className="flex items-center justify-between px-6 py-4 border-t border-border shrink-0">
        <button
          onClick={() => setStep((v) => Math.max(0, v - 1))}
          disabled={step === 0}
          className="flex items-center gap-1 text-sm text-muted-foreground hover:text-foreground disabled:opacity-30 disabled:cursor-not-allowed transition-colors"
        >
          <ChevronLeft className="w-4 h-4" />
          Back
        </button>
        {isLast ? (
          <button
            onClick={handleFinish}
            disabled={finishing}
            className="flex items-center gap-2 px-5 py-2 rounded-lg bg-primary text-primary-foreground text-sm font-medium hover:opacity-90 disabled:opacity-50 transition-opacity"
          >
            {finishing ? (
              <>Launching…</>
            ) : (
              <>
                <Rocket className="w-4 h-4" />
                Launch VibeAround
              </>
            )}
          </button>
        ) : (
          <button
            onClick={() => setStep((v) => Math.min(STEPS.length - 1, v + 1))}
            disabled={!canNext}
            className="flex items-center gap-1 px-4 py-2 rounded-lg bg-primary text-primary-foreground text-sm font-medium hover:opacity-90 disabled:opacity-50 transition-opacity"
          >
            {currentStep === "Welcome" ? "Get Started" : "Next"}
            <ChevronRight className="w-4 h-4" />
          </button>
        )}
      </div>
    </div>
  );
}
