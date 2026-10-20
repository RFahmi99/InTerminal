import { useEffect } from "react";
import { announce } from "../a11y/announcer";
import { cycleRegion, focusRegion } from "../focusRegistry";
import { matchShortcut, type ShortcutId } from "../shortcuts";
import { useAppStore, type Overlay } from "../state/store";

const OVERLAYS: Partial<Record<ShortcutId, Overlay>> = {
  openSearch: "search",
  openAi: "ai",
  openHelp: "help",
  openSettings: "settings",
};

/**
 * Handles app shortcuts in the capture phase, before the terminal or a
 * focused control sees the key.
 */
export function useGlobalShortcuts() {
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      const id = matchShortcut(event);
      if (!id) return;
      const store = useAppStore.getState();
      // A modal the shortcut can't close (a confirmation, the file tree's
      // dialogs) must keep focus, so app shortcuts wait until it's dismissed.
      const modal = document.querySelector('[aria-modal="true"]');
      if (modal && (store.overlay === null || store.pendingRun)) return;
      event.preventDefault();
      event.stopPropagation();
      const overlay = OVERLAYS[id];
      if (overlay !== undefined) {
        store.openOverlay(store.overlay === overlay ? null : overlay);
        return;
      }
      // Moving between areas closes any open dialog first.
      if (store.overlay) store.openOverlay(null);
      requestAnimationFrame(() => {
        switch (id) {
          case "focusExplorer":
            focusRegion("explorer");
            break;
          case "focusTerminal":
            focusRegion("terminal");
            break;
          case "focusSuggestions":
            if (!focusRegion("suggestions")) announce("There are no suggested fixes right now.");
            break;
          case "nextRegion":
            cycleRegion(1);
            break;
          case "previousRegion":
            cycleRegion(-1);
            break;
        }
      });
    };
    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
  }, []);
}
