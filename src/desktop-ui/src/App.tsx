import { useCallback, useEffect, useRef, useState } from "react";
import {
  Globe, Bot, MessageSquare, Terminal, X, RefreshCw, ExternalLink, Server, Wifi, WifiOff, FolderOpen, Eye, Play,
} from "lucide-react";
import type { TunnelStatus } from "@va/client";
import { useChannelsState, type ChannelRuntime } from "./hooks/useChannelsState";
import { useTunnelsState, type TunnelRuntime } from "./hooks/useTunnelsState";
import { useAgentsRuntime, type AgentRuntime } from "./hooks/useAgentsRuntime";
import { openDashboardUrl, DAEMON_PORT } from "./lib/api";
import { Splash } from "./Splash";
import Onboarding from "./Onboarding";
import { Workspaces } from "./Workspaces";
import { Previews } from "./Previews";

// ---------------------------------------------------------------------------
// Per-domain status presentation — each manager has its own natural status
// shape (channel: string enum; tunnel: TunnelStatus; agent: derived
// from busy/failed flags), so each gets its own mapping.
// ---------------------------------------------------------------------------

type Pres = { label: string; color: string; running: boolean };

function channelStatusPresentation(status: ChannelRuntime["status"]): Pres {
  switch (status) {
    case "running":     return { label: "Running",     color: "bg-emerald-500", running: true };
    case "spawning":    return { label: "Spawning",    color: "bg-amber-500",   running: false };
    case "not_started": return { label: "Not started", color: "bg-zinc-400",    running: false };
    case "stopped":     return { label: "Stopped",     color: "bg-zinc-400",    running: false };
    case "crashed":     return { label: "Crashed",     color: "bg-red-500",     running: false };
  }
}

function tunnelStatusPresentation(status: TunnelStatus): Pres {
  switch (status.state) {
    case "running": return { label: "Running", color: "bg-emerald-500", running: true };
    case "stopped": return { label: "Stopped", color: "bg-zinc-400",    running: false };
    case "failed":  return { label: "Failed",  color: "bg-red-500",     running: false };
  }
}

function agentStatusPresentation(agent: AgentRuntime): Pres {
  if (agent.failed) return { label: "Failed",  color: "bg-red-500",     running: false };
  if (agent.busy)   return { label: "Busy",    color: "bg-amber-500",   running: true };
  return              { label: "Idle",    color: "bg-emerald-500", running: true };
}

function StatusDot({ colorClass }: { colorClass: string }) {
  return <span className={`inline-block w-2 h-2 rounded-full ${colorClass}`} />;
}

// ---------------------------------------------------------------------------
// Per-domain row components
// ---------------------------------------------------------------------------

function ChannelRow({ channel, onStart, onStop }: {
  channel: ChannelRuntime;
  onStart: () => void;
  onStop: () => void;
}) {
  const pres = channelStatusPresentation(channel.status);
  const showRestartIn = channel.status === "crashed" && channel.restart_in_secs > 0;
  return (
    <Row
      dot={pres.color}
      name={capitalize(channel.kind)}
      label={pres.label}
      running={pres.running}
      title={channel.reason ?? pres.label}
      suffix={showRestartIn ? ` · retry ${channel.restart_in_secs}s` : null}
      actions={
        <>
          {!pres.running && (
            <IconBtn onClick={onStart} title="Start" icon={<Play className="w-3 h-3" />} hover="emerald" />
          )}
          {pres.running && (
            <IconBtn onClick={onStop} title="Stop" icon={<X className="w-3 h-3" />} hover="destructive" />
          )}
        </>
      }
    />
  );
}

function TunnelRow({ tunnel, onKill }: { tunnel: TunnelRuntime; onKill: () => void }) {
  const pres = tunnelStatusPresentation(tunnel.status);
  const tooltip =
    tunnel.status.state === "stopped" ? (tunnel.status.reason ?? pres.label)
    : tunnel.status.state === "failed" ? tunnel.status.error
    : pres.label;
  return (
    <Row
      dot={pres.color}
      name={`Tunnel (${tunnel.provider})`}
      label={pres.label}
      running={pres.running}
      title={tooltip}
      secondary={tunnel.provider}
      tailLink={tunnel.url ? { url: tunnel.url } : undefined}
      actions={
        pres.running ? (
          <IconBtn onClick={onKill} title="Stop" icon={<X className="w-3 h-3" />} hover="destructive" />
        ) : null
      }
    />
  );
}

function AgentRow({ agent, onKill }: { agent: AgentRuntime; onKill: () => void }) {
  const pres = agentStatusPresentation(agent);
  const kindLabel = agent.cli_kind ?? "agent";
  const name = `${kindLabel} (${agent.route_key})`;
  return (
    <Row
      dot={pres.color}
      name={name}
      label={pres.label}
      running={pres.running}
      title={agent.failed ?? agent.session_id ?? pres.label}
      secondary={agent.agent_version ? `v${agent.agent_version}` : undefined}
      actions={
        pres.running ? (
          <IconBtn onClick={onKill} title="Stop" icon={<X className="w-3 h-3" />} hover="destructive" />
        ) : null
      }
    />
  );
}

