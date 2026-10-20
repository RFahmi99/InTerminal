import { useEffect } from "react";
import { LiveRegions } from "./a11y/LiveRegions";
import { AiPanel } from "./components/ai/AiPanel";
import { FileExplorer } from "./components/explorer/FileExplorer";
import { HelpDialog } from "./components/HelpDialog";
import { Resizer } from "./components/Resizer";
import { RunConfirmDialog } from "./components/RunConfirmDialog";
import { CommandPalette } from "./components/search/CommandPalette";
import { SettingsDialog } from "./components/SettingsDialog";
import { StatusBar } from "./components/StatusBar";
import { SuggestionBar } from "./components/suggestions/SuggestionBar";
import { TerminalPane } from "./components/terminal/TerminalPane";
import { Toasts } from "./components/Toasts";
import { Toolbar } from "./components/Toolbar";
import { useBackendEvents } from "./hooks/useBackendEvents";
import { useGlobalShortcuts } from "./hooks/useGlobalShortcuts";
import { useSettingsSync } from "./hooks/useSettingsSync";
import { useAppStore } from "./state/store";
import { useResolvedTheme } from "./theme";

export default function App() {
  useSettingsSync();
  useBackendEvents();
  useGlobalShortcuts();

  const overlay = useAppStore((s) => s.overlay);
  const explorerWidth = useAppStore((s) => s.ui.explorerWidth);
  const theme = useResolvedTheme();

  useEffect(() => {
    document.documentElement.dataset.theme = theme;
  }, [theme]);

  return (
    <div className="app" style={{ "--explorer-width": `${explorerWidth}px` } as React.CSSProperties}>
      <Toolbar />
      <FileExplorer />
      <Resizer />
      <main className="workspace" aria-label="Terminal">
        <TerminalPane />
        <SuggestionBar />
      </main>
      <StatusBar />

      {overlay === "search" && <CommandPalette />}
      {overlay === "ai" && <AiPanel />}
      {overlay === "settings" && <SettingsDialog />}
      {overlay === "help" && <HelpDialog />}
      <RunConfirmDialog />
      <Toasts />
      <LiveRegions />
    </div>
  );
}
