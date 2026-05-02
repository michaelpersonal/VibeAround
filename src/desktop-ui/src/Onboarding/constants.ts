// Agent and tunnel IDs/labels are loaded from the backend via Tauri commands.
// Only static UI constants remain here.

export type AgentId = string;
export type TunnelProvider = string;

export const STEPS = ["Welcome", "Quick Launch", "Channels", "Tunnel", "Confirm"] as const;
export type OnboardingStep = (typeof STEPS)[number];
