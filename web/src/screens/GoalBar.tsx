import { useEffect, useState } from "react";
import type { Goal } from "../lib/types";
import "./GoalBar.css";

const EXPANDED_KEY = "themis-goal-expanded";

/** Running time in milliseconds: counted time plus the current running stretch. */
export function goalElapsedMs(goal: Goal, now: number): number {
  return goal.active_ms + (goal.running_since === undefined ? 0 : Math.max(0, now - goal.running_since));
}

/** `H:MM:SS`. */
export function formatElapsed(ms: number): string {
  const total = Math.floor(ms / 1000);
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const seconds = total % 60;
  return `${hours}:${String(minutes).padStart(2, "0")}:${String(seconds).padStart(2, "0")}`;
}

/** Short state shown beside the elapsed time when the goal is not running. */
const STATUS_LABEL: Record<Goal["status"], string | null> = {
  running: null,
  paused: "Paused",
  waiting: "Waiting for your reply",
  stuck: "Paused · needs a change",
  quota: "Paused · quota used up",
  reached: null,
};

function readExpanded(): boolean {
  try { return window.localStorage.getItem(EXPANDED_KEY) === "1"; } catch { return false; }
}

interface GoalBarProps {
  goal: Goal;
  onEdit: (text: string) => Promise<void>;
  onPause: () => void;
  onResume: () => void;
  onRemove: () => void;
  /** Folds the composer's boxes to icons; absent when another box offers it. */
  onFold?: () => void;
}

/** The chat's unfinished goal as one row of the composer: its sentence (click to read it
 *  in full), elapsed time, and icons for edit and pause/resume, with remove
 *  under "more". */
export function GoalBar({ goal, onEdit, onPause, onResume, onRemove, onFold }: GoalBarProps) {
  const [now, setNow] = useState(() => Date.now());
  const [expanded, setExpanded] = useState(readExpanded);
  const [draft, setDraft] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [menu, setMenu] = useState(false);
  const ticking = goal.status === "running" && goal.running_since !== undefined;
  useEffect(() => {
    if (!ticking) return;
    setNow(Date.now());
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [ticking]);
  function toggle() {
    setExpanded(previous => {
      try { window.localStorage.setItem(EXPANDED_KEY, previous ? "0" : "1"); } catch { /* per-viewer convenience only */ }
      return !previous;
    });
  }
  async function save() {
    if (draft === null || draft.trim() === "" || saving) return;
    setSaving(true);
    try { await onEdit(draft.trim()); setDraft(null); } finally { setSaving(false); }
  }
  const label = goal.status === "paused" && goal.reason ? `Paused · ${goal.reason}` : STATUS_LABEL[goal.status];
  const paused = goal.status === "paused" || goal.status === "stuck" || goal.status === "quota";
  const editable = goal.status !== "waiting";
  const elapsed = formatElapsed(goalElapsedMs(goal, now));
  // A reached goal leaves the composer; its note stays in the conversation.
  if (goal.status === "reached") return null;

  if (draft !== null) {
    return (
      <form className="themis-goal-bar is-editing" aria-label="Edit goal" onSubmit={event => { event.preventDefault(); void save(); }}>
        <GoalMark />
        <label className="themis-sr-only" htmlFor="themis-goal-text">Goal</label>
        <textarea id="themis-goal-text" className="themis-goal-input" rows={2} value={draft} autoFocus maxLength={2000}
          onChange={event => setDraft(event.target.value)}
          onKeyDown={event => {
            if (event.key === "Escape") { event.preventDefault(); setDraft(null); }
            if (event.key === "Enter" && !event.shiftKey && !event.nativeEvent.isComposing) { event.preventDefault(); void save(); }
          }} />
        <button type="button" className="themis-goal-icon themis-icon-tip" data-tooltip="Discard edits" aria-label="Discard edits" onClick={() => setDraft(null)}>
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" aria-hidden="true"><path d="M6 6l12 12M18 6 6 18" /></svg>
        </button>
        <button type="submit" className="themis-goal-icon is-primary themis-icon-tip" data-tooltip="Save · applies next turn" aria-label="Save goal, applies next turn" disabled={saving || draft.trim() === ""}>
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.4" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="M5 12l5 5 9-10" /></svg>
        </button>
      </form>
    );
  }

  return (
    <div role="region" aria-label="Goal" className={`themis-goal-bar${expanded ? " is-expanded" : ""}`} data-status={goal.status}>
      <GoalMark />
      <button type="button" className="themis-goal-text" aria-expanded={expanded} title={expanded ? "Show less" : goal.text} onClick={toggle}>{goal.text}</button>
      <span className="themis-goal-meta">
        {label && <span className="themis-goal-state">{label}</span>}
        <span className="themis-goal-elapsed themis-icon-tip" data-tooltip="Elapsed" tabIndex={0} aria-label={`Elapsed ${elapsed}`}>
          <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" aria-hidden="true"><circle cx="12" cy="12" r="8" /><path d="M12 8v4l3 2" /></svg>
          {elapsed}
        </span>
      </span>
      {editable && (
        <button type="button" className="themis-goal-icon themis-icon-tip" data-tooltip="Edit goal" aria-label="Edit goal" onClick={() => setDraft(goal.text)}>
          <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="M4 20h4L19 9l-4-4L4 16Z" /><path d="M13.5 6.5l4 4" /></svg>
        </button>
      )}
      {goal.status === "running" && (
        <button type="button" className="themis-goal-icon themis-icon-tip" data-tooltip="Pause after this turn" aria-label="Pause goal after this turn" onClick={onPause}>
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.2" strokeLinecap="round" aria-hidden="true"><path d="M9 6v12M15 6v12" /></svg>
        </button>
      )}
      {paused && (
        <button type="button" className="themis-goal-icon is-primary themis-icon-tip" data-tooltip="Resume" aria-label="Resume goal" onClick={onResume}>
          <svg width="13" height="13" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true"><path d="M7 5v14l12-7Z" /></svg>
        </button>
      )}
      <span className="themis-goal-more">
        <button type="button" className="themis-goal-icon themis-icon-tip" data-tooltip="More" aria-label="More goal actions" aria-haspopup="menu" aria-expanded={menu} onClick={() => setMenu(open => !open)}>
          <svg width="14" height="14" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true"><circle cx="5" cy="12" r="1.7" /><circle cx="12" cy="12" r="1.7" /><circle cx="19" cy="12" r="1.7" /></svg>
        </button>
        {menu && (
          <span className="themis-goal-menu" role="menu"
            // WebKit moves focus off a button on mouse down, which would close the menu before the click lands.
            onMouseDown={event => event.preventDefault()}
            onKeyDown={event => { if (event.key === "Escape") { event.stopPropagation(); setMenu(false); } }}>
            <button type="button" role="menuitem" autoFocus onClick={() => { setMenu(false); onRemove(); }} onBlur={event => { if (!event.currentTarget.parentElement?.contains(event.relatedTarget as Node | null)) setMenu(false); }}>Remove goal</button>
          </span>
        )}
      </span>
      {onFold && (
        <button type="button" className="themis-goal-icon themis-icon-tip" data-tooltip="Fold to icons" aria-label="Fold to icons" onClick={onFold}>
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" aria-hidden="true"><path d="m6 9 6 6 6-6" /></svg>
        </button>
      )}
    </div>
  );
}

function GoalMark() {
  return (
    <svg className="themis-goal-mark" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" aria-hidden="true">
      <circle cx="12" cy="12" r="8" /><circle cx="12" cy="12" r="4" /><circle cx="12" cy="12" r="0.6" fill="currentColor" />
    </svg>
  );
}
