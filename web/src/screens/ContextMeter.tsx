import { useCallback, useRef, useState } from "react";
import { useDismiss } from "../lib/dismiss";
import type { ContextBudget } from "../lib/types";
import { usedPercent } from "../state/reducer";

type Part = { key: keyof ContextBudget["breakdown"]; label: string; color: string; optional?: boolean };
const PARTS: Part[] = [
  { key: "tool_activity", label: "Tool calls & results", color: "var(--themis-accent)" },
  // Only thinking models send their reasoning back; the row appears when there is some.
  { key: "reasoning", label: "Model reasoning", color: "var(--themis-success)", optional: true },
  { key: "conversation", label: "Messages", color: "var(--themis-info)" },
  { key: "attachments", label: "Attachments", color: "#c58af9" },
  { key: "checkpoint", label: "Checkpoint & task ledger", color: "var(--themis-gradient-3)" },
  { key: "system", label: "System & instructions", color: "#6f7480" },
];

export function formatTokens(tokens: number): string {
  return tokens >= 1000 ? `${(tokens / 1000).toFixed(1)}k` : String(tokens);
}

/** Byte-estimated context distribution with the runtime's own thresholds. */
export function ContextMeter({ budget, compacting = false, running = false }: { budget: ContextBudget | undefined; compacting?: boolean; running?: boolean }) {
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  useDismiss(root, open, useCallback(() => setOpen(false), []));
  if (budget === undefined) return null;
  const percent = usedPercent(budget);
  // The projection only matters while a run can still reach the threshold.
  const soon = budget.compaction_soon && running && !compacting;
  const tone = compacting || soon ? "warn" : "quiet";
  const share = (tokens: number) => `${budget.usable_tokens > 0 ? Math.min(100, tokens / budget.usable_tokens * 100) : 0}%`;
  const at = (tokens: number) => Math.min(100, Math.round(tokens / Math.max(1, budget.usable_tokens) * 100));
  const label = compacting ? "Compacting…" : soon ? `${percent}% · compacting soon` : `${percent}%`;
  const tokensOf = (part: Part) => budget.breakdown[part.key] ?? 0;
  const parts = PARTS.filter(part => !part.optional || tokensOf(part) > 0);
  return (
    <div className="themis-context" ref={root}>
      <button type="button" className={`themis-context-chip themis-context-chip--${tone}`} aria-expanded={open}
        aria-label={`Context ${percent}% full${soon ? ", close to auto-compact" : ""}`} onClick={() => setOpen(value => !value)}>
        <svg width="20" height="20" viewBox="0 0 36 36" aria-hidden="true">
          <circle cx="18" cy="18" r="14" fill="none" stroke="var(--themis-border)" strokeWidth="5" />
          <circle className={compacting ? "themis-context-spin" : undefined} cx="18" cy="18" r="14" fill="none" strokeWidth="5" strokeLinecap="round"
            stroke={tone === "warn" ? "var(--themis-gradient-2)" : "var(--themis-text-2)"} pathLength="100"
            strokeDasharray={`${compacting ? 30 : percent} 100`} transform="rotate(-90 18 18)" />
        </svg>
        <span>{label}</span>
      </button>
      {open && (
        <div role="dialog" aria-label="Context" className="themis-context-panel">
          <div className="themis-context-head">
            <strong>Context</strong>
            <span>≈{formatTokens(budget.used_tokens)} of {formatTokens(budget.usable_tokens)} usable · {formatTokens(budget.budget_tokens)} budget</span>
          </div>
          <div className="themis-context-bar" role="img" aria-label={`Context ${percent}% full: ${parts.map(part => `${part.label} ${formatTokens(tokensOf(part))}`).join(", ")}`}>
            {parts.map(part => <span key={part.key} style={{ width: share(tokensOf(part)), background: part.color }} />)}
            <i style={{ left: `${at(budget.maintenance_at_tokens)}%` }} />
            <i className="is-compact" style={{ left: `${at(budget.compaction_at_tokens)}%` }} />
          </div>
          <p className="themis-context-thresholds"><span><i aria-hidden="true" />Tidy-up at {at(budget.maintenance_at_tokens)}%</span><span><i className="is-compact" aria-hidden="true" />Auto-compact at {at(budget.compaction_at_tokens)}%</span></p>
          <ul className="themis-context-legend">
            {parts.map(part => <li key={part.key}><span style={{ background: part.color }} aria-hidden="true" />{part.label}<b>{formatTokens(tokensOf(part))}</b></li>)}
          </ul>
          <p className="themis-context-note">Held outside the bar: {formatTokens(budget.tool_definition_tokens)} tool definitions · {formatTokens(budget.output_reserve_tokens)} reply · {formatTokens(budget.safety_margin_tokens)} safety margin. Estimates (bytes ÷ 4).</p>
          {soon && <p className="themis-context-warn">Close to auto-compact. Themis will summarise older work into a checkpoint and task ledger; every original stays on disk.</p>}
        </div>
      )}
    </div>
  );
}

/** One-time heads-up shown above the composer when compaction is projected. */
export function CompactionNotice({ onDismiss }: { onDismiss: () => void }) {
  return (
    <div role="status" className="themis-compaction-notice">
      <span><strong>Close to auto-compact.</strong> Soon Themis will summarise older work. Nothing is deleted.</span>
      <button type="button" aria-label="Dismiss" onClick={onDismiss}>×</button>
    </div>
  );
}