function Row({ dot, name, label, running, title, suffix, secondary, tailLink, actions }: {
  dot: string;
  name: string;
  label: string;
  running: boolean;
  title: string;
  suffix?: React.ReactNode;
  secondary?: string;
  tailLink?: { url: string };
  actions?: React.ReactNode;
}) {
  return (
    <div className="flex items-center gap-2 py-1.5 px-2 rounded-md hover:bg-accent/50 transition-colors group">
      <StatusDot colorClass={dot} />
      <span className="text-xs font-medium flex-1 truncate">{name}</span>
      {secondary && (
        <span className="text-[10px] text-muted-foreground/70 truncate max-w-[100px]">{secondary}</span>
      )}
      <span
        className={`text-[10px] tabular-nums ${running ? "text-muted-foreground/60" : "text-muted-foreground/80"}`}
        title={title}
      >
        {label}
        {suffix && <span className="text-muted-foreground/50">{suffix}</span>}
      </span>
      {tailLink && (
        <button
          type="button"
          onClick={(e) => {
            e.preventDefault();
            void openDashboardUrl(tailLink.url);
          }}
          className="text-muted-foreground/50 hover:text-primary"
          title={tailLink.url}
        >
          <ExternalLink className="w-3 h-3" />
        </button>
      )}
      {actions}
    </div>
  );
}

function IconBtn({ onClick, title, icon, hover }: {
  onClick: () => void;
  title: string;
  icon: React.ReactNode;
  hover: "destructive" | "emerald";
}) {
  const hoverClass = hover === "destructive"
    ? "hover:text-destructive"
    : "hover:text-emerald-500";
  return (
    <button
      onClick={onClick}
      className={`text-muted-foreground/40 ${hoverClass} opacity-0 group-hover:opacity-100 transition-opacity`}
      title={title}
    >
      {icon}
    </button>
  );
}

function capitalize(s: string): string {
  return s.length === 0 ? s : s[0].toUpperCase() + s.slice(1);
}

// ---------------------------------------------------------------------------
// Routing + Dashboard
// ---------------------------------------------------------------------------

function App() {
  const [route, setRoute] = useState(() => window.location.pathname);

  useEffect(() => {
    const onPop = () => setRoute(window.location.pathname);
    window.addEventListener("popstate", onPop);
    return () => window.removeEventListener("popstate", onPop);
  }, []);

  if (route === "/onboarding") {
    return <Onboarding />;
  }

  return <Dashboard />;
}

type DashboardPage = "services" | "workspaces" | "previews";

