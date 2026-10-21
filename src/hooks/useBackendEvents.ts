import { useEffect } from "react";
import { announce } from "../a11y/announcer";
import { api, events, subscribeAll } from "../ipc/api";
import { notifyError, notifyFailure } from "../notify";
import { useAppStore } from "../state/store";
import { recordProgress } from "../updates";

function folderName(path: string): string {
  const trimmed = path.replace(/[\\/]+$/, "");
  return trimmed.slice(trimmed.lastIndexOf("\\") + 1) || trimmed;
}

/**
 * Keeps the store in step with the backend: shell state, command index, AI
 * status and update downloads. Announces background changes so screen-reader users hear what
 * sighted users see change.
 */
export function useBackendEvents() {
  useEffect(() => {
    const store = useAppStore.getState;
    const isCurrent = (sessionId: number) => sessionId === store().shell.sessionId;

    api.indexStatus().then(store().setIndexStatus).catch((error) => notifyFailure(error, "Command search status unavailable"));
    api.aiStatus().then(store().setAiStatus).catch((error) => notifyFailure(error, "AI status unavailable"));

    return subscribeAll(
      [
        events.onHello((hello) => {
          if (!isCurrent(hello.sessionId)) return;
          store().updateShell({ hello });
        }),
        events.onPrompt((prompt) => {
          if (!isCurrent(prompt.sessionId)) return;
          const previous = store().shell.cwd;
          const cwd = prompt.provider === "FileSystem" ? prompt.cwd : previous;
          store().updateShell({ lastPrompt: prompt, cwd });
          if (cwd && previous && cwd.toLowerCase() !== previous.toLowerCase()) {
            announce(`Folder changed to ${folderName(cwd)}. Full path ${cwd}.`, "polite", "cwd");
          }
        }),
        events.onIntegration((e) => {
          if (!isCurrent(e.sessionId)) return;
          store().updateShell({ integration: e.status, integrationMessage: e.message });
          if (e.message) notifyError(e.message);
        }),
        events.onExit((e) => {
          if (!isCurrent(e.sessionId)) return;
          store().updateShell({ exitCode: e.exitCode, exitReason: e.reason, busy: false });
          const reason = e.reason ? ` ${e.reason}` : "";
          announce(`PowerShell has stopped.${reason} Press the Restart PowerShell button to start it again.`, "assertive");
        }),
        events.onIndexStatus((status) => {
          const previous = store().indexStatus;
          store().setIndexStatus(status);
          if (status.state === "failed" && previous?.state !== "failed") {
            notifyError(status.messages[status.messages.length - 1] ?? "Command search couldn't start.");
          } else if (status.state === "ready" && previous?.state !== "ready") {
            const limits = status.messages.length ? ` ${status.messages.join(" ")}` : "";
            announce(`Command search is ready.${limits}`, "polite", "index");
          }
        }),
        events.onUpdateProgress(recordProgress),
      ],
      (error) => notifyFailure(error, "VCE couldn't subscribe to background updates"),
    );
  }, []);
}
