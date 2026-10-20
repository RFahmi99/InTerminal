// The two ways any proposed command reaches the shell. Both are explicit
// user actions; dangerous commands additionally need a confirmation.

import { announce } from "./a11y/announcer";
import { focusRegion } from "./focusRegistry";
import { api } from "./ipc/api";
import type { RiskLevel, RiskReport } from "./ipc/types";
import { notifyFailure } from "./notify";
import { useAppStore } from "./state/store";

const PLACEHOLDER = /<[a-z0-9-]+>/;

export function hasPlaceholders(command: string): boolean {
  return PLACEHOLDER.test(command);
}

const ORDER: RiskLevel[] = ["safe", "caution", "danger"];

/** The more severe of two reports, with both sets of reasons. */
export function combineRisk(a: RiskReport, b: RiskReport): RiskReport {
  const level = ORDER[Math.max(ORDER.indexOf(a.level), ORDER.indexOf(b.level))];
  return { level, reasons: Array.from(new Set([...a.reasons, ...b.reasons])) };
}

/** Types the command at the prompt for the user to review and run themselves. */
export async function insertCommand(command: string): Promise<boolean> {
  try {
    await api.shellInsert(command);
    focusRegion("terminal");
    announce("The command is on the command line. Check it, then press Enter to run it.");
    return true;
  } catch (error) {
    notifyFailure(error);
    return false;
  }
}

/**
 * Runs immediately, unless the command is dangerous, which asks first. Risk is
 * always re-assessed here, on the exact text being run, so an edit made after
 * a stale assessment (or a missing one) can never skip the confirmation. If
 * the assessment itself fails, the command is treated as dangerous.
 */
export async function runCommand(command: string, knownRisk: RiskReport | null): Promise<boolean> {
  let risk: RiskReport;
  try {
    const fresh = await api.assessRisk(command);
    risk = knownRisk ? combineRisk(fresh, knownRisk) : fresh;
  } catch {
    risk = { level: "danger", reasons: ["VCE couldn't check how risky this command is, so it asks before running it."] };
  }
  if (risk.level === "danger") {
    useAppStore.getState().setPendingRun({ command, risk });
    return false;
  }
  return runConfirmed(command);
}

export async function runConfirmed(command: string): Promise<boolean> {
  try {
    await api.shellRun(command);
    focusRegion("terminal");
    return true;
  } catch (error) {
    notifyFailure(error);
    return false;
  }
}

export async function copyCommand(command: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(command);
    announce("Command copied.");
  } catch (error) {
    notifyFailure(error, "Couldn't copy the command");
  }
}
