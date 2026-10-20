import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { announce, subscribe } from "./a11y/announcer";
import { combineRisk, hasPlaceholders } from "./commands";
import { breadcrumbs, flattenTree, formatSize, typeaheadMatch, type ListingState } from "./components/explorer/treeModel";
import { readLines, type BufferLike } from "./components/terminal/outputCapture";
import type { DirEntryInfo } from "./ipc/types";
import { matchShortcut } from "./shortcuts";

const key = (key: string, mods: Partial<Record<"ctrlKey" | "shiftKey" | "altKey" | "metaKey", boolean>> = {}) => ({
  key,
  ctrlKey: false,
  shiftKey: false,
  altKey: false,
  metaKey: false,
  ...mods,
});

describe("matchShortcut", () => {
  it("matches app shortcuts regardless of the shifted key's case", () => {
    expect(matchShortcut(key("P", { ctrlKey: true, shiftKey: true }))).toBe("openSearch");
    expect(matchShortcut(key("`", { ctrlKey: true }))).toBe("focusTerminal");
    expect(matchShortcut(key("F6", { shiftKey: true }))).toBe("previousRegion");
  });

  it("leaves shell keys alone", () => {
    expect(matchShortcut(key("c", { ctrlKey: true }))).toBeNull();
    expect(matchShortcut(key("p", { ctrlKey: true }))).toBeNull();
    expect(matchShortcut(key("F6", { shiftKey: true, metaKey: true }))).toBeNull();
  });
});

const entry = (path: string, isDir: boolean): DirEntryInfo => ({
  name: path.slice(path.lastIndexOf("\\") + 1),
  path,
  isDir,
  isHidden: false,
  isSymlink: false,
  size: isDir ? null : 10,
  modifiedMs: null,
});

describe("tree model", () => {
  const root = [entry("C:\\p\\src", true), entry("C:\\p\\README.md", false)];
  const listings = new Map<string, ListingState>([
    [
      "c:\\p\\src",
      {
        listing: {
          path: "C:\\p\\src",
          parent: "C:\\p",
          entries: [entry("C:\\p\\src\\main.ts", false)],
          total: 1,
          truncated: false,
          unreadable: 0,
        },
      },
    ],
  ]);

  it("shows children only for expanded folders, with ARIA positions", () => {
    expect(flattenTree(root, listings, new Set()).map((r) => r.entry.name)).toEqual(["src", "README.md"]);
    const rows = flattenTree(root, listings, new Set(["c:\\p\\src"]));
    expect(rows.map((r) => [r.entry.name, r.level, r.posinset, r.setsize])).toEqual([
      ["src", 1, 1, 2],
      ["main.ts", 2, 1, 1],
      ["README.md", 1, 2, 2],
    ]);
    expect(rows[1].parentPath).toBe("C:\\p\\src");
  });

  it("splits drive and UNC paths into breadcrumbs", () => {
    expect(breadcrumbs("C:\\Users\\me")).toEqual([
      { label: "C:", path: "C:\\" },
      { label: "Users", path: "C:\\Users" },
      { label: "me", path: "C:\\Users\\me" },
    ]);
    expect(breadcrumbs("\\\\server\\share\\docs")).toEqual([
      { label: "\\\\server\\share", path: "\\\\server\\share\\" },
      { label: "docs", path: "\\\\server\\share\\docs" },
    ]);
  });

  it("type-ahead cycles on repeated letters and matches prefixes", () => {
    const rows = flattenTree([entry("C:\\a", true), entry("C:\\b1", true), entry("C:\\b2", true)], new Map(), new Set());
    expect(typeaheadMatch(rows, 0, "b")).toBe(1);
    expect(typeaheadMatch(rows, 1, "b")).toBe(2);
    expect(typeaheadMatch(rows, 1, "b1")).toBe(1);
    expect(typeaheadMatch(rows, 0, "z")).toBe(-1);
  });

  it("formats sizes readably", () => {
    expect(formatSize(512)).toBe("512 bytes");
    expect(formatSize(1536)).toBe("1.5 KB");
    expect(formatSize(50 * 1024 * 1024)).toBe("50 MB");
  });
});

describe("readLines", () => {
  const buffer = (lines: [string, boolean][]): BufferLike => ({
    length: lines.length,
    getLine: (i) => (lines[i] ? { isWrapped: lines[i][1], translateToString: () => lines[i][0] } : undefined),
  });

  it("re-joins soft-wrapped lines and trims trailing blanks", () => {
    const text = readLines(
      buffer([
        ["PS> npm run dev", false],
        ["Error: listen EADDRINUSE: address already in use :", false],
        ["::3000", true],
        ["", false],
      ]),
      1,
      4,
    );
    expect(text).toBe("Error: listen EADDRINUSE: address already in use :::3000");
  });

  it("keeps the end of very long output", () => {
    const text = readLines(buffer([["x".repeat(50), false], ["the error", false]]), 0, 2, 20);
    expect(text.endsWith("the error")).toBe(true);
    expect(text.length).toBe(20);
  });
});

describe("command helpers", () => {
  it("detects unfilled placeholders", () => {
    expect(hasPlaceholders("Expand-Archive -Path <archive>")).toBe(true);
    expect(hasPlaceholders("Get-Process | Where-Object CPU -gt 5")).toBe(false);
  });

  it("combines risk reports to the most severe level", () => {
    const combined = combineRisk({ level: "caution", reasons: ["a"] }, { level: "danger", reasons: ["a", "b"] });
    expect(combined).toEqual({ level: "danger", reasons: ["a", "b"] });
  });
});

describe("announce", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it("coalesces keyed announcements into the latest one", () => {
    const heard: string[] = [];
    const unsubscribe = subscribe((message) => heard.push(message));
    announce("Folder changed to a", "polite", "cwd");
    announce("Folder changed to b", "polite", "cwd");
    announce("Unkeyed", "assertive");
    expect(heard).toEqual(["Unkeyed"]);
    vi.advanceTimersByTime(1000);
    expect(heard).toEqual(["Unkeyed", "Folder changed to b"]);
    unsubscribe();
  });
});
