import { beforeEach, describe, expect, it, vi } from "vitest";

const assessRisk = vi.fn();
const shellRun = vi.fn();
vi.mock("./ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./ipc/api")>();
  return { ...actual, api: { ...actual.api, assessRisk, shellRun, shellInsert: vi.fn() } };
});

const { runCommand } = await import("./commands");
const { subscribeAll } = await import("./ipc/api");
const { useAppStore } = await import("./state/store");

describe("runCommand", () => {
  beforeEach(() => {
    assessRisk.mockReset();
    shellRun.mockReset().mockResolvedValue(undefined);
    useAppStore.getState().setPendingRun(null);
  });

  it("re-assesses the exact text, so an edit after a stale 'safe' rating still asks first", async () => {
    assessRisk.mockResolvedValue({ level: "danger", reasons: ["Deletes files permanently."] });
    const ran = await runCommand("Get-ChildItem *.tmp | Remove-Item -Recurse", { level: "safe", reasons: [] });
    expect(ran).toBe(false);
    expect(shellRun).not.toHaveBeenCalled();
    expect(useAppStore.getState().pendingRun?.risk.level).toBe("danger");
  });

  it("asks before running when the risk can't be assessed", async () => {
    assessRisk.mockRejectedValue(new Error("IPC down"));
    expect(await runCommand("anything", null)).toBe(false);
    expect(useAppStore.getState().pendingRun?.risk.reasons[0]).toMatch(/couldn't check/);
  });

  it("keeps the more severe of the known and fresh ratings", async () => {
    assessRisk.mockResolvedValue({ level: "safe", reasons: [] });
    expect(await runCommand("Restart-Computer", { level: "danger", reasons: ["catalog says so"] })).toBe(false);
    expect(shellRun).not.toHaveBeenCalled();
  });

  it("runs safe commands straight away", async () => {
    assessRisk.mockResolvedValue({ level: "safe", reasons: [] });
    expect(await runCommand("git status", null)).toBe(true);
    expect(shellRun).toHaveBeenCalledWith("git status");
  });
});

describe("subscribeAll", () => {
  it("reports failed subscriptions and releases late ones after cleanup", async () => {
    const onError = vi.fn();
    const lateUnlisten = vi.fn();
    let resolveLate!: (unlisten: () => void) => void;
    const late = new Promise<() => void>((resolve) => (resolveLate = resolve));
    const cleanup = subscribeAll([Promise.reject(new Error("no events")), late], onError);
    await Promise.resolve();
    cleanup();
    resolveLate(lateUnlisten);
    await late;
    await Promise.resolve();
    expect(onError).toHaveBeenCalledOnce();
    expect(lateUnlisten).toHaveBeenCalledOnce();
  });
});
