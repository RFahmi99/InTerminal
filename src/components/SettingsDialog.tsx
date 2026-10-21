import { useEffect, useRef, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { open as openFileDialog } from "@tauri-apps/plugin-dialog";
import { announce } from "../a11y/announcer";
import { runCommand } from "../commands";
import { api, toAppError } from "../ipc/api";
import type { ScreenReaderSetting, ThemeSetting } from "../ipc/types";
import { useAppStore, type UpdateState } from "../state/store";
import { checkForUpdates, installUpdate } from "../updates";
import { Dialog } from "./common/Dialog";

const MIN_FONT = 11;
const MAX_FONT = 28;

/**
 * Windows PowerShell keeps help next to its own files, so installing it
 * needs an administrator window; PowerShell 7 can install it per user.
 */
function updateHelpCommand(edition: string | undefined): string {
  return edition === "Core"
    ? "Update-Help -Scope CurrentUser -ErrorAction Continue"
    : "Start-Process powershell -Verb RunAs -ArgumentList '-NoExit', '-Command', 'Update-Help -ErrorAction Continue'";
}

export function SettingsDialog() {
  const ui = useAppStore((s) => s.ui);
  const setUi = useAppStore((s) => s.setUi);
  const srDetected = useAppStore((s) => s.screenReaderDetected);
  const aiStatus = useAppStore((s) => s.aiStatus);
  const setAiStatus = useAppStore((s) => s.setAiStatus);
  const indexStatus = useAppStore((s) => s.indexStatus);
  const edition = useAppStore((s) => s.shell.hello?.edition);
  const close = () => useAppStore.getState().openOverlay(null);

  const installHelp = () => {
    close();
    void runCommand(updateHelpCommand(edition), null).then((ran) => {
      if (ran) {
        announce("Installing PowerShell help. When it finishes, open Settings and choose Check for newly installed programs.");
      }
    });
  };
  const [error, setError] = useState<string | null>(null);

  const refreshAi = () => api.aiStatus().then(setAiStatus);
  const fail = (e: unknown) => setError(toAppError(e).message);

  const chooseModel = () => {
    openFileDialog({ multiple: false, directory: false, filters: [{ name: "GGUF model", extensions: ["gguf"] }] })
      .then((path) => (typeof path === "string" ? api.aiChooseModel(path) : undefined))
      .then(refreshAi)
      .catch(fail);
  };

  return (
    <Dialog title="Settings" narrow onClose={close} footer={<button type="button" className="button primary" onClick={close}>Done</button>}>
      {error && (
        <p className="notice error" role="alert">
          {error}
        </p>
      )}
      <fieldset className="field">
        <legend>Theme</legend>
        <div className="choice-row">
          {(["system", "dark", "light"] as ThemeSetting[]).map((theme) => (
            <label key={theme}>
              <input type="radio" name="theme" checked={ui.theme === theme} onChange={() => setUi({ theme })} />
              {theme === "system" ? "Match Windows" : theme === "dark" ? "Dark" : "Light"}
            </label>
          ))}
        </div>
      </fieldset>

      <div className="field">
        <label htmlFor="font-size">Terminal text size</label>
        <div style={{ display: "flex", gap: "var(--space-2)", alignItems: "center" }}>
          <button type="button" className="button" aria-label="Smaller text" onClick={() => setUi({ fontSize: Math.max(MIN_FONT, ui.fontSize - 1) })}>
            A−
          </button>
          <input
            id="font-size"
            className="text-input"
            style={{ width: "6em" }}
            type="number"
            min={MIN_FONT}
            max={MAX_FONT}
            value={ui.fontSize}
            onChange={(event) => {
              const size = Number(event.target.value);
              if (size >= MIN_FONT && size <= MAX_FONT) setUi({ fontSize: size });
            }}
          />
          <button type="button" className="button" aria-label="Larger text" onClick={() => setUi({ fontSize: Math.min(MAX_FONT, ui.fontSize + 1) })}>
            A+
          </button>
        </div>
      </div>

      <fieldset className="field">
        <legend>Screen reader support in the terminal</legend>
        <div className="choice-row">
          {(["auto", "on", "off"] as ScreenReaderSetting[]).map((mode) => (
            <label key={mode}>
              <input
                type="radio"
                name="screen-reader"
                checked={ui.screenReaderMode === mode}
                onChange={() => setUi({ screenReaderMode: mode })}
              />
              {mode === "auto" ? "Automatic" : mode === "on" ? "On" : "Off"}
            </label>
          ))}
        </div>
        <span className="hint">
          Automatic turns it on when Windows reports a screen reader. Right now Windows reports{" "}
          {srDetected ? "a screen reader is running" : "no screen reader"}.
        </span>
      </fieldset>

      <label className="choice-row">
        <input type="checkbox" checked={ui.showHidden} onChange={(event) => setUi({ showHidden: event.target.checked })} />
        Show hidden files in the file tree
      </label>

      <div className="field">
        <span style={{ fontWeight: 600 }}>AI model</span>
        <span className="hint">
          {aiStatus?.usingDefaultModel
            ? `${aiStatus.defaultModel.name}${aiStatus.modelReady ? "" : " (not downloaded yet)"}`
            : `Your file: ${aiStatus?.modelPath ?? "unknown"}`}
        </span>
        <div style={{ display: "flex", gap: "var(--space-2)", flexWrap: "wrap" }}>
          <button type="button" className="button" onClick={chooseModel}>
            Choose a model file…
          </button>
          {aiStatus && !aiStatus.usingDefaultModel && (
            <button type="button" className="button" onClick={() => api.aiUseDefaultModel().then(refreshAi).catch(fail)}>
              Use the default model
            </button>
          )}
        </div>
      </div>

      <div className="field">
        <span style={{ fontWeight: 600 }}>Command search</span>
        <span className="hint">
          {indexStatus?.state === "ready"
            ? `Knows ${indexStatus.documents.toLocaleString()} commands available on this computer.`
            : indexStatus?.state === "failed"
              ? "Command search isn't working right now."
              : "Still learning what's installed."}
          {indexStatus?.messages.length ? ` ${indexStatus.messages.join(" ")}` : ""}
        </span>
        <div>
          <button
            type="button"
            className="button"
            onClick={() =>
              api
                .indexRefresh()
                .then(() => announce("Updating command search."))
                .catch(fail)
            }
          >
            Check for newly installed programs
          </button>
        </div>
      </div>

      <div className="field">
        <span style={{ fontWeight: 600 }}>PowerShell help</span>
        <span className="hint">
          {indexStatus
            ? `${indexStatus.helpDescriptions.toLocaleString()} of ${indexStatus.psCommands.toLocaleString()} PowerShell commands have a description from installed help. Search also has ${indexStatus.examples.toLocaleString()} worked examples for programs on this computer.`
            : "Checking which help is installed."}{" "}
          Windows doesn't include most PowerShell help until you install it. Installing downloads it from Microsoft
          {edition === "Core" ? "" : " and asks for administrator permission"}. When it finishes, choose “Check for newly
          installed programs” above.
        </span>
        <div>
          <button type="button" className="button" onClick={installHelp}>
            Install PowerShell help…
          </button>
        </div>
      </div>

      <UpdatesSection />
    </Dialog>
  );
}

function updateStatusText(update: UpdateState): string {
  switch (update.phase) {
    case "checking":
      return "Checking for updates…";
    case "upToDate":
      return "You have the latest version.";
    case "available":
      return `Version ${update.info?.version} is available.`;
    case "installing":
      return `Downloading version ${update.info?.version}${update.percent === null ? "…" : `: ${update.percent}%`}. VCE will close and restart to finish.`;
    case "failed":
      // The message already says whether checking or installing failed.
      return update.error ?? "";
    case "idle":
      return "";
  }
}

function UpdatesSection() {
  const ui = useAppStore((s) => s.ui);
  const setUi = useAppStore((s) => s.setUi);
  const update = useAppStore((s) => s.update);
  const [version, setVersion] = useState<string | null>(null);
  const status = useRef<HTMLSpanElement>(null);
  const busy = update.phase === "checking" || update.phase === "installing";

  const install = () => {
    void installUpdate();
    // The Install button goes away, so focus the status, which reads out the
    // download's progress; otherwise focus would fall out of the dialog.
    status.current?.focus();
  };

  useEffect(() => {
    getVersion()
      .then(setVersion)
      .catch((error) => console.warn(`Couldn't read VCE's version: ${toAppError(error).message}`));
  }, []);

  return (
    <div className="field">
      <span style={{ fontWeight: 600 }}>Updates</span>
      <span className="hint" ref={status} tabIndex={-1}>
        {version ? `You have VCE ${version}. ` : ""}
        {updateStatusText(update)}
      </span>
      {update.phase === "installing" && <progress max={100} value={update.percent ?? undefined} aria-label="Update download" />}
      {update.info?.notes && (
        <details>
          <summary>What's new in {update.info.version}</summary>
          <p style={{ whiteSpace: "pre-wrap" }}>{update.info.notes}</p>
        </details>
      )}
      <div style={{ display: "flex", gap: "var(--space-2)", flexWrap: "wrap" }}>
        <button type="button" className="button" aria-disabled={busy} onClick={() => {
            if (!busy) void checkForUpdates("user");
          }}
        >
          Check for updates now
        </button>
        {update.info && (update.phase === "available" || update.phase === "failed") && (
          <button type="button" className="button primary" onClick={install}>
            Install {update.info.version} and restart
          </button>
        )}
      </div>
      <label className="choice-row">
        <input type="checkbox" checked={ui.checkForUpdates} onChange={(event) => setUi({ checkForUpdates: event.target.checked })} />
        Check for updates when VCE starts
      </label>
      <span className="hint">Checking asks GitHub for the latest release. VCE sends nothing about you or your files.</span>
    </div>
  );
}
