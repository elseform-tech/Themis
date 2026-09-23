import "./StatusDot.css";

export type StatusKind = "running" | "awaiting" | "done" | "failed" | "queued";

export interface StatusDotProps {
  status: StatusKind;
  label?: string;
}

export function StatusDot({ status, label }: StatusDotProps) {
  return (
    <span className="themis-status" title={label ?? status}>
      <span
        aria-hidden="true"
        className={`themis-status-dot themis-status-dot--${status}`}
      />
      {label !== undefined && label !== "" && (
        <span className="themis-status-label">{label}</span>
      )}
    </span>
  );
}
