import { useEffect, useId, useRef, type ReactNode, type RefObject } from "react";
import { Icon } from "./Icon";

const FOCUSABLE =
  'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

interface DialogProps {
  title: string;
  onClose: () => void;
  children: ReactNode;
  footer?: ReactNode;
  narrow?: boolean;
  /** Element to focus when the dialog opens; defaults to the first focusable one. */
  initialFocus?: RefObject<HTMLElement | null>;
  /** Replaces the standard title header, e.g. with a search field. */
  header?: ReactNode;
}

/**
 * A modal dialog that traps Tab, closes on Esc or a click outside, and puts
 * focus back where it was when it closes.
 */
export function Dialog({ title, onClose, children, footer, narrow, initialFocus, header }: DialogProps) {
  const titleId = useId();
  const dialogRef = useRef<HTMLDivElement>(null);
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;

  useEffect(() => {
    const previouslyFocused = document.activeElement as HTMLElement | null;
    const dialog = dialogRef.current;
    const target = initialFocus?.current ?? dialog?.querySelector<HTMLElement>(FOCUSABLE) ?? dialog;
    target?.focus();
    return () => {
      if (previouslyFocused && document.contains(previouslyFocused)) previouslyFocused.focus();
    };
    // Focus once on open; initialFocus is a ref and stable.
  }, []);

  const onKeyDown = (event: React.KeyboardEvent) => {
    if (event.key === "Escape") {
      event.stopPropagation();
      onCloseRef.current();
      return;
    }
    if (event.key !== "Tab" || !dialogRef.current) return;
    const focusable = Array.from(dialogRef.current.querySelectorAll<HTMLElement>(FOCUSABLE)).filter(
      (el) => !el.closest("[hidden], [inert]"),
    );
    if (focusable.length === 0) return;
    const first = focusable[0];
    const last = focusable[focusable.length - 1];
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first.focus();
    }
  };

  return (
    <div
      className="scrim"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) onCloseRef.current();
      }}
    >
      <div
        ref={dialogRef}
        className={`dialog${narrow ? " narrow" : ""}`}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        tabIndex={-1}
        onKeyDown={onKeyDown}
      >
        {header ? (
          <>
            <h2 id={titleId} className="visually-hidden">
              {title}
            </h2>
            {header}
          </>
        ) : (
          <div className="dialog-header">
            <h2 id={titleId}>{title}</h2>
            <button type="button" className="icon-button" onClick={() => onCloseRef.current()} aria-label="Close">
              <Icon name="close" />
            </button>
          </div>
        )}
        <div className="dialog-body">{children}</div>
        {footer && <div className="dialog-footer">{footer}</div>}
      </div>
    </div>
  );
}
