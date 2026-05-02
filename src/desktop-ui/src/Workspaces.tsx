import { useCallback, useEffect, useState } from "react";
import { FolderOpen, Plus, Star, Trash2, RefreshCw } from "lucide-react";
import { open } from "@tauri-apps/plugin-dialog";
import { WorkspacesResponseSchema, type WorkspacesResponse } from "@va/client";

import { EmptyBlock, PageHeader, PageShell, StatusBanner } from "@/components/page";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { apiFetch } from "./lib/api";

export function Workspaces() {
  const [data, setData] = useState<WorkspacesResponse | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [adding, setAdding] = useState(false);

  const fetchWorkspaces = useCallback(async () => {
    setLoading(true);
    setError("");
    try {
      const res = await apiFetch(`/api/workspaces`);
      if (!res.ok) throw new Error(await res.text());
      setData(WorkspacesResponseSchema.parse(await res.json()));
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    fetchWorkspaces();
  }, [fetchWorkspaces]);

  const addWorkspace = async () => {
    setAdding(true);
    try {
      const selected = await open({ directory: true, multiple: false, title: "Select Workspace Folder" });
      if (!selected) { setAdding(false); return; }
      const path = typeof selected === "string" ? selected : selected[0];
      if (!path) { setAdding(false); return; }
      const res = await apiFetch(`/api/workspaces`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ path }),
      });
      if (!res.ok) throw new Error(await res.text());
      fetchWorkspaces();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setAdding(false);
    }
  };

  const removeWorkspace = async (path: string) => {
    try {
      const res = await apiFetch(`/api/workspaces/remove`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ path }),
      });
      if (!res.ok) throw new Error(await res.text());
      fetchWorkspaces();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  const setDefault = async (path: string) => {
    try {
      const res = await apiFetch(`/api/workspaces/default`, {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ path }),
      });
      if (!res.ok) throw new Error(await res.text());
      fetchWorkspaces();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  return (
    <PageShell>
      <PageHeader
        icon={<FolderOpen className="w-4 h-4 text-primary" />}
        title="Workspaces"
        description="Workspace folders where agents build projects. The built-in workspace is used when no default folder is selected."
        actions={(
          <>
            <Button
              type="button"
              variant="ghost"
              size="icon-xs"
              onClick={fetchWorkspaces}
              title="Refresh"
            >
              <RefreshCw
                className={`w-3.5 h-3.5 text-muted-foreground ${loading ? "animate-spin" : ""}`}
              />
            </Button>
            <Button
              type="button"
              size="sm"
              onClick={addWorkspace}
              disabled={adding}
              className="text-xs font-semibold"
            >
              <Plus className="w-3.5 h-3.5" />
              {adding ? "Selecting…" : "Add Folder"}
            </Button>
          </>
        )}
      />

      {error && (
        <StatusBanner>{error}</StatusBanner>
      )}

      <div className="rounded-xl border border-border bg-card overflow-hidden">
        {data?.workspaces.map((ws, i) => (
          <div
            key={ws.path}
            className={`flex items-center justify-between px-3 py-2 ${
              i > 0 ? "border-t border-border" : ""
            }`}
          >
            <div className="flex items-center gap-3 min-w-0">
              <FolderOpen className="w-4 h-4 text-muted-foreground shrink-0" />
              <div className="min-w-0">
                <div className="text-xs font-mono truncate">{ws.path}</div>
                <div className="flex items-center gap-2 mt-0.5">
                  {ws.is_builtin && (
                    <Badge className="text-[10px]">
                      Built-in
                    </Badge>
                  )}
                  {ws.is_default && (
                    <Badge className="text-[10px] bg-amber-500/10 text-amber-600">
                      <Star className="w-2.5 h-2.5" /> Default
                    </Badge>
                  )}
                </div>
              </div>
            </div>

            <div className="flex items-center gap-1 shrink-0">
              {!ws.is_default && (
                <Button
                  type="button"
                  variant="ghost"
                  size="icon-sm"
                  onClick={() => setDefault(ws.path)}
                  title="Set as default"
                >
                  <Star className="w-3.5 h-3.5 text-muted-foreground" />
                </Button>
              )}
              {!ws.is_builtin && (
                <Button
                  type="button"
                  variant="ghost"
                  size="icon-sm"
                  onClick={() => removeWorkspace(ws.path)}
                  className="hover:bg-destructive/10"
                  title="Remove workspace"
                >
                  <Trash2 className="w-3.5 h-3.5 text-muted-foreground hover:text-destructive" />
                </Button>
              )}
            </div>
          </div>
        ))}

        {(!data || data.workspaces.length === 0) && !loading && (
          <EmptyBlock>
            No workspaces configured
          </EmptyBlock>
        )}
      </div>
    </PageShell>
  );
}
