import { useRef } from "react";
import { insertCommand, runConfirmed } from "../commands";
import { useAppStore } from "../state/store";
import { CommandTape } from "./common/CommandTape";
import { Dialog } from "./common/Dialog";
import { RiskBadge } from "./common/RiskBadge";

/** Second step before running a command assessed as dangerous. */
export function RunConfirmDialog() {
  const pending = useAppStore((s) => s.pendingRun);
  const setPendingRun = useAppStore((s) => s.setPendingRun);
  const cancelRef = useRef<HTMLButtonElement>(null);
  if (!pending) return null;

  const cancel = () => setPendingRun(null);
  return (
    <Dialog
      narrow
      title="Run this command?"
      onClose={cancel}
      initialFocus={cancelRef}
      footer={
        <>
          <button ref={cancelRef} type="button" className="button" onClick={cancel}>
            Don't run it
          </button>
          <button
            type="button"
            className="button"
            onClick={() => {
              setPendingRun(null);
              void insertCommand(pending.command);
            }}
          >
            Put on command line
          </button>
          <button
            type="button"
            className="button danger"
            onClick={() => {
              setPendingRun(null);
              void runConfirmed(pending.command);
            }}
          >
            Run anyway
          </button>
        </>
      }
    >
      <CommandTape command={pending.command} block />
      <RiskBadge risk={pending.risk} showReasons />
      <p className="muted">If you're not sure, put it on the command line instead, read it there, and only press Enter when you're happy.</p>
    </Dialog>
  );
}
