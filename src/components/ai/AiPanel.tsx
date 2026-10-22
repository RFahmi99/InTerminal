import { useCallback, useEffect, useRef, useState } from "react";
import { open as openFileDialog } from "@tauri-apps/plugin-dialog";
import { announce } from "../../a11y/announcer";
import { copyCommand, insertCommand, runCommand } from "../../commands";
import { api, events, subscribeAll, toAppError } from "../../ipc/api";
import type { DownloadProgress, GeneratedCommand, RiskReport } from "../../ipc/types";
import { useAppStore } from "../../state/store";
import { Dialog } from "../common/Dialog";
import { Icon } from "../common/Icon";
import { RiskBadge } from "../common/RiskBadge";

type Phase = "idle" | "loading" | "generating" | "done";

const STAGES: { phase: Phase; label: string }[] = [
  { phase: "loading", label: "Loading the model into memory" },
  { phase: "generating", label: "Writing the command" },
  { phase: "done", label: "Unloading the model" },
];

function megabytes(bytes: number): string {
  return `${Math.round(bytes / 1_000_000)} MB`;
}

/** Lazy-loaded AI command generation with mandatory human review. */
export function AiPanel() {
  const status = useAppStore((s) => s.aiStatus);
  const setAiStatus = useAppStore((s) => s.setAiStatus);
  const close = () => useAppStore.getState().openOverlay(null);
  const [request, setRequest] = useState("");
  const [phase, setPhase] = useState<Phase>("idle");
  const [result, setResult] = useState<GeneratedCommand | null>(null);
  const [command, setCommand] = useState("");
  const [risk, setRisk] = useState<RiskReport | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [download, setDownload] = useState<DownloadProgress | null>(null);
  const requestRef = useRef<HTMLTextAreaElement>(null);
  const commandRef = useRef<HTMLTextAreaElement>(null);
  const cancelRef = useRef<HTMLButtonElement>(null);
  const downloadAttempt = useRef(0);

  const refreshStatus = useCallback(() => {
    api
      .aiStatus()
      .then(setAiStatus)
      .catch((e) => setError(toAppError(e).message));
  }, [setAiStatus]);

  useEffect(() => {
    refreshStatus();
    return subscribeAll(
      [
        events.onAiProgress((stage) => {
          if (stage === "loading") {
            setPhase("loading");
            announce("Loading the AI model.");
          } else if (stage === "generating") {
            setPhase("generating");
            announce("Writing the command.");
          }
        }),
        events.onAiDownload(setDownload),
      ],
      (e) => setError(`Progress updates aren't available: ${toAppError(e).message}`),
    );
  }, [refreshStatus]);

  // While the model works, the Cancel button replaces the controls that had
  // focus; move focus with it, and back to the request when it's done.
  const busy = phase === "loading" || phase === "generating";
  const wasBusy = useRef(false);
  useEffect(() => {
    if (busy && !wasBusy.current) cancelRef.current?.focus();
    // Land on the command to review it, or back on the request if there's none.
    if (!busy && wasBusy.current) (commandRef.current ?? requestRef.current)?.focus();
    wasBusy.current = busy;
  }, [busy]);

  // Re-check the risk whenever the user edits the generated command.
  useEffect(() => {
    if (!result) return;
    const timer = setTimeout(() => {
      api
        .assessRisk(command)
        .then(setRisk)
        .catch((e) => setError(toAppError(e).message));
    }, 150);
    return () => clearTimeout(timer);
  }, [command, result]);

  const generate = () => {
    if (!request.trim() || phase === "loading" || phase === "generating") return;
    setError(null);
    setResult(null);
    setPhase("loading");
    api
      .aiGenerate(request)
      .then((generated) => {
        setResult(generated);
        setCommand(generated.command);
        setRisk(generated.risk);
        setPhase("done");
        if (generated.command) {
          announce(`Command ready for review: ${generated.command}. ${generated.explanation}`, "assertive");
        } else {
          announce(`The AI didn't write a command. ${generated.explanation}`, "assertive");
        }
      })
      .catch((e) => {
        const appError = toAppError(e);
        setPhase("idle");
        if (appError.code === "cancelled") {
          announce("Cancelled. The model has been unloaded.");
        } else {
          setError(appError.message);
          announce(appError.message, "assertive");
        }
      })
      .finally(refreshStatus);
  };

  const startDownload = () => {
    const attempt = ++downloadAttempt.current;
    setError(null);
    setDownload({ downloaded: 0, total: status?.defaultModel.bytes ?? 0 });
    refreshStatus();
    api
      .aiDownloadModel()
      .then(() => {
        announce("The AI model is downloaded and ready.");
        requestAnimationFrame(() => requestRef.current?.focus());
      })
      .catch((e) => {
        const appError = toAppError(e);
        // A cancelled attempt was already reported when it was cancelled.
        if (appError.code === "cancelled" || attempt !== downloadAttempt.current) return;
        setError(appError.message);
        announce(appError.message, "assertive");
      })
      .finally(() => {
        if (attempt !== downloadAttempt.current) return;
        setDownload(null);
        refreshStatus();
      });
  };

  const chooseModelFile = () => {
    openFileDialog({ multiple: false, directory: false, filters: [{ name: "GGUF model", extensions: ["gguf"] }] })
      .then((path) => (typeof path === "string" ? api.aiChooseModel(path).then(() => announce("Model selected.")) : undefined))
      .then(refreshStatus)
      .catch((e) => setError(toAppError(e).message));
  };

  const cancelGeneration = () => {
    api.aiCancel().catch((e) => setError(toAppError(e).message));
  };
  // The backend abandons the transfer at once (even one stuck on a dead
  // connection), so the panel can reset without waiting for it.
  const cancelDownload = () => {
    downloadAttempt.current++;
    api
      .aiCancelDownload()
      .then(() => {
        setDownload(null);
        announce("Download cancelled.");
        refreshStatus();
      })
      .catch((e) => setError(toAppError(e).message));
  };

  const switchToDefaultModel = () => {
    api
      .aiUseDefaultModel()
      .then(refreshStatus)
      .catch((e) => setError(toAppError(e).message));
  };

  const downloading = Boolean(download) || status?.downloading;

  const body = (() => {
    if (!status) return <p className="muted">Checking the AI setup…</p>;
    if (!status.engineAvailable) {
      return (
        <p className="notice">
          The AI engine isn't included in this copy of InTerminal, so command generation isn't available. Everything else
          works as normal.
        </p>
      );
    }
    if (!status.modelReady && !status.usingDefaultModel) {
      return (
        <>
          <p className="notice error">Your model file can't be found: {status.modelPath}. It may have been moved or deleted.</p>
          <div style={{ display: "flex", gap: "var(--space-2)", flexWrap: "wrap" }}>
            <button type="button" className="button primary" onClick={chooseModelFile}>
              Choose another model file…
            </button>
            <button type="button" className="button" onClick={switchToDefaultModel}>
              Use the default model
            </button>
          </div>
        </>
      );
    }
    if (!status.modelReady) {
      return (
        <>
          <p>
            To write commands, InTerminal needs a small language model: {status.defaultModel.name},{" "}
            {megabytes(status.defaultModel.bytes)}, {status.defaultModel.license} licence.
          </p>
          <p className="muted">
            It's downloaded once from Hugging Face. After that it runs entirely on this computer, and it's only in
            memory while it writes a command.
          </p>
          {downloading && download && (
            <div>
              <div
                className="progress"
                role="progressbar"
                aria-label="Model download"
                aria-valuemin={0}
                aria-valuemax={download.total}
                aria-valuenow={download.downloaded}
                aria-valuetext={`${megabytes(download.downloaded)} of ${megabytes(download.total)}`}
              >
                <div style={{ width: `${download.total ? (download.downloaded / download.total) * 100 : 0}%` }} />
              </div>
              <p className="muted">
                {megabytes(download.downloaded)} of {megabytes(download.total)}
              </p>
            </div>
          )}
          <div style={{ display: "flex", gap: "var(--space-2)", flexWrap: "wrap" }}>
            {downloading ? (
              <button type="button" className="button" onClick={cancelDownload}>
                Cancel download
              </button>
            ) : (
              <button type="button" className="button primary" onClick={startDownload}>
                Download the model ({megabytes(status.defaultModel.bytes)})
              </button>
            )}
            <button type="button" className="button" onClick={chooseModelFile} disabled={downloading}>
              Use a model file I already have…
            </button>
          </div>
        </>
      );
    }
    return (
      <>
        <div className="field">
          <label htmlFor="ai-request">What do you want to do?</label>
          <textarea
            id="ai-request"
            ref={requestRef}
            className="text-input"
            rows={3}
            value={request}
            disabled={busy}
            placeholder="For example: find every PDF in this folder and its subfolders that was changed this week"
            onChange={(event) => setRequest(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter" && event.ctrlKey) {
                event.preventDefault();
                generate();
              }
            }}
          />
          <span className="hint">
            Runs on this computer. The model loads only while it writes, then it's removed from memory.
          </span>
        </div>

        {busy && (
          <ol className="stage-list" aria-label="Progress">
            {STAGES.map((stage) => (
              <li key={stage.phase} aria-current={stage.phase === phase ? "step" : undefined}>
                <Icon name={stage.phase === phase ? "sparkle" : "chevronRight"} />
                {stage.label}
              </li>
            ))}
          </ol>
        )}

        {result && !busy && (
          <section aria-labelledby="ai-result-heading" style={{ display: "grid", gap: "var(--space-3)" }}>
            <h3 id="ai-result-heading" style={{ margin: 0, fontSize: "var(--text-m)" }}>
              Review before running
            </h3>
            {result.command ? (
              <>
                <div className="field">
                  <label htmlFor="ai-command">Command (you can edit it)</label>
                  <textarea
                    id="ai-command"
                    ref={commandRef}
                    className="tape"
                    rows={Math.min(6, Math.max(2, Math.ceil(command.length / 70)))}
                    value={command}
                    spellCheck={false}
                    onChange={(event) => {
                      setCommand(event.target.value);
                      // The old assessment no longer applies; a fresh one follows.
                      setRisk(null);
                    }}
                  />
                </div>
                {result.explanation && <p>{result.explanation}</p>}
                {risk && <RiskBadge risk={risk} showReasons />}
                {result.warnings.length > 0 && (
                  <div className="notice error">
                    <strong>Check this carefully:</strong>
                    <ul>
                      {result.warnings.map((warning) => (
                        <li key={warning}>{warning}</li>
                      ))}
                    </ul>
                  </div>
                )}
              </>
            ) : (
              <p className="notice">{result.explanation || "The model couldn't turn that into a command. Try describing it differently."}</p>
            )}
          </section>
        )}
      </>
    );
  })();

  const reviewing = Boolean(result?.command) && !busy;
  return (
    <Dialog
      title="Write a command with AI"
      onClose={() => {
        if (busy) cancelGeneration();
        close();
      }}
      initialFocus={requestRef}
      footer={
        status?.modelReady && status.engineAvailable ? (
          busy ? (
            <button ref={cancelRef} type="button" className="button" onClick={cancelGeneration}>
              Cancel
            </button>
          ) : (
            <>
              {reviewing && (
                <>
                  <button type="button" className="button quiet" onClick={() => void copyCommand(command)}>
                    <Icon name="copy" /> Copy
                  </button>
                  <button
                    type="button"
                    className="button"
                    disabled={!command.trim()}
                    onClick={() => void insertCommand(command).then((ok) => ok && close())}
                  >
                    Put on command line
                  </button>
                  <button
                    type="button"
                    className={`button ${risk?.level === "danger" ? "danger" : "primary"}`}
                    disabled={!command.trim()}
                    onClick={() => {
                      close();
                      void runCommand(command, risk);
                    }}
                  >
                    Run
                  </button>
                </>
              )}
              <button
                type="button"
                className={reviewing ? "button" : "button primary"}
                disabled={!request.trim()}
                onClick={generate}
                aria-keyshortcuts="Control+Enter"
                title="Ctrl+Enter"
              >
                <Icon name="sparkle" /> {result ? "Write again" : "Write command"}
              </button>
            </>
          )
        ) : undefined
      }
    >
      {error && (
        <p className="notice error" role="alert">
          {error}
        </p>
      )}
      {body}
    </Dialog>
  );
}
