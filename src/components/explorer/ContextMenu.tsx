import { Fragment, useEffect, useLayoutEffect, useRef, useState } from "react";

export interface MenuItem {
  label: string;
  shortcut?: string;
  onSelect: () => void;
  separatorBefore?: boolean;
}

interface ContextMenuProps {
  label: string;
  x: number;
  y: number;
  items: MenuItem[];
  /**
   * Called on Esc, Tab or an outside click (`chosen` false), or just before an
   * item's action runs (`chosen` true), so the caller knows who decides where
   * focus goes next.
   */
  onClose: (chosen: boolean) => void;
}

/** A WAI-ARIA menu: arrow keys move, Enter chooses, Esc closes. */
export function ContextMenu({ label, x, y, items, onClose }: ContextMenuProps) {
  const menuRef = useRef<HTMLUListElement>(null);
  const [position, setPosition] = useState({ left: x, top: y });

  useLayoutEffect(() => {
    const menu = menuRef.current;
    if (!menu) return;
    const { width, height } = menu.getBoundingClientRect();
    setPosition({
      left: Math.max(4, Math.min(x, window.innerWidth - width - 4)),
      top: Math.max(4, Math.min(y, window.innerHeight - height - 4)),
    });
    menu.querySelector<HTMLElement>('[role="menuitem"]')?.focus();
  }, [x, y]);

  useEffect(() => {
    const onPointerDown = (event: MouseEvent) => {
      if (!menuRef.current?.contains(event.target as Node)) onClose(false);
    };
    document.addEventListener("mousedown", onPointerDown);
    return () => document.removeEventListener("mousedown", onPointerDown);
  }, [onClose]);

  const onKeyDown = (event: React.KeyboardEvent) => {
    const menuItems = Array.from(menuRef.current?.querySelectorAll<HTMLElement>('[role="menuitem"]') ?? []);
    const index = menuItems.indexOf(document.activeElement as HTMLElement);
    const move = (to: number) => menuItems[(to + menuItems.length) % menuItems.length]?.focus();
    switch (event.key) {
      case "ArrowDown":
        move(index + 1);
        break;
      case "ArrowUp":
        move(index - 1);
        break;
      case "Home":
        move(0);
        break;
      case "End":
        move(menuItems.length - 1);
        break;
      case "Escape":
      case "Tab":
        onClose(false);
        break;
      default:
        return;
    }
    event.preventDefault();
    event.stopPropagation();
  };

  const choose = (item: MenuItem) => {
    onClose(true);
    item.onSelect();
  };

  return (
    <ul ref={menuRef} className="menu" role="menu" aria-label={label} style={position} onKeyDown={onKeyDown}>
      {items.map((item) => (
        <Fragment key={item.label}>
          {item.separatorBefore && <li role="separator" />}
          <li role="none">
          <div
            role="menuitem"
            tabIndex={-1}
            aria-keyshortcuts={item.shortcut}
            onClick={() => choose(item)}
            onKeyDown={(event) => {
              if (event.key === "Enter" || event.key === " ") {
                event.preventDefault();
                choose(item);
              }
            }}
          >
            {item.label}
            {item.shortcut && <kbd style={{ float: "right", marginLeft: 16 }}>{item.shortcut}</kbd>}
          </div>
          </li>
        </Fragment>
      ))}
    </ul>
  );
}
