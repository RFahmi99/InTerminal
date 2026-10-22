import { useEffect, useRef } from "react";
import { registerRegion } from "../focusRegistry";
import { useAppStore } from "../state/store";
import { Icon } from "./common/Icon";

export function Toolbar() {
  const openOverlay = useAppStore((s) => s.openOverlay);
  const searchRef = useRef<HTMLButtonElement>(null);

  useEffect(
    () =>
      registerRegion("toolbar", () => {
        searchRef.current?.focus();
        return true;
      }),
    [],
  );

  return (
    <header className="toolbar" data-region="toolbar">
      <div className="wordmark">
        InTerminal
      </div>
      <button
        ref={searchRef}
        type="button"
        className="toolbar-search"
        aria-keyshortcuts="Control+Shift+P"
        onClick={() => openOverlay("search")}
      >
        <Icon name="search" />
        <span>Find a command by describing it</span>
        <kbd>Ctrl+Shift+P</kbd>
      </button>
      <div className="toolbar-spacer" />
      <button type="button" className="button" aria-keyshortcuts="Control+Shift+G" onClick={() => openOverlay("ai")}>
        <Icon name="sparkle" />
        Write a command with AI
      </button>
      <button type="button" className="icon-button" aria-label="Keyboard shortcuts" aria-keyshortcuts="F1" title="Keyboard shortcuts (F1)" onClick={() => openOverlay("help")}>
        <Icon name="help" />
      </button>
      <button type="button" className="icon-button" aria-label="Settings" aria-keyshortcuts="Control+," title="Settings (Ctrl+,)" onClick={() => openOverlay("settings")}>
        <Icon name="gear" />
      </button>
    </header>
  );
}
