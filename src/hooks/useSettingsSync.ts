import { useEffect, useRef } from "react";
import { api } from "../ipc/api";
import { notifyError, notifyFailure } from "../notify";
import { DEFAULT_UI_SETTINGS, useAppStore } from "../state/store";
import { checkForUpdates, STARTUP_CHECK_DELAY_MS } from "../updates";

const SAVE_DELAY_MS = 400;

/**
 * Loads saved UI settings once, then saves changes (debounced). Also starts
 * the launch-time update check, which depends on a saved setting.
 */
export function useSettingsSync() {
  const loaded = useRef(false);

  useEffect(() => {
    const store = useAppStore.getState;
    let updateTimer: ReturnType<typeof setTimeout> | undefined;
    api
      .settingsGet()
      .then((settings) => {
        store().setUi({ ...DEFAULT_UI_SETTINGS, ...(settings.ui ?? {}) });
        if (settings.loadWarning) notifyError(settings.loadWarning);
      })
      .catch((error) => notifyFailure(error, "Saved settings couldn't be loaded"))
      .finally(() => {
        loaded.current = true;
        // Only once the saved choice is known, so turning checks off is respected.
        if (store().ui.checkForUpdates) {
          updateTimer = setTimeout(() => void checkForUpdates("startup"), STARTUP_CHECK_DELAY_MS);
        }
      });
    api
      .screenReaderActive()
      .then(store().setScreenReaderDetected)
      .catch((error) => notifyFailure(error, "Screen reader detection failed, so turn on screen reader support in Settings if you need it"));

    let timer: ReturnType<typeof setTimeout> | undefined;
    const unsubscribe = useAppStore.subscribe((state, previous) => {
      if (!loaded.current || state.ui === previous.ui) return;
      clearTimeout(timer);
      timer = setTimeout(() => {
        api.settingsSetUi(state.ui).catch((error) => notifyFailure(error, "Settings couldn't be saved"));
      }, SAVE_DELAY_MS);
    });
    return () => {
      clearTimeout(timer);
      clearTimeout(updateTimer);
      unsubscribe();
    };
  }, []);
}
