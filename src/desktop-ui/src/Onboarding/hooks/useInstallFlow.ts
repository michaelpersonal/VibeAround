import { useCallback, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import type { InstallTaskInfo, InstallTaskProgress, Settings } from "../types";

interface UseInstallFlowResult {
  finishing: boolean;
  isInstalling: boolean;
  installComplete: boolean;
  installTasks: InstallTaskProgress[];
  startInstall: (finalSettings: Settings) => Promise<void>;
  cancelInstall: () => Promise<void>;
  completeInstall: () => Promise<void>;
}

/**
 * Orchestrates the onboarding install: pre-populates the task list from
 * `get_install_manifest`, subscribes to `onboarding-install-progress` and
 * `onboarding-install-complete` Tauri events, and owns the `finishing /
 * isInstalling / installComplete / installTasks` state machine.
 *
 * Event listeners are registered in `startInstall` and released in
 * `completeInstall` (or when the consumer unmounts — the caller owns that
 * lifecycle via React).
 */
export function useInstallFlow(): UseInstallFlowResult {
  const [finishing, setFinishing] = useState(false);
  const [isInstalling, setIsInstalling] = useState(false);
  const [installComplete, setInstallComplete] = useState(false);
  const [installTasks, setInstallTasks] = useState<InstallTaskProgress[]>([]);
  const unlistenRefs = useRef<UnlistenFn[]>([]);

  const startInstall = useCallback(async (finalSettings: Settings) => {
    setFinishing(true);
    try {
      const manifest = await invoke<InstallTaskInfo[]>("get_install_manifest", {
        settings: finalSettings,
      });
      setInstallTasks(
        manifest.map((t) => ({
          id: t.id,
          label: t.label,
          status: "pending" as const,
          logs: [],
        })),
      );
      setIsInstalling(true);

      const unlistenProgress = await listen<{
        id: string;
        label: string;
        status: string;
        message?: string;
      }>("onboarding-install-progress", (event) => {
        const { id, status, message } = event.payload;
        setInstallTasks((prev) =>
          prev.map((task) =>
            task.id === id
              ? {
                  ...task,
                  status: status as InstallTaskProgress["status"],
                  message,
                  logs: message ? [...(task.logs ?? []), message] : task.logs,
                }
              : task,
          ),
        );
      });

      const unlistenComplete = await listen<{ status: string }>(
        "onboarding-install-complete",
        () => {
          setInstallComplete(true);
        },
      );

      unlistenRefs.current = [unlistenProgress, unlistenComplete];

      await invoke("start_onboarding_install", { settings: finalSettings });
    } catch (error) {
      console.error("start_onboarding_install failed:", error);
      setFinishing(false);
      setIsInstalling(false);
    }
  }, []);

  const cancelInstall = useCallback(async () => {
    try {
      await invoke("cancel_onboarding_install");
      setInstallTasks((prev) =>
        prev.map((task) =>
          task.status === "pending" || task.status === "running"
            ? {
                ...task,
                status: "cancelled" as const,
                message: "Cancelled",
                logs: [...(task.logs ?? []), "Cancelled"],
              }
            : task,
        ),
      );
      setInstallComplete(true);
    } catch (error) {
      console.error("cancel failed:", error);
    }
  }, []);

  const completeInstall = useCallback(async () => {
    for (const unlisten of unlistenRefs.current) {
      unlisten();
    }
    unlistenRefs.current = [];

    try {
      await invoke("finish_onboarding");
      window.location.replace("/");
    } catch (error) {
      console.error("finish_onboarding failed:", error);
    }
  }, []);

  return {
    finishing,
    isInstalling,
    installComplete,
    installTasks,
    startInstall,
    cancelInstall,
    completeInstall,
  };
}
