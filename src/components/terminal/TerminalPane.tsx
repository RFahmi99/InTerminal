import { useEffect, useRef, useState } from "react";
import { Channel } from "@tauri-apps/api/core";
import { Terminal, type IMarker } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";

import { announce } from "../../a11y/announcer";
import { api, events, subscribeAll, toAppError } from "../../ipc/api";
import type { PromptEvent, SessionInfo } from "../../ipc/types";
import { registerRegion } from "../../focusRegistry";
import { notifyError, notifyFailure } from "../../notify";
import { matchShortcut } from "../../shortcuts";
import { applySession, screenReaderModeOn, useAppStore } from "../../state/store";
import { useResolvedTheme } from "../../theme";
import { readLines } from "./outputCapture";
import { DARK_TERMINAL_THEME, LIGHT_TERMINAL_THEME } from "./terminalTheme";

export const PATH_DRAG_TYPE = "application/x-interminal-path";

/** Lines read back when the start-of-command marker is gone (scrolled out). */
const FALLBACK_CAPTURE_LINES = 200;
/** Lets the last output chunk render before the buffer is read. */
const CAPTURE_DELAY_MS = 150;

function windowsBuild(osVersion: string | undefined): number | undefined {
  // "Microsoft Windows NT 10.0.26300.0" -> 26300
  const match = osVersion?.match(/\d+\.\d+\.(\d+)/);
  return match ? Number(match[1]) : undefined;
}

const modalOpen = () => document.querySelector('[aria-modal="true"]') !== null;

