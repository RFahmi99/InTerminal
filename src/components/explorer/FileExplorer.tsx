import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { announce } from "../../a11y/announcer";
import { registerRegion, focusRegion } from "../../focusRegistry";
import { api, toAppError } from "../../ipc/api";
import type { FsOpResult } from "../../ipc/types";
import { notifyDone, notifyFailure } from "../../notify";
import { useAppStore } from "../../state/store";
import { Dialog } from "../common/Dialog";
import { Icon } from "../common/Icon";
import { PATH_DRAG_TYPE } from "../terminal/TerminalPane";
import { ContextMenu, type MenuItem } from "./ContextMenu";
import {
  breadcrumbs,
  flattenTree,
  formatSize,
  pathKey,
  typeaheadMatch,
  type ListingState,
  type TreeRow,
} from "./treeModel";

type PendingDialog =
  | { kind: "newFolder"; parent: string; name: string; error: string | null }
  | { kind: "trash"; row: TreeRow };

const TYPEAHEAD_RESET_MS = 700;

function parentOf(path: string): string | null {
  const trimmed = path.replace(/\\+$/, "");
  const index = trimmed.lastIndexOf("\\");
  if (index < 0) return null;
  const parent = trimmed.slice(0, index);
  return /^[A-Za-z]:$/.test(parent) ? `${parent}\\` : parent || null;
}

/** Confirms a file operation, teaching the equivalent command. */
function reportDone(message: string, result: FsOpResult) {
  notifyDone(message, result.equivalent.startsWith("#") ? undefined : result.equivalent);
}

/** Whether focus is somewhere the explorer may take it from: nowhere, or one of its own rows. */
function focusIsFree(explorer: HTMLElement | null): boolean {
  const active = document.activeElement;
  if (!active || active === document.body) return true;
  return Boolean(explorer?.contains(active)) && !active.closest('[role="dialog"], [role="menu"]');
}

