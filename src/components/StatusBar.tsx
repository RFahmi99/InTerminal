import { shellLabel, useAppStore } from "../state/store";
import { Icon } from "./common/Icon";

export function StatusBar() {
  const shell = useAppStore((s) => s.shell);
  const indexStatus = useAppStore((s) => s.indexStatus);
  const aiStatus = useAppStore((s) => s.aiStatus);
  const openOverlay = useAppStore((s) => s.openOverlay);

  const [stateClass, stateText] =
    shell.exitCode !== undefined
      ? ["stopped", "Stopped"]
      : shell.sessionId === null || (shell.busy && !shell.lastPrompt)
        ? ["busy", "Starting"]
        : shell.busy
          ? ["busy", "Running a command"]
          : ["ready", "Ready"];

  const searchText = !indexStatus
    ? "Search: checking"
    : indexStatus.state === "building"
      ? "Search: learning installed commands"
      : indexStatus.state === "failed"
        ? "Search: unavailable"
        : `Search: ${indexStatus.documents.toLocaleString()} commands${indexStatus.semantic ? "" : " (keywords only)"}`;
  // Degraded or failed search explains itself on hover and to screen readers.
  const searchProblems = indexStatus?.messages.join(" ") ?? "";

  const aiText = !aiStatus
    ? "AI: checking"
    : !aiStatus.engineAvailable
      ? "AI: unavailable"
      : aiStatus.generating
        ? "AI: working"
        : aiStatus.modelReady
          ? "AI: ready, not loaded"
          : "AI: not set up";

  return (
    <footer className="status-bar">
      <div className="status-segment">
        <span className={`status-dot ${stateClass}`} aria-hidden="true" />
        {shellLabel(shell.hello)}: {stateText}
      </div>
      <div className="status-segment grow" title={shell.cwd ?? undefined}>
        {shell.cwd ?? ""}
      </div>
      {shell.integration === "unavailable" && (
        <div className="status-segment" title={shell.integrationMessage ?? undefined}>
          <Icon name="warning" />
          File tree isn't following the terminal
        </div>
      )}
      <div className="status-segment" title={searchProblems || undefined}>
        {searchProblems && <Icon name="warning" />}
        {searchText}
        {searchProblems && <span className="visually-hidden">. {searchProblems}</span>}
      </div>
      <div className="status-segment">
        {aiStatus && !aiStatus.modelReady && aiStatus.engineAvailable ? (
          <button type="button" className="status-button" onClick={() => openOverlay("ai")}>
            {aiText}
          </button>
        ) : (
          aiText
        )}
      </div>
    </footer>
  );
}