export function TerminalPane() {
  const hostRef = useRef<HTMLDivElement>(null);
  const termRef = useRef<Terminal | null>(null);
  const fitRef = useRef<FitAddon | null>(null);
  const restartRef = useRef<HTMLButtonElement>(null);
  const [dropActive, setDropActive] = useState(false);
  const ui = useAppStore((s) => s.ui);
  const srDetected = useAppStore((s) => s.screenReaderDetected);
  const exitCode = useAppStore((s) => s.shell.exitCode);
  const exitReason = useAppStore((s) => s.shell.exitReason);
  const resolvedTheme = useResolvedTheme();
  const srMode = screenReaderModeOn(ui, srDetected);

  /** Adopts a session and tells the user if it started without integration. */
  const adopt = (info: SessionInfo) => {
    applySession(info);
    const buildNumber = windowsBuild(info.hello?.osVersion);
    if (buildNumber && termRef.current) termRef.current.options.windowsPty = { backend: "conpty", buildNumber };
    if (info.integration === "unavailable" && info.integrationMessage) notifyError(info.integrationMessage);
  };

  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;
    const store = useAppStore.getState;
    const term = new Terminal({
      fontFamily: getComputedStyle(document.documentElement).getPropertyValue("--font-mono"),
      fontSize: store().ui.fontSize,
      lineHeight: 1.15,
      cursorBlink: true,
      scrollback: 5000,
      screenReaderMode: screenReaderModeOn(store().ui, store().screenReaderDetected),
      theme: DARK_TERMINAL_THEME,
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(host);
    fit.fit();
    termRef.current = term;
    fitRef.current = fit;

    let writeErrorShown = false;
    let commandStart: IMarker | null = null;
    let disposed = false;

    term.attachCustomKeyEventHandler((event) => {
      if (event.type !== "keydown") return true;
      if (matchShortcut(event)) return false; // Handled by the app's global key handler.
      const key = event.key.toLowerCase();
      if (event.ctrlKey && !event.altKey && key === "c" && (event.shiftKey || term.hasSelection())) {
        if (term.hasSelection()) {
          navigator.clipboard.writeText(term.getSelection()).catch((error) => notifyFailure(error, "Couldn't copy"));
          term.clearSelection();
        }
        event.preventDefault();
        return false;
      }
      // Let the browser's paste event through; xterm turns it into input.
      if (event.ctrlKey && !event.altKey && key === "v") return false;
      return true;
    });

    // Always forward, even while the UI shows the shell as stopped: a freshly
    // started pseudo-console asks the terminal for its cursor position and
    // blocks until xterm's reply reaches it, which can happen before the
    // restart call returns.
    const dataSubscription = term.onData((data) => {
      api.shellWrite(data).catch((error) => {
        // Typing into a stopped shell is expected while the overlay says so.
        const stoppedVisibly = toAppError(error).code === "shellNotRunning" && store().shell.exitCode !== undefined;
        if (stoppedVisibly || writeErrorShown) return;
        writeErrorShown = true;
        notifyFailure(error, "Typing into PowerShell failed");
      });
    });

    let resizeTimer: ReturnType<typeof setTimeout> | undefined;
    const resizeSubscription = term.onResize(({ cols, rows }) => {
      clearTimeout(resizeTimer);
      resizeTimer = setTimeout(() => {
        api.shellResize(cols, rows).catch((error) => {
          if (toAppError(error).code !== "shellNotRunning") notifyFailure(error, "Resizing the terminal failed");
        });
      }, 60);
    });

    let fitFrame = 0;
    const observer = new ResizeObserver(() => {
      cancelAnimationFrame(fitFrame);
      fitFrame = requestAnimationFrame(() => fit.fit());
    });
    observer.observe(host);

    const output = new Channel<ArrayBuffer>();
    output.onmessage = (chunk) => term.write(new Uint8Array(chunk));
    api
      .shellAttach(output, term.cols, term.rows)
      .then((info) => {
        if (!disposed) adopt(info);
      })
      .catch((error) => {
        store().updateShell({ exitCode: null, exitReason: toAppError(error).message });
        notifyFailure(error, "PowerShell couldn't start");
      });

    const captureFailure = (prompt: PromptEvent) => {
      const commandLine = prompt.commandLine;
      if (!commandLine) return;
      const marker = commandStart;
      setTimeout(() => {
        const buffer = term.buffer.active;
        const cursorLine = buffer.baseY + buffer.cursorY;
        const from = marker && !marker.isDisposed ? marker.line + 1 : cursorLine - FALLBACK_CAPTURE_LINES;
        const outputText = readLines(buffer, from, cursorLine);
        api
          .analyzeFailure({ commandLine, cwd: prompt.cwd, error: prompt.error, output: outputText })
          .then((items) => {
            if (items.length === 0) return;
            store().setSuggestions({ commandLine, items });
            const count = items.length === 1 ? "1 suggested fix" : `${items.length} suggested fixes`;
            announce(`The command failed. ${count}. Press Control period to review.`, "assertive");
          })
          .catch((error) => notifyFailure(error, "Couldn't check the error"));
      }, CAPTURE_DELAY_MS);
    };

    const isCurrent = (sessionId: number) => sessionId === store().shell.sessionId;
    const unsubscribe = subscribeAll(
      [
        events.onActivity((e) => {
          if (!isCurrent(e.sessionId)) return;
          if (e.busy && !store().shell.busy) {
            commandStart?.dispose();
            commandStart = term.registerMarker(0);
          }
          store().updateShell({ busy: e.busy, lineDirty: e.lineDirty });
        }),
        events.onPrompt((prompt) => {
          if (!isCurrent(prompt.sessionId) || !prompt.ran) return;
          if (prompt.success) {
            store().setSuggestions(null);
          } else {
            captureFailure(prompt);
          }
        }),
        events.onHello((hello) => {
          if (!isCurrent(hello.sessionId)) return;
          const buildNumber = windowsBuild(hello.osVersion);
          if (buildNumber) term.options.windowsPty = { backend: "conpty", buildNumber };
        }),
      ],
      (error) => notifyFailure(error, "The terminal couldn't subscribe to PowerShell's updates"),
    );

    const unregister = registerRegion("terminal", () => {
      term.focus();
      return true;
    });
    term.focus();

    return () => {
      disposed = true;
      unregister();
      unsubscribe();
      observer.disconnect();
      cancelAnimationFrame(fitFrame);
      clearTimeout(resizeTimer);
      dataSubscription.dispose();
      resizeSubscription.dispose();
      commandStart?.dispose();
      term.dispose();
      termRef.current = null;
    };
    // The terminal is created once; adopt only touches refs and the store.
  }, []);

  useEffect(() => {
    const term = termRef.current;
    if (!term) return;
    term.options.fontSize = ui.fontSize;
    term.options.screenReaderMode = srMode;
    term.options.theme = resolvedTheme === "light" ? LIGHT_TERMINAL_THEME : DARK_TERMINAL_THEME;
    fitRef.current?.fit();
  }, [ui.fontSize, srMode, resolvedTheme]);

  // Offer the restart button to the keyboard when the shell stops, unless a
  // dialog is open: that dialog keeps focus.
  useEffect(() => {
    if (exitCode !== undefined && !modalOpen()) restartRef.current?.focus();
  }, [exitCode]);

  const restart = () => {
    const term = termRef.current;
    if (!term) return;
    term.reset();
    api
      .shellRestart(term.cols, term.rows)
      .then((info) => {
        adopt(info);
        announce("PowerShell restarted.");
        term.focus();
      })
      .catch((error) => notifyFailure(error, "PowerShell couldn't restart"));
  };

  const onDrop = (event: React.DragEvent) => {
    setDropActive(false);
    const path = event.dataTransfer.getData(PATH_DRAG_TYPE);
    if (!path) return;
    event.preventDefault();
    api
      .fillTemplate("<path>", { path })
      .then((filled) => api.shellInsert(`${filled.command} `))
      .then(() => termRef.current?.focus())
      .catch((error) => notifyFailure(error, "Couldn't put the path on the command line"));
  };

  return (
    <div
      className={`terminal-host${dropActive ? " drop-active" : ""}`}
      data-region="terminal"
      onDragOver={(event) => {
        if (event.dataTransfer.types.includes(PATH_DRAG_TYPE)) {
          event.preventDefault();
          setDropActive(true);
        }
      }}
      onDragLeave={() => setDropActive(false)}
      onDrop={onDrop}
    >
      <div ref={hostRef} style={{ height: "100%" }} aria-label="Terminal" />
      {exitCode !== undefined && (
        <div className="shell-exited" role="status">
          <span>
            PowerShell has stopped{exitCode !== null ? ` (exit code ${exitCode})` : ""}.
            {exitReason && <span className="muted"> {exitReason}</span>}
          </span>
          <button ref={restartRef} type="button" className="button primary" onClick={restart}>
            Restart PowerShell
          </button>
        </div>
      )}
    </div>
  );
}
