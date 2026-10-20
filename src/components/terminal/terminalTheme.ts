import type { ITheme } from "@xterm/xterm";

// ANSI palettes tuned for VCE's grounds: every colour keeps at least 4.5:1
// contrast against the background so PowerShell's red errors and yellow
// warnings stay readable.

export const DARK_TERMINAL_THEME: ITheme = {
  background: "#13202d",
  foreground: "#e8eef4",
  cursor: "#6cb8ff",
  cursorAccent: "#13202d",
  selectionBackground: "rgba(108, 184, 255, 0.35)",
  black: "#30465c",
  red: "#ff8a80",
  green: "#7bd389",
  yellow: "#f4c04e",
  blue: "#6cb8ff",
  magenta: "#d7a6ff",
  cyan: "#6fdde0",
  white: "#d5dfe9",
  brightBlack: "#8296ab",
  brightRed: "#ffb0a8",
  brightGreen: "#a3e6ad",
  brightYellow: "#ffd98a",
  brightBlue: "#a3d3ff",
  brightMagenta: "#e8c8ff",
  brightCyan: "#a2eef0",
  brightWhite: "#ffffff",
};

export const LIGHT_TERMINAL_THEME: ITheme = {
  background: "#fbfcfd",
  foreground: "#17222d",
  cursor: "#0b62b8",
  cursorAccent: "#fbfcfd",
  selectionBackground: "rgba(11, 98, 184, 0.22)",
  black: "#17222d",
  red: "#b3261e",
  green: "#2e7d32",
  yellow: "#8a5a00",
  blue: "#0b62b8",
  magenta: "#8e3fb5",
  cyan: "#00727a",
  white: "#5b6b7c",
  brightBlack: "#4b5d70",
  brightRed: "#c62828",
  brightGreen: "#33691e",
  brightYellow: "#7a5000",
  brightBlue: "#0a4f94",
  brightMagenta: "#7b1fa2",
  brightCyan: "#006064",
  brightWhite: "#17222d",
};
