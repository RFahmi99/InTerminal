// Lets shortcuts and F6 move focus between the window's areas without the
// areas having to know about each other.

export type Region = "toolbar" | "explorer" | "terminal" | "suggestions";

const ORDER: Region[] = ["toolbar", "explorer", "terminal", "suggestions"];
const focusers = new Map<Region, () => boolean>();

/** Registers how to focus a region; the function returns false when the region is unavailable. */
export function registerRegion(region: Region, focus: () => boolean): () => void {
  focusers.set(region, focus);
  return () => {
    if (focusers.get(region) === focus) focusers.delete(region);
  };
}

export function focusRegion(region: Region): boolean {
  return focusers.get(region)?.() ?? false;
}

/** Which region currently holds focus, judged by the nearest marked ancestor. */
export function currentRegion(): Region | null {
  const element = document.activeElement?.closest<HTMLElement>("[data-region]");
  return (element?.dataset.region as Region | undefined) ?? null;
}

/** F6 / Shift+F6: move to the next available region, wrapping around. */
export function cycleRegion(step: 1 | -1): void {
  const current = currentRegion();
  let index = current ? ORDER.indexOf(current) : -1;
  for (let attempt = 0; attempt < ORDER.length; attempt++) {
    index = (index + step + ORDER.length) % ORDER.length;
    if (focusRegion(ORDER[index])) return;
  }
}
