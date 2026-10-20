// One way to tell the user something: spoken through the live regions for
// screen-reader users and shown as a toast for everyone else, so no message
// reaches only one audience.

import { announce } from "./a11y/announcer";
import { toAppError } from "./ipc/api";
import { useAppStore } from "./state/store";

export function notifyError(message: string): void {
  announce(message, "assertive");
  useAppStore.getState().pushToast({ message });
}

/** Reports a failed operation, prefixed with what was being attempted. */
export function notifyFailure(error: unknown, what?: string): void {
  const { message } = toAppError(error);
  notifyError(what ? `${what}: ${message}` : message);
}

/** A completed action, optionally with the command that does the same thing. */
export function notifyDone(message: string, command?: string): void {
  announce(message);
  useAppStore.getState().pushToast({ message, command });
}
