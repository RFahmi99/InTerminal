// Pure helpers behind the file tree: which rows are visible, how the path
// breaks into breadcrumbs, and type-ahead matching.

import type { DirEntryInfo, DirListing } from "../../ipc/types";

export interface ListingState {
  listing?: DirListing;
  error?: string;
}

export interface TreeRow {
  entry: DirEntryInfo;
  level: number;
  parentPath: string | null;
  posinset: number;
  setsize: number;
}

/** Windows paths are case-insensitive, so maps are keyed by lower case. */
export const pathKey = (path: string) => path.toLowerCase();

/** The visible rows in display order: each entry, then its children if expanded. */
export function flattenTree(
  entries: DirEntryInfo[],
  listings: Map<string, ListingState>,
  expanded: Set<string>,
  level = 1,
  parentPath: string | null = null,
): TreeRow[] {
  const rows: TreeRow[] = [];
  entries.forEach((entry, i) => {
    rows.push({ entry, level, parentPath, posinset: i + 1, setsize: entries.length });
    if (entry.isDir && expanded.has(pathKey(entry.path))) {
      const children = listings.get(pathKey(entry.path))?.listing?.entries ?? [];
      rows.push(...flattenTree(children, listings, expanded, level + 1, entry.path));
    }
  });
  return rows;
}

export interface Crumb {
  label: string;
  path: string;
}

/** "C:\Users\me" -> C:\ > Users > me, each with the path to navigate to. */
export function breadcrumbs(cwd: string): Crumb[] {
  const unc = cwd.match(/^(\\\\[^\\]+\\[^\\]+)(.*)$/);
  const [rootLabel, rootPath, rest] = unc
    ? [unc[1], `${unc[1]}\\`, unc[2]]
    : (() => {
        const drive = cwd.match(/^([A-Za-z]:)(.*)$/);
        return drive ? [drive[1], `${drive[1]}\\`, drive[2]] : ["", "", cwd];
      })();
  const crumbs: Crumb[] = rootLabel ? [{ label: rootLabel, path: rootPath }] : [];
  let current = rootPath.replace(/\\$/, "");
  for (const part of rest.split("\\").filter(Boolean)) {
    current = current ? `${current}\\${part}` : part;
    crumbs.push({ label: part, path: current });
  }
  return crumbs;
}

/**
 * Index of the next row (after `fromIndex`, wrapping) whose name starts
 * with `typed`, or -1.
 */
export function typeaheadMatch(rows: TreeRow[], fromIndex: number, typed: string): number {
  const needle = typed.toLowerCase();
  // A repeated single letter cycles through matches; a longer prefix may
  // still match the current row.
  const offset = needle.length > 1 ? 0 : 1;
  for (let step = 0; step < rows.length; step++) {
    const index = (fromIndex + offset + step) % rows.length;
    if (rows[index].entry.name.toLowerCase().startsWith(needle)) return index;
  }
  return -1;
}

export function formatSize(bytes: number | null): string {
  if (bytes === null) return "";
  const units = ["bytes", "KB", "MB", "GB", "TB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit++;
  }
  return unit === 0 ? `${value} ${units[0]}` : `${value.toFixed(value < 10 ? 1 : 0)} ${units[unit]}`;
}
