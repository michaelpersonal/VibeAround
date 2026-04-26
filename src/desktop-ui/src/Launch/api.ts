import { invoke } from "@tauri-apps/api/core";

import type { CatalogEntry, ProfileDef, ProfileSummary } from "./types";

export function listProfiles(): Promise<ProfileSummary[]> {
  return invoke<ProfileSummary[]>("profiles_list");
}

export function getProfile(id: string): Promise<ProfileDef> {
  return invoke<ProfileDef>("profiles_get", { id });
}

export function upsertProfile(profile: ProfileDef): Promise<void> {
  return invoke<void>("profiles_upsert", { profile });
}

export function deleteProfile(id: string): Promise<void> {
  return invoke<void>("profiles_delete", { id });
}

export function launchProfile(id: string, launchTarget: string): Promise<void> {
  return invoke<void>("profiles_launch", { id, launchTarget });
}

/** Direct launch — no env, CLI uses whatever global OAuth the user has. */
export function launchDirect(agentId: string): Promise<void> {
  return invoke<void>("profiles_launch_direct", { agentId });
}

export interface AgentSummary {
  id: string;
  display_name: string;
  description: string;
  install_type: string | null;
}

/** Reuses the onboarding command that returns all CLIs in agents.json. */
export function listAgents(): Promise<AgentSummary[]> {
  return invoke<AgentSummary[]>("list_agents");
}

export interface TerminalOption {
  id: string;
  label: string;
  installed: boolean;
}

export interface LauncherPreferences {
  terminal: string;
  options: TerminalOption[];
}

export function getLauncherPreferences(): Promise<LauncherPreferences> {
  return invoke<LauncherPreferences>("launcher_get_preferences");
}

export function setLauncherTerminal(terminalId: string): Promise<void> {
  return invoke<void>("launcher_set_terminal", { terminalId });
}

export function listCatalog(): Promise<CatalogEntry[]> {
  return invoke<CatalogEntry[]>("profiles_catalog");
}
