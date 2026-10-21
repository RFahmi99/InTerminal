// Typed wrappers around the Rust commands and events. Components never call
// `invoke` directly, so argument names and result types live in one place.

import { Channel, invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  ActivityEvent,
  AiStage,
  AiStatus,
  AppError,
  DirListing,
  DownloadProgress,
  ExitEvent,
  FailureReport,
  FilledCommand,
  FsOpResult,
  GeneratedCommand,
  HelloEvent,
  IndexStatus,
  IntegrationEvent,
  PromptEvent,
  RiskReport,
  SearchHit,
  SessionInfo,
  Settings,
  Suggestion,
  UiSettings,
  UpdateInfo,
  UpdateProgress,
} from "./types";

/** Normalises anything a command can reject with into an AppError. */
export function toAppError(error: unknown): AppError {
  if (typeof error === "object" && error !== null && "message" in error) {
    const { code, message } = error as { code?: unknown; message: unknown };
    return { code: typeof code === "string" ? (code as AppError["code"]) : "unknown", message: String(message) };
  }
  return { code: "unknown", message: String(error) };
}

export const api = {
  shellAttach: (output: Channel<ArrayBuffer>, cols: number, rows: number) =>
    invoke<SessionInfo>("shell_attach", { output, cols, rows }),
  shellRestart: (cols: number, rows: number) => invoke<SessionInfo>("shell_restart", { cols, rows }),
  shellWrite: (data: string) => invoke<void>("shell_write", { data }),
  shellResize: (cols: number, rows: number) => invoke<void>("shell_resize", { cols, rows }),
  shellChangeDir: (path: string) => invoke<string>("shell_change_dir", { path }),
  shellRun: (command: string) => invoke<void>("shell_run", { command }),
  shellInsert: (text: string) => invoke<void>("shell_insert", { text }),

  fsList: (path: string, showHidden: boolean) => invoke<DirListing>("fs_list", { path, showHidden }),
  fsCreateDir: (parent: string, name: string) => invoke<FsOpResult>("fs_create_dir", { parent, name }),
  fsRename: (path: string, newName: string) => invoke<FsOpResult>("fs_rename", { path, newName }),
  fsMove: (paths: string[], destDir: string) => invoke<FsOpResult>("fs_move", { paths, destDir }),
  fsTrash: (paths: string[]) => invoke<FsOpResult>("fs_trash", { paths }),
  fsOpen: (path: string) => invoke<void>("fs_open", { path }),
  fsReveal: (path: string) => invoke<void>("fs_reveal", { path }),

  indexStatus: () => invoke<IndexStatus>("index_status"),
  indexRefresh: () => invoke<void>("index_refresh"),
  searchCommands: (query: string, limit?: number) => invoke<SearchHit[]>("search_commands", { query, limit }),

  analyzeFailure: (report: FailureReport) => invoke<Suggestion[]>("analyze_failure", { report }),
  assessRisk: (command: string) => invoke<RiskReport>("assess_risk", { command }),
  fillTemplate: (template: string, values: Record<string, string>) =>
    invoke<FilledCommand>("fill_template", { template, values }),

  aiStatus: () => invoke<AiStatus>("ai_status"),
  aiDownloadModel: () => invoke<string>("ai_download_model"),
  aiCancelDownload: () => invoke<void>("ai_cancel_download"),
  aiChooseModel: (path: string) => invoke<void>("ai_choose_model", { path }),
  aiUseDefaultModel: () => invoke<void>("ai_use_default_model"),
  aiGenerate: (request: string) => invoke<GeneratedCommand>("ai_generate", { request }),
  aiCancel: () => invoke<void>("ai_cancel"),

  settingsGet: () => invoke<Settings & { loadWarning: string | null }>("settings_get"),
  settingsSetUi: (ui: UiSettings) => invoke<void>("settings_set_ui", { ui }),

  screenReaderActive: () => invoke<boolean>("system_screen_reader_active"),

  updateCheck: () => invoke<UpdateInfo | null>("update_check"),
  updateInstall: () => invoke<void>("update_install"),
};

/**
 * Starts several event subscriptions and returns one cleanup. A subscription
 * that fails to start goes to `onError` rather than becoming an unhandled
 * rejection; one that resolves after cleanup is released immediately.
 */
export function subscribeAll(subscriptions: Promise<UnlistenFn>[], onError: (error: unknown) => void): () => void {
  let disposed = false;
  const unlisteners: UnlistenFn[] = [];
  for (const subscription of subscriptions) {
    subscription.then((unlisten) => (disposed ? unlisten() : unlisteners.push(unlisten)), onError);
  }
  return () => {
    disposed = true;
    unlisteners.splice(0).forEach((unlisten) => unlisten());
  };
}

export const events = {
  onHello: (handler: (e: HelloEvent) => void): Promise<UnlistenFn> => listen<HelloEvent>("shell:hello", (e) => handler(e.payload)),
  onPrompt: (handler: (e: PromptEvent) => void) => listen<PromptEvent>("shell:prompt", (e) => handler(e.payload)),
  onActivity: (handler: (e: ActivityEvent) => void) => listen<ActivityEvent>("shell:activity", (e) => handler(e.payload)),
  onIntegration: (handler: (e: IntegrationEvent) => void) =>
    listen<IntegrationEvent>("shell:integration", (e) => handler(e.payload)),
  onExit: (handler: (e: ExitEvent) => void) => listen<ExitEvent>("shell:exit", (e) => handler(e.payload)),
  onIndexStatus: (handler: (e: IndexStatus) => void) => listen<IndexStatus>("index:status", (e) => handler(e.payload)),
  onAiProgress: (handler: (e: AiStage) => void) => listen<AiStage>("ai:progress", (e) => handler(e.payload)),
  onAiDownload: (handler: (e: DownloadProgress) => void) =>
    listen<DownloadProgress>("ai:download", (e) => handler(e.payload)),
  onUpdateProgress: (handler: (e: UpdateProgress) => void) =>
    listen<UpdateProgress>("update:progress", (e) => handler(e.payload)),
};
