import { useEffect, useState } from "react";
import { useAppStore, type Toast } from "../state/store";
import { CommandTape } from "./common/CommandTape";
import { Icon } from "./common/Icon";

const TOAST_MS = 9000;

/**
 * Visual confirmations. Their text is also announced through the live
 * regions when raised, so this list itself isn't live (no double reading).
 */
export function Toasts() {
  const toasts = useAppStore((s) => s.toasts);
  if (toasts.length === 0) return null;
  return (
    <ul className="toasts" aria-label="Notifications" style={{ listStyle: "none", margin: 0, padding: 0 }}>
      {toasts.map((toast) => (
        <ToastItem key={toast.id} toast={toast} />
      ))}
    </ul>
  );
}

function ToastItem({ toast }: { toast: Toast }) {
  const dismiss = useAppStore((s) => s.dismissToast);
  const [paused, setPaused] = useState(false);

  useEffect(() => {
    if (paused) return;
    const timer = setTimeout(() => dismiss(toast.id), TOAST_MS);
    return () => clearTimeout(timer);
  }, [paused, dismiss, toast.id]);

  return (
    <li
      className="toast"
      onMouseEnter={() => setPaused(true)}
      onMouseLeave={() => setPaused(false)}
      onFocus={() => setPaused(true)}
      onBlur={() => setPaused(false)}
    >
      <div>
        <p>{toast.message}</p>
        {toast.command && (
          <>
            <p className="muted">In the terminal, that's:</p>
            <CommandTape command={toast.command} />
          </>
        )}
      </div>
      <button type="button" className="icon-button" aria-label="Dismiss notification" onClick={() => dismiss(toast.id)}>
        <Icon name="close" />
      </button>
    </li>
  );
}
