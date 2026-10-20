import { useEffect, useState } from "react";
import { useAppStore } from "./state/store";

export type ResolvedTheme = "dark" | "light";

const LIGHT_QUERY = "(prefers-color-scheme: light)";

/** The theme in effect: the user's choice, or Windows' app mode for "system". */
export function useResolvedTheme(): ResolvedTheme {
  const setting = useAppStore((s) => s.ui.theme);
  const [systemLight, setSystemLight] = useState(() => window.matchMedia(LIGHT_QUERY).matches);

  useEffect(() => {
    const query = window.matchMedia(LIGHT_QUERY);
    const onChange = () => setSystemLight(query.matches);
    query.addEventListener("change", onChange);
    return () => query.removeEventListener("change", onChange);
  }, []);

  if (setting === "system") return systemLight ? "light" : "dark";
  return setting;
}