export function FileExplorer() {
  const cwd = useAppStore((s) => s.shell.cwd);
  const lastPrompt = useAppStore((s) => s.shell.lastPrompt);
  const showHidden = useAppStore((s) => s.ui.showHidden);
  const setUi = useAppStore((s) => s.setUi);

  const [listings, setListings] = useState<Map<string, ListingState>>(new Map());
  const [expanded, setExpanded] = useState<Map<string, string>>(new Map()); // key -> real path
  const [focusedKey, setFocusedKey] = useState<string | null>(null);
  const [renamingKey, setRenamingKey] = useState<string | null>(null);
  const [cutPaths, setCutPaths] = useState<string[]>([]);
  const [menu, setMenu] = useState<{ x: number; y: number; row: TreeRow } | null>(null);
  const [dialog, setDialog] = useState<PendingDialog | null>(null);
  const [dropKey, setDropKey] = useState<string | null>(null);
  const navRef = useRef<HTMLElement>(null);
  const treeRef = useRef<HTMLUListElement>(null);
  const upButtonRef = useRef<HTMLButtonElement>(null);
  const typeahead = useRef({ text: "", at: 0 });
  /**
   * A row to focus once it's rendered (`null`: the tree's current tab stop).
   * Kept until the row exists, e.g. after a reload, but dropped if the user
   * has moved focus elsewhere, so the tree never steals focus back.
   */
  const pendingFocus = useRef<{ key: string | null } | null>(null);
  const [focusTick, setFocusTick] = useState(0);
  const requestFocus = useCallback((key: string | null) => {
    pendingFocus.current = { key };
    if (key) setFocusedKey(key);
    setFocusTick((tick) => tick + 1);
  }, []);

  const load = useCallback(
    async (path: string) => {
      try {
        const listing = await api.fsList(path, showHidden);
        setListings((current) => new Map(current).set(pathKey(path), { listing }));
      } catch (error) {
        setListings((current) => new Map(current).set(pathKey(path), { error: toAppError(error).message }));
      }
    },
    [showHidden],
  );

  // A new working folder starts with a collapsed tree. If the user was in the
  // tree (e.g. pressed Enter on a folder), focus follows into the new one.
  useEffect(() => {
    setExpanded(new Map());
    setFocusedKey(null);
    setRenamingKey(null);
    if (focusIsFree(navRef.current)) requestFocus(null);
  }, [cwd, requestFocus]);

  const expandedRef = useRef(expanded);
  expandedRef.current = expanded;
  const refresh = useCallback(() => {
    if (!cwd) return;
    void load(cwd);
    expandedRef.current.forEach((path) => void load(path));
  }, [cwd, load]);

  // Reload after every command (it may have changed files) and when the
  // window regains focus (files may have changed outside the app).
  useEffect(refresh, [refresh, lastPrompt]);
  useEffect(() => {
    window.addEventListener("focus", refresh);
    return () => window.removeEventListener("focus", refresh);
  }, [refresh]);

  const root = cwd ? listings.get(pathKey(cwd)) : undefined;
  const expandedKeys = useMemo(() => new Set(expanded.keys()), [expanded]);
  const rows = useMemo(
    () => flattenTree(root?.listing?.entries ?? [], listings, expandedKeys),
    [root, listings, expandedKeys],
  );
  const focusedIndex = Math.max(0, rows.findIndex((r) => pathKey(r.entry.path) === focusedKey));
  const tabStopKey = rows.length ? pathKey(rows[focusedIndex].entry.path) : null;

  useEffect(() => {
    const pending = pendingFocus.current;
    if (!pending) return;
    if (!focusIsFree(navRef.current)) {
      pendingFocus.current = null;
      return;
    }
    const key = pending.key ?? tabStopKey;
    const element = key ? treeRef.current?.querySelector<HTMLElement>(`[data-key="${CSS.escape(key)}"]`) : null;
    if (element) {
      pendingFocus.current = null;
      element.focus();
      return;
    }
    // An empty (or unreadable) folder has no rows to land on; the way back
    // up is the most useful next step, and keeps focus out of the void.
    const folderShown = Boolean(root?.listing || root?.error);
    if (pending.key === null && folderShown && rows.length === 0) {
      pendingFocus.current = null;
      upButtonRef.current?.focus();
    }
    // Otherwise the rows aren't rendered yet; try again when they change.
  }, [focusTick, rows, tabStopKey, root]);

  useEffect(
    () =>
      registerRegion("explorer", () => {
        const target =
          treeRef.current?.querySelector<HTMLElement>('[role="treeitem"][tabindex="0"]') ??
          document.querySelector<HTMLElement>('[data-region="explorer"] button');
        target?.focus();
        return Boolean(target);
      }),
    [],
  );

  const focusRow = (index: number) => {
    const row = rows[Math.max(0, Math.min(rows.length - 1, index))];
    if (row) requestFocus(pathKey(row.entry.path));
  };

  const navigate = (path: string) => {
    api.shellChangeDir(path).catch((error) => notifyFailure(error));
  };

  const setExpandedState = (row: TreeRow, open: boolean) => {
    const key = pathKey(row.entry.path);
    setExpanded((current) => {
      const next = new Map(current);
      if (open) next.set(key, row.entry.path);
      else next.delete(key);
      return next;
    });
    if (open && !listings.get(key)?.listing) void load(row.entry.path);
  };

  const insertPath = (path: string) => {
    api
      .fillTemplate("<path>", { path })
      .then((filled) => api.shellInsert(`${filled.command} `))
      .then(() => focusRegion("terminal"))
      .catch((error) => notifyFailure(error, "Couldn't put the path on the command line"));
  };

  const copyPath = (path: string) => {
    navigator.clipboard
      .writeText(path)
      .then(() => announce("Path copied."))
      .catch((error) => notifyFailure(error, "Couldn't copy the path"));
  };

  const commitRename = (row: TreeRow, newName: string) => {
    const oldKey = pathKey(row.entry.path);
    setRenamingKey(null);
    if (!newName.trim() || newName === row.entry.name) {
      requestFocus(oldKey);
      return;
    }
    api
      .fsRename(row.entry.path, newName)
      .then(async (result) => {
        reportDone(`Renamed to ${newName.trim()}.`, result);
        await load(row.parentPath ?? cwd ?? "");
        requestFocus(pathKey(result.paths[0]));
      })
      .catch((error) => {
        notifyFailure(error);
        requestFocus(oldKey);
      });
  };

  const moveInto = (sources: string[], destination: string) => {
    api
      .fsMove(sources, destination)
      .then((result) => {
        const name = destination.slice(destination.lastIndexOf("\\") + 1) || destination;
        reportDone(`Moved ${sources.length === 1 ? "1 item" : `${sources.length} items`} into ${name}.`, result);
        setCutPaths([]);
      })
      .catch((error) => notifyFailure(error))
      // Refresh either way: a move can fail part-way through.
      .finally(refresh);
  };

  /** The row to focus after `row` disappears: the next sibling, else the previous, else its parent. */
  const neighbourKey = (row: TreeRow): string | null => {
    const index = rows.indexOf(row);
    const sameParent = (r: TreeRow) => r.level === row.level && r.parentPath === row.parentPath;
    const sibling = rows.slice(index + 1).find(sameParent) ?? rows.slice(0, index).reverse().find(sameParent);
    if (sibling) return pathKey(sibling.entry.path);
    return row.parentPath ? pathKey(row.parentPath) : null;
  };

  const closeDialog = (focusKey: string | null) => {
    setDialog(null);
    requestFocus(focusKey);
  };

  const menuItems = (row: TreeRow): MenuItem[] => {
    const { entry } = row;
    const isOpen = expanded.has(pathKey(entry.path));
    const common: MenuItem[] = [
      { label: "Put path on the command line", onSelect: () => insertPath(entry.path) },
      {
        label: "Copy path",
        shortcut: "Ctrl+C",
        onSelect: () => {
          copyPath(entry.path);
          requestFocus(pathKey(entry.path));
        },
      },
      { label: "Rename", shortcut: "F2", onSelect: () => setRenamingKey(pathKey(entry.path)) },
      {
        label: "Cut (to move it)",
        shortcut: "Ctrl+X",
        onSelect: () => {
          cut(row);
          requestFocus(pathKey(entry.path));
        },
      },
      {
        label: "Show in File Explorer",
        onSelect: () => {
          api.fsReveal(entry.path).catch((e) => notifyFailure(e));
          requestFocus(pathKey(entry.path));
        },
      },
      {
        label: "Move to Recycle Bin",
        shortcut: "Delete",
        separatorBefore: true,
        onSelect: () => setDialog({ kind: "trash", row }),
      },
    ];
    if (!entry.isDir) {
      const open = () => {
        api.fsOpen(entry.path).catch((e) => notifyFailure(e));
        requestFocus(pathKey(entry.path));
      };
      return [{ label: "Open", shortcut: "Enter", onSelect: open }, ...common];
    }
    return [
      { label: "Go to this folder in the terminal", shortcut: "Enter", onSelect: () => navigate(entry.path) },
      {
        label: isOpen ? "Collapse" : "Expand",
        onSelect: () => {
          setExpandedState(row, !isOpen);
          requestFocus(pathKey(entry.path));
        },
      },
      { label: "New folder inside…", onSelect: () => setDialog({ kind: "newFolder", parent: entry.path, name: "", error: null }) },
      ...(cutPaths.length
        ? [
            {
              label: "Paste here (move)",
              shortcut: "Ctrl+V",
              onSelect: () => {
                moveInto(cutPaths, entry.path);
                requestFocus(pathKey(entry.path));
              },
            },
          ]
        : []),
      ...common,
    ];
  };

  const cut = (row: TreeRow) => {
    setCutPaths([row.entry.path]);
    announce(`Cut ${row.entry.name}. Select a folder and press Control V to move it there.`);
  };

  const openMenuFor = (row: TreeRow, element: HTMLElement | null) => {
    const rect = element?.getBoundingClientRect();
    setMenu({ row, x: rect ? rect.left + 24 : 40, y: rect ? rect.bottom : 40 });
  };

  const onTreeKeyDown = (event: React.KeyboardEvent) => {
    const row = rows[focusedIndex];
    if (!row) return;
    const { entry } = row;
    const isOpen = expanded.has(pathKey(entry.path));
    const ctrl = event.ctrlKey && !event.altKey;
    const key = event.key;

    if (ctrl && key.toLowerCase() === "x") cut(row);
    else if (ctrl && key.toLowerCase() === "v" && cutPaths.length) moveInto(cutPaths, entry.isDir ? entry.path : (row.parentPath ?? cwd ?? ""));
    else if (ctrl && key.toLowerCase() === "c") copyPath(entry.path);
    else if (key === "ArrowDown") focusRow(focusedIndex + 1);
    else if (key === "ArrowUp") focusRow(focusedIndex - 1);
    else if (key === "Home") focusRow(0);
    else if (key === "End") focusRow(rows.length - 1);
    else if (key === "ArrowRight" && entry.isDir) {
      if (!isOpen) setExpandedState(row, true);
      else if (rows[focusedIndex + 1]?.level > row.level) focusRow(focusedIndex + 1);
    } else if (key === "ArrowLeft") {
      if (entry.isDir && isOpen) setExpandedState(row, false);
      else if (row.parentPath) focusRow(rows.findIndex((r) => pathKey(r.entry.path) === pathKey(row.parentPath!)));
    } else if (key === "Enter") {
      if (entry.isDir) navigate(entry.path);
      else api.fsOpen(entry.path).catch((e) => notifyFailure(e));
    } else if (key === "F2") setRenamingKey(pathKey(entry.path));
    else if (key === "Delete") setDialog({ kind: "trash", row });
    else if (key === "Backspace" && cwd) {
      const parent = parentOf(cwd);
      if (parent) navigate(parent);
    } else if (key === "ContextMenu" || (key === "F10" && event.shiftKey)) {
      openMenuFor(row, event.target as HTMLElement);
    } else if (key === "Escape" && cutPaths.length) {
      setCutPaths([]);
      announce("Move cancelled.");
    } else if (key.length === 1 && !event.ctrlKey && !event.altKey && key !== " ") {
      const now = Date.now();
      const state = typeahead.current;
      state.text = now - state.at < TYPEAHEAD_RESET_MS ? state.text + key : key;
      state.at = now;
      const match = typeaheadMatch(rows, focusedIndex, state.text);
      if (match >= 0) focusRow(match);
    } else {
      return;
    }
    event.preventDefault();
    event.stopPropagation();
  };

  const renderRows = (entries: TreeRow[]) =>
    entries.map((row) => {
      const { entry, level } = row;
      const key = pathKey(entry.path);
      const isOpen = entry.isDir && expanded.has(key);
      const children = isOpen ? rows.filter((r) => r.parentPath && pathKey(r.parentPath) === key) : [];
      const childState = isOpen ? listings.get(key) : undefined;
      return (
        <li
          key={key}
          role="treeitem"
          data-key={key}
          aria-level={level}
          aria-setsize={row.setsize}
          aria-posinset={row.posinset}
          aria-expanded={entry.isDir ? isOpen : undefined}
          aria-selected={key === tabStopKey}
          tabIndex={key === tabStopKey ? 0 : -1}
          onFocus={(event) => {
            if (event.target === event.currentTarget) setFocusedKey(key);
          }}
        >
          <div
            className={`tree-row${cutPaths.includes(entry.path) ? " cut" : ""}${dropKey === key ? " drop-target" : ""}`}
            style={{ "--level": level - 1 } as React.CSSProperties}
            title={entry.isDir ? entry.path : `${entry.path}\n${formatSize(entry.size)}`}
            draggable={renamingKey !== key}
            onDragStart={(event) => {
              event.dataTransfer.setData(PATH_DRAG_TYPE, entry.path);
              event.dataTransfer.setData("text/plain", entry.path);
              event.dataTransfer.effectAllowed = "copyMove";
            }}
            onDragOver={(event) => {
              if (entry.isDir && event.dataTransfer.types.includes(PATH_DRAG_TYPE)) {
                event.preventDefault();
                setDropKey(key);
              }
            }}
            onDragLeave={() => setDropKey((current) => (current === key ? null : current))}
            onDrop={(event) => {
              setDropKey(null);
              const source = event.dataTransfer.getData(PATH_DRAG_TYPE);
              if (!entry.isDir || !source || pathKey(source) === key) return;
              event.preventDefault();
              moveInto([source], entry.path);
            }}
            onClick={(event) => {
              setFocusedKey(key);
              // detail > 1 is the second click of a double-click; one cd is enough.
              if (entry.isDir && event.detail === 1) navigate(entry.path);
            }}
            onDoubleClick={() => {
              if (!entry.isDir) api.fsOpen(entry.path).catch((e) => notifyFailure(e));
            }}
            onContextMenu={(event) => {
              event.preventDefault();
              setFocusedKey(key);
              setMenu({ row, x: event.clientX, y: event.clientY });
            }}
          >
            <span
              className="tree-twisty"
              onClick={(event) => {
                if (!entry.isDir) return;
                event.stopPropagation();
                setExpandedState(row, !isOpen);
              }}
            >
              {entry.isDir && <Icon name={isOpen ? "chevronDown" : "chevronRight"} />}
            </span>
            <Icon name={entry.isDir ? "folder" : "file"} className={`tree-icon ${entry.isDir ? "folder" : "file"}`} />
            {renamingKey === key ? (
              <input
                className="tree-rename"
                aria-label={`New name for ${entry.name}`}
                defaultValue={entry.name}
                autoFocus
                onFocus={(event) => {
                  const dot = entry.isDir ? -1 : entry.name.lastIndexOf(".");
                  event.target.setSelectionRange(0, dot > 0 ? dot : entry.name.length);
                }}
                onClick={(event) => event.stopPropagation()}
                onKeyDown={(event) => {
                  event.stopPropagation();
                  if (event.key === "Enter") commitRename(row, event.currentTarget.value);
                  if (event.key === "Escape") {
                    setRenamingKey(null);
                    requestFocus(key);
                  }
                }}
                onBlur={(event) => commitRename(row, event.currentTarget.value)}
              />
            ) : (
              <span className={`tree-name${entry.isHidden ? " hidden-item" : ""}`}>
                {entry.name}
                {entry.isHidden && <span className="visually-hidden">, hidden</span>}
                {entry.isSymlink && <span className="visually-hidden">, link</span>}
                {!entry.isDir && entry.size !== null && <span className="visually-hidden">, {formatSize(entry.size)}</span>}
              </span>
            )}
          </div>
          {isOpen && (
            <ul role="group">
              {childState?.error && <li role="none" className="tree-note">Can't open this folder: {childState.error}</li>}
              {!childState && <li role="none" className="tree-note">Loading…</li>}
              {childState?.listing && children.length === 0 && <li role="none" className="tree-note">Empty folder</li>}
              {renderRows(children)}
            </ul>
          )}
        </li>
      );
    });

  const topLevel = rows.filter((r) => r.level === 1);
  const folderLabel = cwd ? (cwd.replace(/\\+$/, "").split("\\").pop() ?? cwd) : "";
  const notOnDisk = lastPrompt && lastPrompt.provider !== "FileSystem" && lastPrompt.cwd;

  return (
    <nav ref={navRef} className="explorer" aria-label="Files" data-region="explorer">
      <div className="explorer-header">
        <div className="explorer-title">
          <h2>Files</h2>
          <div className="explorer-actions">
            <button
              ref={upButtonRef}
              type="button"
              className="icon-button"
              aria-label="Go up one folder"
              title="Go up one folder (Backspace)"
              disabled={!cwd || !parentOf(cwd)}
              onClick={() => cwd && parentOf(cwd) && navigate(parentOf(cwd)!)}
            >
              <Icon name="arrowUp" />
            </button>
            <button
              type="button"
              className="icon-button"
              aria-label="New folder"
              title="New folder"
              disabled={!cwd}
              onClick={() => cwd && setDialog({ kind: "newFolder", parent: cwd, name: "", error: null })}
            >
              <Icon name="folderPlus" />
            </button>
            <button
              type="button"
              className="icon-button"
              aria-label="Show hidden files"
              aria-pressed={showHidden}
              title={showHidden ? "Hide hidden files" : "Show hidden files"}
              onClick={() => setUi({ showHidden: !showHidden })}
            >
              <Icon name={showHidden ? "eye" : "eyeOff"} />
            </button>
            <button type="button" className="icon-button" aria-label="Refresh" title="Refresh" onClick={refresh}>
              <Icon name="refresh" />
            </button>
          </div>
        </div>
        {cwd && (
          <nav className="breadcrumbs" aria-label="Current folder">
            <ol>
              {breadcrumbs(cwd).map((crumb, i, all) => (
                <li key={crumb.path}>
                  <button
                    type="button"
                    className="crumb"
                    aria-current={i === all.length - 1 ? "location" : undefined}
                    title={crumb.path}
                    onClick={() => i < all.length - 1 && navigate(crumb.path)}
                  >
                    {crumb.label}
                  </button>
                </li>
              ))}
            </ol>
          </nav>
        )}
      </div>

      {notOnDisk && (
        <p className="tree-note">The terminal is in {lastPrompt.cwd}, which isn't a folder on disk. The tree shows the last folder.</p>
      )}
      {!cwd && <p className="explorer-empty">Waiting for PowerShell to start…</p>}
      {root?.error && <p className="explorer-empty">Can't show this folder: {root.error}</p>}
      {root?.listing && root.listing.entries.length === 0 && (
        <p className="explorer-empty">This folder is empty. Create something with the New folder button, or from the terminal.</p>
      )}
      {rows.length > 0 && (
        <ul ref={treeRef} className="tree" role="tree" aria-label={`Contents of ${folderLabel}`} onKeyDown={onTreeKeyDown}>
          {renderRows(topLevel)}
        </ul>
      )}
      {root?.listing && root.listing.unreadable > 0 && (
        <p className="tree-note">
          {root.listing.unreadable === 1 ? "1 item" : `${root.listing.unreadable} items`} in this folder couldn't be read
          (often because of permissions) and aren't shown.
        </p>
      )}
      {root?.listing?.truncated && (
        <p className="tree-note">
          Showing {root.listing.entries.length.toLocaleString()} of {root.listing.total.toLocaleString()} items. Use the
          terminal to see everything.
        </p>
      )}

      {menu && (
        <ContextMenu
          label={`Actions for ${menu.row.entry.name}`}
          x={menu.x}
          y={menu.y}
          items={menuItems(menu.row)}
          onClose={(chosen) => {
            setMenu(null);
            // A chosen item decides where focus goes; dismissing returns it to the row.
            if (!chosen) requestFocus(pathKey(menu.row.entry.path));
          }}
        />
      )}

      {dialog?.kind === "trash" && (
        <Dialog
          narrow
          title={`Move ${dialog.row.entry.name} to the Recycle Bin?`}
          onClose={() => closeDialog(pathKey(dialog.row.entry.path))}
          footer={
            <>
              <button type="button" className="button" onClick={() => closeDialog(pathKey(dialog.row.entry.path))}>
                Cancel
              </button>
              <button
                type="button"
                className="button danger"
                onClick={() => {
                  const { row } = dialog;
                  const next = neighbourKey(row);
                  setDialog(null);
                  api
                    .fsTrash([row.entry.path])
                    .then(async (result) => {
                      reportDone(`Moved ${row.entry.name} to the Recycle Bin.`, result);
                      await load(row.parentPath ?? cwd ?? "");
                      requestFocus(next);
                    })
                    .catch((error) => {
                      notifyFailure(error);
                      requestFocus(pathKey(row.entry.path));
                    });
                }}
              >
                Move to Recycle Bin
              </button>
            </>
          }
        >
          <p>You can restore it from the Recycle Bin later.</p>
          <p className="muted">
            In the terminal, <code>Remove-Item</code> deletes permanently instead, so InTerminal uses the Recycle Bin for this
            button.
          </p>
        </Dialog>
      )}

      {dialog?.kind === "newFolder" && (
        <Dialog
          narrow
          title="New folder"
          onClose={() => closeDialog(null)}
          footer={
            <>
              <button type="button" className="button" onClick={() => closeDialog(null)}>
                Cancel
              </button>
              <button type="submit" form="new-folder-form" className="button primary">
                Create folder
              </button>
            </>
          }
        >
          <form
            id="new-folder-form"
            className="field"
            onSubmit={(event) => {
              event.preventDefault();
              const { parent, name } = dialog;
              api
                .fsCreateDir(parent, name)
                .then(async (result) => {
                  setDialog(null);
                  reportDone(`Created folder ${name.trim()}.`, result);
                  await load(parent);
                  requestFocus(pathKey(result.paths[0]));
                })
                .catch((error) => setDialog({ ...dialog, error: toAppError(error).message }));
            }}
          >
            <label htmlFor="new-folder-name">Folder name</label>
            <input
              id="new-folder-name"
              className="text-input"
              value={dialog.name}
              onChange={(event) => setDialog({ ...dialog, name: event.target.value, error: null })}
              aria-invalid={dialog.error ? true : undefined}
              aria-describedby={dialog.error ? "new-folder-error" : "new-folder-hint"}
              autoFocus
            />
            {dialog.error ? (
              <p id="new-folder-error" className="notice error" role="alert">
                {dialog.error}
              </p>
            ) : (
              <p id="new-folder-hint" className="hint">
                Created inside {dialog.parent}
              </p>
            )}
          </form>
        </Dialog>
      )}
    </nav>
  );
}
