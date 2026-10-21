// Mirrors of the Rust types serialised over IPC (serde camelCase).

export type ErrorCode =
  | "shellNotRunning"
  | "shellBusy"
  | "lineDirty"
  | "invalidInput"
  | "notFound"
  | "io"
  | "indexUnavailable"
  | "aiUnavailable"
  | "aiFailed"
  | "cancelled"
  | "updateFailed";

export interface AppError {
  code: ErrorCode | "unknown";
  message: string;
}

export type IntegrationStatus = "pending" | "connected" | "unavailable";

export interface HelloInfo {
  psVersion: string;
  edition: string;
  osVersion: string;
  psReadLine: boolean;
}

export interface SessionInfo {
  sessionId: number;
  shellPath: string;
  cwd: string | null;
  hello: HelloInfo | null;
  integration: IntegrationStatus;
  integrationMessage: string | null;
  busy: boolean;
  lineDirty: boolean;
}

export interface PsError {
  message: string;
  errorId: string;
  category: string;
  target: string;
  exceptionType: string;
}

export interface PromptEvent {
  sessionId: number;
  cwd: string | null;
  provider: string | null;
  ran: boolean;
  commandLine: string | null;
  success: boolean;
  exitCode: number | null;
  error: PsError | null;
  editMode: string | null;
}

export interface HelloEvent extends HelloInfo {
  sessionId: number;
}

export interface ActivityEvent {
  sessionId: number;
  busy: boolean;
  lineDirty: boolean;
}

export interface IntegrationEvent {
  sessionId: number;
  status: IntegrationStatus;
  message: string | null;
}

export interface ExitEvent {
  sessionId: number;
  exitCode: number | null;
  /** Set when VCE stopped the shell because it lost contact with it. */
  reason: string | null;
}

export interface DirEntryInfo {
  name: string;
  path: string;
  isDir: boolean;
  isHidden: boolean;
  isSymlink: boolean;
  size: number | null;
  modifiedMs: number | null;
}

export interface DirListing {
  path: string;
  parent: string | null;
  entries: DirEntryInfo[];
  total: number;
  truncated: boolean;
  unreadable: number;
}

export interface FsOpResult {
  paths: string[];
  equivalent: string;
}

export type RiskLevel = "safe" | "caution" | "danger";

export interface RiskReport {
  level: RiskLevel;
  reasons: string[];
}

export interface Param {
  name: string;
  description: string;
}

export type HitKind = "recipe" | "cmdlet" | "program" | "example";

export interface SearchHit {
  id: string;
  kind: HitKind;
  title: string;
  command: string;
  description: string;
  params: Param[];
  example: string | null;
  category: string | null;
  /** For cmdlets: the parameters of the default parameter set. */
  usage: string | null;
  risk: RiskReport;
  score: number;
}

export interface IndexStatus {
  state: "building" | "ready" | "failed";
  messages: string[];
  documents: number;
  programs: number;
  psCommands: number;
  /** Cmdlets with a description from installed help files. */
  helpDescriptions: number;
  /** tldr-pages examples that apply on this machine. */
  examples: number;
  semantic: boolean;
}

export interface Suggestion {
  rule: string;
  title: string;
  explanation: string;
  command: string | null;
  risk: RiskReport | null;
}

export interface FailureReport {
  commandLine: string;
  cwd: string | null;
  error: PsError | null;
  output: string;
}

export interface FilledCommand {
  command: string;
  missing: string[];
  risk: RiskReport;
}

export interface AiStatus {
  engineAvailable: boolean;
  modelPath: string | null;
  modelReady: boolean;
  usingDefaultModel: boolean;
  defaultModel: { name: string; bytes: number; license: string };
  generating: boolean;
  downloading: boolean;
}

export type AiStage = "loading" | "generating" | "unloaded";

export interface DownloadProgress {
  downloaded: number;
  total: number;
}

export interface GeneratedCommand {
  command: string;
  explanation: string;
  risk: RiskReport;
  warnings: string[];
}

export type ThemeSetting = "system" | "dark" | "light";
export type ScreenReaderSetting = "auto" | "on" | "off";

export interface UiSettings {
  theme: ThemeSetting;
  fontSize: number;
  screenReaderMode: ScreenReaderSetting;
  showHidden: boolean;
  explorerWidth: number;
  /** Look for a new VCE version when VCE starts. */
  checkForUpdates: boolean;
}

export interface UpdateInfo {
  version: string;
  currentVersion: string;
  notes: string | null;
}

export interface UpdateProgress {
  downloaded: number;
  total: number | null;
}

export interface Settings {
  modelPath: string | null;
  ui: Partial<UiSettings> | null;
}
