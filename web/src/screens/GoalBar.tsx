import { useEffect, useLayoutEffect, useRef, useState } from "react";
import type { Goal } from "../lib/types";
import "./GoalBar.css";

/** The editor grows with its text up to this many lines, then scrolls. */
const MAX_EDIT_LINES = 10;

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
  blocked: "Blocked",
  quota: "Paused · quota used up",
  reached: "Complete",
};

interface GoalBarProps {
  goal: Goal;
  onEdit: (text: string) => Promise<void>;
  onPause: () => void;
  onResume: () => void;
  onRemove: () => void;
  /** Folds the composer's boxes to icons; absent when another box offers it. */
  onFold?: () => void;
}

/** The chat's goal as one line of the composer: its sentence (in full on hover; a click
 *  edits it), its state and elapsed time, and icons for edit and pause/resume, with remove
 *  under "more". What the person also asked and the evidence stay out of it: they are in the
 *  chat, the goal the agent reads, and the work's plan record. */
export function GoalBar({ goal, onEdit, onPause, onResume, onRemove, onFold }: GoalBarProps) {
  const [now, setNow] = useState(() => Date.now());
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
  async function save() {
    if (draft === null || draft.trim() === "" || saving) return;
    setSaving(true);
    try { await onEdit(draft.trim()); setDraft(null); } finally { setSaving(false); }
  }
  const label = (goal.status === "paused" || goal.status === "blocked") && goal.reason ? `${STATUS_LABEL[goal.status]} · ${goal.reason}` : STATUS_LABEL[goal.status];
  const paused = goal.status === "paused" || goal.status === "blocked" || goal.status === "quota";
  const complete = goal.status === "reached";
  const editable = goal.status !== "waiting" && !complete;
  const elapsed = formatElapsed(goalElapsedMs(goal, now));

  if (draft !== null) {
    return (
      <form className="themis-goal-bar is-editing" aria-label="Edit goal" onSubmit={event => { event.preventDefault(); void save(); }}>
        <GoalMark />
        <label className="themis-sr-only" htmlFor="themis-goal-text">Goal</label>
        <GoalEditor value={draft} onChange={setDraft}
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
    <div role="region" aria-label="Goal" className="themis-goal-bar" data-status={goal.status}>
      <GoalMark />
      <div className="themis-goal-body">
        {editable
          ? <button type="button" className="themis-goal-text" title={goal.text} onClick={() => setDraft(goal.text)}>{goal.text}</button>
          : <span className="themis-goal-text" title={goal.text}>{goal.text}</span>}
      </div>
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

/** The goal's text while it is edited: grows with what it holds, up to [`MAX_EDIT_LINES`]. */
function GoalEditor({ value, onChange, onKeyDown }: {
  value: string;
  onChange: (value: string) => void;
  onKeyDown: (event: React.KeyboardEvent<HTMLTextAreaElement>) => void;
}) {
  const field = useRef<HTMLTextAreaElement>(null);
  useLayoutEffect(() => {
    const box = field.current;
    if (!box) return;
    box.style.height = "auto";
    const line = parseFloat(window.getComputedStyle(box).lineHeight) || 20;
    box.style.height = `${Math.min(box.scrollHeight, line * MAX_EDIT_LINES + 12)}px`;
  }, [value]);
  return (
    <textarea ref={field} id="themis-goal-text" className="themis-goal-input" rows={1} value={value} autoFocus maxLength={2000}
      onFocus={event => event.currentTarget.setSelectionRange(event.currentTarget.value.length, event.currentTarget.value.length)}
      onChange={event => onChange(event.target.value)} onKeyDown={onKeyDown} />
  );
}

function GoalMark() {
  return (
    <svg className="themis-goal-mark" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" aria-hidden="true">
      <circle cx="12" cy="12" r="8" /><circle cx="12" cy="12" r="4" /><circle cx="12" cy="12" r="0.6" fill="currentColor" />
    </svg>
  );
}
