// Screen-reader announcements through ARIA live regions. Background changes
// (the shell changing folder, a search finishing, a command failing) are
// spoken without moving focus.

export type Politeness = "polite" | "assertive";

type Listener = (message: string, politeness: Politeness) => void;

const listeners = new Set<Listener>();
const pending = new Map<string, ReturnType<typeof setTimeout>>();

/** How long a keyed announcement waits for a newer one with the same key. */
const COALESCE_MS = 350;

export function subscribe(listener: Listener): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/**
 * Speaks `message`. Announcements with the same `key` replace each other
 * within a short window, so rapid repeats (e.g. a script changing folder ten
 * times) are read once, with the final state.
 */
export function announce(message: string, politeness: Politeness = "polite", key?: string): void {
  const deliver = () => listeners.forEach((listener) => listener(message, politeness));
  if (!key) {
    deliver();
    return;
  }
  const existing = pending.get(key);
  if (existing) clearTimeout(existing);
  pending.set(
    key,
    setTimeout(() => {
      pending.delete(key);
      deliver();
    }, COALESCE_MS),
  );
}
