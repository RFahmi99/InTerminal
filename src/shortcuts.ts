// Every app-level keyboard shortcut, in one place: the global key handler,
// the terminal's pass-through filter and the help dialog all read this list,
// so they can't drift apart. All of them are combinations PowerShell's line
// editor doesn't use.

export type ShortcutId =
  | "openSearch"
  | "openAi"
  | "focusExplorer"
  | "focusTerminal"
  | "focusSuggestions"
  | "nextRegion"
  | "previousRegion"
  | "openHelp"
  | "openSettings";

export interface Shortcut {
  id: ShortcutId;
  keys: string;
  description: string;
}

export const SHORTCUTS: Shortcut[] = [
  { id: "openSearch", keys: "Ctrl+Shift+P", description: "Find a command by describing what you want to do" },
  { id: "openAi", keys: "Ctrl+Shift+G", description: "Write a command with the local AI" },
  { id: "focusExplorer", keys: "Ctrl+Shift+E", description: "Go to the file tree" },
  { id: "focusTerminal", keys: "Ctrl+`", description: "Go to the terminal" },
  { id: "focusSuggestions", keys: "Ctrl+.", description: "Go to the suggested fixes for the last error" },
  { id: "nextRegion", keys: "F6", description: "Move to the next area of the window" },
  { id: "previousRegion", keys: "Shift+F6", description: "Move to the previous area of the window" },
  { id: "openHelp", keys: "F1", description: "Show keyboard shortcuts" },
  { id: "openSettings", keys: "Ctrl+,", description: "Open settings" },
];

/** Keys handled inside specific areas, listed in the help dialog. */
export const AREA_SHORTCUTS: { area: string; keys: string; description: string }[] = [
  { area: "Terminal", keys: "Ctrl+C", description: "Copy the selection, or stop the running command when nothing is selected" },
  { area: "Terminal", keys: "Ctrl+V", description: "Paste" },
  { area: "File tree", keys: "Enter", description: "Open a folder in the terminal, or open a file" },
  { area: "File tree", keys: "Right / Left", description: "Expand or collapse a folder" },
  { area: "File tree", keys: "Shift+F10", description: "Show actions for the selected item" },
  { area: "File tree", keys: "F2", description: "Rename" },
  { area: "File tree", keys: "Delete", description: "Move to the Recycle Bin" },
  { area: "File tree", keys: "Ctrl+X, then Ctrl+V", description: "Move an item into another folder" },
  { area: "File tree", keys: "Backspace", description: "Go up one folder" },
  { area: "Command search", keys: "Enter", description: "Choose a command, then put it on the command line" },
  { area: "Command search", keys: "Ctrl+Enter", description: "Run the command" },
  { area: "Dialogs", keys: "Esc", description: "Close and return to where you were" },
];

interface ParsedKeys {
  ctrl: boolean;
  shift: boolean;
  alt: boolean;
  key: string;
}

function parse(keys: string): ParsedKeys {
  const parts = keys.split("+");
  const key = parts[parts.length - 1].toLowerCase();
  return {
    ctrl: parts.includes("Ctrl"),
    shift: parts.includes("Shift"),
    alt: parts.includes("Alt"),
    key,
  };
}

const PARSED = SHORTCUTS.map((s) => ({ id: s.id, ...parse(s.keys) }));

/** The app shortcut a key event triggers, if any. */
export function matchShortcut(event: Pick<KeyboardEvent, "key" | "ctrlKey" | "shiftKey" | "altKey" | "metaKey">): ShortcutId | null {
  if (event.metaKey) return null;
  const key = event.key.toLowerCase();
  const match = PARSED.find(
    (s) => s.ctrl === event.ctrlKey && s.shift === event.shiftKey && s.alt === event.altKey && s.key === key,
  );
  return match?.id ?? null;
}