function Dashboard() {
  const [page, setPage] = useState<DashboardPage>("services");

  const channels = useChannelsState();
  const tunnels = useTunnelsState();
  const agents = useAgentsRuntime();

  const anyEverLoaded = channels.everLoaded || tunnels.everLoaded || agents.everLoaded;
  const anyConnected = channels.connected || tunnels.connected || agents.connected;
  const firstError = channels.error ?? tunnels.error ?? agents.error ?? null;

  const refreshAll = useCallback(() => {
    void channels.refresh();
    void tunnels.refresh();
    void agents.refresh();
  }, [channels, tunnels, agents]);

  const everHadData = useRef(false);
  const [startTime] = useState(() => Date.now());
  const [timedOut, setTimedOut] = useState(false);

  if (anyEverLoaded) everHadData.current = true;

  useEffect(() => {
    if (anyEverLoaded || everHadData.current) return;
    if (timedOut) return;
    const elapsed = Date.now() - startTime;
    if (elapsed > 30_000) {
      setTimedOut(true);
      return;
    }
    if (firstError) {
      const timer = setTimeout(refreshAll, 2000);
      return () => clearTimeout(timer);
    }
  }, [anyEverLoaded, firstError, timedOut, startTime, refreshAll]);

  const showSplash = !everHadData.current && !anyEverLoaded && !timedOut;
  if (showSplash) return <Splash visible />;

  if (timedOut && !anyEverLoaded) {
    return (
      <div className="flex flex-col items-center justify-center h-full gap-3">
        <p className="text-sm text-destructive">Server failed to start</p>
        <button
          onClick={() => { setTimedOut(false); refreshAll(); }}
          className="text-xs text-primary hover:underline flex items-center gap-1"
        >
          <RefreshCw className="w-3 h-3" /> Retry
        </button>
      </div>
    );
  }

  return (
    <div className="h-full flex flex-col">
      <header className="flex items-center justify-between px-4 py-3 border-b border-border shrink-0">
        <div className="flex items-center gap-0.5 bg-muted rounded-md p-0.5">
          <TabButton active={page === "services"} onClick={() => setPage("services")} icon={<Server className="w-3 h-3" />} label="VibeAround" />
          <TabButton active={page === "workspaces"} onClick={() => setPage("workspaces")} icon={<FolderOpen className="w-3 h-3" />} label="Workspaces" />
          <TabButton active={page === "previews"} onClick={() => setPage("previews")} icon={<Eye className="w-3 h-3" />} label="Previews" />
        </div>
        <div className="flex items-center gap-3">
          <button
            onClick={() => window.location.replace("/onboarding")}
            className="text-xs text-primary hover:underline"
            title="Open Config Wizard"
          >
            Config Wizard
          </button>
          {anyConnected ? (
            <span className="flex items-center gap-1 text-xs text-emerald-600">
              <Wifi className="w-3 h-3" /> Live
            </span>
          ) : (
            <span className="flex items-center gap-1 text-xs text-muted-foreground">
              <WifiOff className="w-3 h-3" /> Polling
            </span>
          )}
          <button
            onClick={refreshAll}
            className="p-1 rounded hover:bg-accent transition-colors"
            title="Refresh"
          >
            <RefreshCw className="w-3.5 h-3.5 text-muted-foreground" />
          </button>
        </div>
      </header>

      {firstError && (
        <div className="px-4 py-1.5 bg-destructive/10 text-destructive text-xs">{firstError}</div>
      )}

      {page === "workspaces" ? (
        <div className="flex-1 overflow-y-auto"><Workspaces /></div>
      ) : page === "previews" ? (
        <div className="flex-1 overflow-y-auto"><Previews /></div>
      ) : (
        <div className="flex-1 overflow-y-auto p-4 space-y-3">
          <Section
            icon={<Globe className="w-4 h-4 text-primary" />}
            title="Tunnel"
            badge={tunnels.tunnels.length}
          >
            {tunnels.tunnels.length === 0 ? (
              <p className="text-xs text-muted-foreground px-3 py-2">No tunnel running</p>
            ) : (
              tunnels.tunnels.map((t) => (
                <TunnelRow key={t.provider} tunnel={t} onKill={() => tunnels.kill(t.provider)} />
              ))
            )}
          </Section>

          <Section
            icon={<Bot className="w-4 h-4 text-primary" />}
            title="Agents"
            badge={agents.agents.length}
          >
            {agents.agents.length === 0 ? (
              <p className="text-xs text-muted-foreground px-3 py-2">No agents running</p>
            ) : (
              agents.agents.map((a) => (
                <AgentRow key={a.route_key} agent={a} onKill={() => agents.kill(a.route_key)} />
              ))
            )}
          </Section>

          <Section
            icon={<MessageSquare className="w-4 h-4 text-primary" />}
            title="Channels"
            badge={channels.channels.length}
          >
            {channels.channels.length === 0 ? (
              <p className="text-xs text-muted-foreground px-3 py-2">No channels running</p>
            ) : (
              channels.channels.map((c) => (
                <ChannelRow
                  key={c.kind}
                  channel={c}
                  onStart={() => channels.start(c.kind)}
                  onStop={() => channels.stop(c.kind)}
                />
              ))
            )}
          </Section>

          <Section
            icon={<Terminal className="w-4 h-4 text-primary" />}
            title="Dashboard"
          >
            <div className="flex items-center justify-between px-3 py-2">
              <span className="text-sm text-muted-foreground">Open the web dashboard</span>
              <button
                type="button"
                onClick={(e) => {
                  e.preventDefault();
                  void openDashboardUrl(`http://127.0.0.1:${DAEMON_PORT}/va/`);
                }}
                className="text-xs text-primary hover:underline flex items-center gap-1"
              >
                Open Web Dashboard <ExternalLink className="w-3 h-3" />
              </button>
            </div>
          </Section>
        </div>
      )}
    </div>
  );
}

function TabButton({ active, onClick, icon, label }: {
  active: boolean;
  onClick: () => void;
  icon: React.ReactNode;
  label: string;
}) {
  return (
    <button
      onClick={onClick}
      className={`px-2.5 py-1 rounded text-xs font-medium transition-colors flex items-center gap-1.5 ${
        active ? "bg-background text-foreground shadow-sm" : "text-muted-foreground hover:text-foreground"
      }`}
    >
      {icon}
      {label}
    </button>
  );
}

function Section({ icon, title, children, badge }: {
  icon: React.ReactNode;
  title: string;
  children: React.ReactNode;
  badge?: string | number;
}) {
  return (
    <div className="border border-border rounded-lg overflow-hidden">
      <div className="flex items-center gap-2 px-3 py-2 bg-muted/40 border-b border-border">
        {icon}
        <span className="text-sm font-semibold">{title}</span>
        {badge !== undefined && (
          <span className="ml-auto text-xs text-muted-foreground bg-muted px-1.5 py-0.5 rounded-full tabular-nums">
            {badge}
          </span>
        )}
      </div>
      <div className="divide-y divide-border/50">{children}</div>
    </div>
  );
}

export default App;
