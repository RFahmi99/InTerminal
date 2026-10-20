import type { RiskReport } from "../../ipc/types";
import { Icon } from "./Icon";

const LABELS = { safe: "Looks safe", caution: "Caution", danger: "Danger" } as const;

/** Risk level as text plus icon (never colour alone), with reasons optionally listed. */
export function RiskBadge({ risk, showReasons = false }: { risk: RiskReport; showReasons?: boolean }) {
  return (
    <div>
      <span className={`risk ${risk.level}`}>
        <Icon name={risk.level === "safe" ? "check" : "warning"} />
        {LABELS[risk.level]}
      </span>
      {showReasons && risk.reasons.length > 0 && (
        <ul className="risk-reasons">
          {risk.reasons.map((reason) => (
            <li key={reason}>{reason}</li>
          ))}
        </ul>
      )}
    </div>
  );
}
