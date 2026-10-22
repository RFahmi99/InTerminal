import { create } from "zustand";
import type {
  AiStatus,
  SessionInfo,
  HelloInfo,
  IndexStatus,
  IntegrationStatus,
  PromptEvent,
  RiskReport,
  Suggestion,
  UiSettings,
  UpdateInfo,
} from "../ipc/types";

export const DEFAULT_UI_SETTINGS: UiSettings = {
  theme: "system",
  fontSize: 15,
  screenReaderMode: "auto",
  showHidden: false,
  explorerWidth: 280,
  checkForUpdates: true,
};

export interface UpdateState {
  phase: "idle" | "checking" | "upToDate" | "available" | "installing" | "failed";
  info: UpdateInfo | null;
  /** Download progress while installing, 0–100; null until the size is known. */
  percent: number | null;
  error: string | null;
}

export type Overlay = "search" | "ai" | "settings" | "help" | null;

export interface FixSuggestions {
  commandLine: string;
  items: Suggestion[];
}

export interface PendingRun {
  command: string;
  risk: RiskReport;
}

export interface Toast {
  id: number;
  message: string;
  /** A command to show on a tape, e.g. the CLI equivalent of a GUI action. */
  command?: string;
}

interface ShellState {
  sessionId: number | null;
  cwd: string | null;
  hello: HelloInfo | null;
  integration: IntegrationStatus;
  integrationMessage: string | null;
  busy: boolean;
  lineDirty: boolean;
  exitCode: number | null | undefined; // undefined while running
  /** Why InTerminal stopped the shell, when it wasn't a normal exit. */
  exitReason: string | null;
  lastPrompt: PromptEvent | null;
}

interface AppState {
  shell: ShellState;
  ui: UiSettings;
  screenReaderDetected: boolean;
  overlay: Overlay;
  suggestions: FixSuggestions | null;
  indexStatus: IndexStatus | null;
  aiStatus: AiStatus | null;
  toasts: Toast[];
  /** A dangerous command waiting for the user to confirm running it. */
  pendingRun: PendingRun | null;
  update: UpdateState;

  updateShell: (patch: Partial<ShellState>) => void;
  setUpdate: (patch: Partial<UpdateState>) => void;
  setPendingRun: (pending: PendingRun | null) => void;
  setUi: (patch: Partial<UiSettings>) => void;
  setScreenReaderDetected: (detected: boolean) => void;
  openOverlay: (overlay: Overlay) => void;
  setSuggestions: (suggestions: FixSuggestions | null) => void;
  setIndexStatus: (status: IndexStatus) => void;
  setAiStatus: (status: AiStatus) => void;
  pushToast: (toast: Omit<Toast, "id">) => void;
  dismissToast: (id: number) => void;
}

let nextToastId = 1;

export const useAppStore = create<AppState>((set) => ({
  shell: {
    sessionId: null,
    cwd: null,
    hello: null,
    integration: "pending",
    integrationMessage: null,
    busy: true,
    lineDirty: false,
    exitCode: undefined,
    exitReason: null,
    lastPrompt: null,
  },
  ui: DEFAULT_UI_SETTINGS,
  screenReaderDetected: false,
  overlay: null,
  suggestions: null,
  indexStatus: null,
  aiStatus: null,
  toasts: [],
  pendingRun: null,
  update: { phase: "idle", info: null, percent: null, error: null },

  updateShell: (patch) => set((state) => ({ shell: { ...state.shell, ...patch } })),
  setUpdate: (patch) => set((state) => ({ update: { ...state.update, ...patch } })),
  setPendingRun: (pendingRun) => set({ pendingRun }),
  setUi: (patch) => set((state) => ({ ui: { ...state.ui, ...patch } })),
  setScreenReaderDetected: (screenReaderDetected) => set({ screenReaderDetected }),
  openOverlay: (overlay) => set({ overlay }),
  setSuggestions: (suggestions) => set({ suggestions }),
  setIndexStatus: (indexStatus) => set({ indexStatus }),
  setAiStatus: (aiStatus) => set({ aiStatus }),
  pushToast: (toast) =>
    set((state) => ({ toasts: [...state.toasts.slice(-2), { ...toast, id: nextToastId++ }] })),
  dismissToast: (id) => set((state) => ({ toasts: state.toasts.filter((t) => t.id !== id) })),
}));

/** Adopts a (re)started shell session as the current one. */
export function applySession(info: SessionInfo): void {
  useAppStore.getState().updateShell({
    sessionId: info.sessionId,
    cwd: info.cwd,
    hello: info.hello,
    integration: info.integration,
    integrationMessage: info.integrationMessage,
    busy: info.busy,
    lineDirty: info.lineDirty,
    exitCode: undefined,
    exitReason: null,
  });
}

/** Whether the terminal should run in screen-reader mode. */
export function screenReaderModeOn(ui: UiSettings, detected: boolean): boolean {
  return ui.screenReaderMode === "on" || (ui.screenReaderMode === "auto" && detected);
}

/** "Windows PowerShell 5.1" / "PowerShell 7.4". */
export function shellLabel(hello: HelloInfo | null): string {
  if (!hello) return "PowerShell";
  const [major, minor] = hello.psVersion.split(".");
  const product = hello.edition === "Core" ? "PowerShell" : "Windows PowerShell";
  return `${product} ${major}.${minor}`;
}
