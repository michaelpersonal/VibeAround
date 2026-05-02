import type { DiscoveredChannelPlugin, Settings } from "../types";
import type { AgentId, TunnelProvider } from "../constants";

export interface BuildSettingsInput {
  settings: Settings;
  enabledAgents: Set<AgentId>;
  enabledChannels: Set<string>;
  channelConfigs: Record<string, Record<string, string>>;
  discoveredPlugins: DiscoveredChannelPlugin[];
  tunnelProvider: TunnelProvider;
  ngrokToken: string;
  ngrokDomain: string;
  cfToken: string;
  cfHostname: string;
}

/**
 * Reduce the UI state into the final `settings.json` payload that
 * the daemon reads. Pure — no Tauri calls, no React — so it stays
 * testable in isolation.
 */
export function buildSettings(input: BuildSettingsInput): Settings {
  const {
    settings,
    enabledAgents,
    enabledChannels,
    channelConfigs,
    discoveredPlugins,
    tunnelProvider,
    ngrokToken,
    ngrokDomain,
    cfToken,
    cfHostname,
  } = input;

  const result: Settings = {
    ...settings,
    enabled_agents: Array.from(enabledAgents),
  };
  delete result.default_agent;
  delete result.default_profiles;

  const channels: Record<string, Record<string, unknown>> = {};
  for (const id of enabledChannels) {
    const config: Record<string, unknown> = {};
    const userConfig = channelConfigs[id] ?? {};

    for (const [key, value] of Object.entries(userConfig)) {
      if (value) config[key] = value;
    }

    const discovered = discoveredPlugins.find((p) => p.id === id);
    if (discovered?.configSchema?.properties) {
      for (const [key, prop] of Object.entries(discovered.configSchema.properties)) {
        if (prop.hidden && prop.default && !config[key]) {
          config[key] = prop.default;
        }
      }
    }

    const existingVerbose = (settings.channels as Record<string, Record<string, unknown>> | undefined)
      ?.[id]?.verbose;
    config.verbose = existingVerbose ?? { show_thinking: false, show_tool_use: false };

    channels[id] = config;
  }

  if (Object.keys(channels).length > 0) {
    result.channels = channels;
  } else {
    delete result.channels;
  }

  if (tunnelProvider !== "none") {
    const tunnel: Settings["tunnel"] = { provider: tunnelProvider };
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
}
