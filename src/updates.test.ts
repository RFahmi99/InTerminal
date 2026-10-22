import { beforeEach, describe, expect, it, vi } from "vitest";
import { announce } from "./a11y/announcer";
import { api } from "./ipc/api";
import { useAppStore } from "./state/store";
import { checkForUpdates, downloadPercent, installUpdate, recordProgress } from "./updates";

vi.mock("./a11y/announcer", () => ({ announce: vi.fn() }));
vi.mock("./ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./ipc/api")>();
  return { ...actual, api: { updateCheck: vi.fn(), updateInstall: vi.fn() } };
});

const available = { version: "0.2.0", currentVersion: "0.1.0", notes: "Faster search." };

beforeEach(() => {
  vi.clearAllMocks();
  useAppStore.setState({ toasts: [], update: { phase: "idle", info: null, percent: null, error: null } });
});

describe("checkForUpdates", () => {
  it("offers a found update and says so", async () => {
    vi.mocked(api.updateCheck).mockResolvedValue(available);
    await checkForUpdates("startup");
    expect(useAppStore.getState().update).toMatchObject({ phase: "available", info: available });
    expect(useAppStore.getState().toasts[0].message).toBe("InTerminal 0.2.0 is available. Open Settings to install it.");
  });

  it("stays quiet when a startup check fails, but reports a requested one", async () => {
    vi.mocked(api.updateCheck).mockRejectedValue({ code: "updateFailed", message: "Couldn't check for updates: offline" });
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    await checkForUpdates("startup");
    expect(useAppStore.getState().toasts).toHaveLength(0);
    expect(warn).toHaveBeenCalled();
    expect(useAppStore.getState().update.phase).toBe("failed");

    await checkForUpdates("user");
    expect(useAppStore.getState().toasts[0].message).toBe("Couldn't check for updates: offline");
    warn.mockRestore();
  });

  it("skips the startup check once the user has acted or turned it off", async () => {
    useAppStore.setState({ update: { phase: "installing", info: available, percent: 40, error: null } });
    await checkForUpdates("startup");
    expect(useAppStore.getState().update).toMatchObject({ phase: "installing", percent: 40 });

    useAppStore.setState({ update: { phase: "idle", info: null, percent: null, error: null } });
    useAppStore.getState().setUi({ checkForUpdates: false });
    await checkForUpdates("startup");
    expect(api.updateCheck).not.toHaveBeenCalled();
    useAppStore.getState().setUi({ checkForUpdates: true });
  });

  it("confirms being up to date only when asked", async () => {
    vi.mocked(api.updateCheck).mockResolvedValue(null);
    await checkForUpdates("startup");
    expect(announce).not.toHaveBeenCalled();
    await checkForUpdates("user");
    expect(announce).toHaveBeenCalledWith("You have the latest version of InTerminal.");
    expect(useAppStore.getState().update.phase).toBe("upToDate");
  });
});

describe("installing", () => {
  it("reports progress by whole percent and announces each quarter", () => {
    expect(downloadPercent({ downloaded: 5, total: null })).toBeNull();
    expect(downloadPercent({ downloaded: 499, total: 1000 })).toBe(49);
    recordProgress({ downloaded: 250, total: 1000 });
    recordProgress({ downloaded: 251, total: 1000 });
    expect(useAppStore.getState().update.percent).toBe(25);
    expect(announce).toHaveBeenCalledTimes(1);
  });

  it("keeps the update on offer when installing fails", async () => {
    useAppStore.setState({ update: { phase: "available", info: available, percent: null, error: null } });
    vi.mocked(api.updateInstall).mockRejectedValue({ code: "updateFailed", message: "The update couldn't be installed: bad signature" });
    await installUpdate();
    expect(useAppStore.getState().update).toMatchObject({ phase: "failed", info: available, error: "The update couldn't be installed: bad signature" });
    expect(useAppStore.getState().toasts[0].message).toBe("The update couldn't be installed: bad signature");
  });
});
