import "./BudgetMeter.css";

export interface BudgetMeterProps {
  /** Fraction of budget used, clamped to [0, 1]. */
  fraction: number;
  label?: string;
}

export function BudgetMeter({ fraction, label }: BudgetMeterProps) {
  const clamped = Math.min(1, Math.max(0, fraction));
  const percent = Math.round(clamped * 100);
  return (
    <div className="themis-budget">
      {label !== undefined && label !== "" && (
        <div className="themis-budget-head">
          <span className="themis-budget-label">{label}</span>
          <span className="themis-budget-percent">{percent}%</span>
        </div>
      )}
      <div
        role="progressbar"
        aria-valuenow={percent}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-label={label ?? "Budget used"}
        className="themis-budget-track"
      >
        <div
          className="themis-budget-fill"
          style={{ width: `${percent}%` }}
        />
      </div>
    </div>
  );
}
