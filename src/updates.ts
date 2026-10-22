// Checking for and installing new InTerminal versions. A check at startup stays
// quiet unless it finds something; one the user asks for always answers.

import { announce } from "./a11y/announcer";
import { api, toAppError } from "./ipc/api";
import type { UpdateProgress } from "./ipc/types";
import { notifyDone, notifyFailure } from "./notify";
import { useAppStore } from "./state/store";

/** How long after launch the automatic check waits, so it doesn't compete with startup. */
export const STARTUP_CHECK_DELAY_MS = 15_000;

export async function checkForUpdates(trigger: "startup" | "user"): Promise<void> {
  const { update, ui, setUpdate } = useAppStore.getState();
  // By the time the startup check runs, the user may have checked or started
  // installing already, or turned automatic checks off.
  if (trigger === "startup" && (update.phase !== "idle" || !ui.checkForUpdates)) return;
  setUpdate({ phase: "checking", error: null });
  try {
    const info = await api.updateCheck();
    if (info) {
      setUpdate({ phase: "available", info });
      notifyDone(`InTerminal ${info.version} is available. Open Settings to install it.`);
    } else {
      setUpdate({ phase: "upToDate", info: null });
      if (trigger === "user") announce("You have the latest version of InTerminal.");
    }
  } catch (error) {
    const { message } = toAppError(error);
    setUpdate({ phase: "failed", error: message });
    // Being offline at startup is normal, so only a requested check reports it.
    if (trigger === "user") {
      notifyFailure(error);
    } else {
      console.warn(`Automatic update check failed: ${message}`);
    }
  }
}

/** Whole-percent progress, or null while the download size is unknown. */
export function downloadPercent(progress: UpdateProgress): number | null {
  if (!progress.total) return null;
  return Math.min(100, Math.floor((progress.downloaded / progress.total) * 100));
}

export function recordProgress(progress: UpdateProgress): void {
  const { update, setUpdate } = useAppStore.getState();
  const percent = downloadPercent(progress);
  if (percent !== null && percent !== update.percent && percent % 25 === 0 && percent < 100) {
    announce(`Update ${percent}% downloaded.`, "polite", "update");
  }
  setUpdate({ percent });
}

/** Downloads and installs the update; on success InTerminal closes and the new version starts. */
export async function installUpdate(): Promise<void> {
  const { update, setUpdate } = useAppStore.getState();
  if (!update.info) return;
  // Settings moves focus to its status text, which says what's happening.
  setUpdate({ phase: "installing", percent: null, error: null });
  try {
    await api.updateInstall();
  } catch (error) {
    setUpdate({ phase: "failed", error: toAppError(error).message });
    notifyFailure(error);
  }
}
