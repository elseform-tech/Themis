import { useCallback, useEffect, useRef, useState } from "react";
import { Switch } from "../components";
import { ADMIN_VISION_ID, getVisionsSettings, pauseVision, updateVisionsSettings, type VisionsSettings, type VisionsSettingsView } from "../lib/visions";
import { describeError, useApp, useSettingsChanges } from "../state/store";
import { SettingsGroup } from "./SettingsGroup";
import { LevelSwitch } from "./visions/LevelSwitch";
import "./visions/VisionsPanels.css";

type Changes = Parameters<typeof updateVisionsSettings>[0];
/** A write, worked out from the settings as they are when it runs (after earlier writes). */
type Patch = (latest: VisionsSettings) => Changes | null;

/** Lowest and highest daily budget for all Visions, as the server allows. */
const MIN_BUDGET = 0.1;
const MAX_BUDGET = 100;

function LockIcon() {
  return <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><rect x="5" y="11" width="14" height="10" rx="2" /><path d="M8 11V7a4 4 0 0 1 8 0v4" /></svg>;
}

/** What a chief never decides itself, at either level (Rust `vision_decisions` leaves them out). */
const LOCKED = ["Anything that leaves this computer", "Deleting or replacing work", "Spending past the budget"];

/** The level of new channels (and of channels with none), and what always asks the person
 *  whatever the level (board TwoLevels). */
function LevelCard({ settings, patch }: { settings: VisionsSettings; patch: (write: Patch) => void }) {
  return <section className="themis-settings-group themis-visions-level-group">
    <div className="themis-settings-group-head"><h3 className="themis-settings-subtitle">Level for new channels</h3></div>
    <LevelSwitch label="Level for new channels" level={settings.level} onChange={next => patch(latest => latest.level === next ? null : { level: next })} />
    <div className="themis-settings-card">
      {LOCKED.map(label => <div key={label} className="themis-settings-control themis-visions-decision"><span>{label}</span><span className="themis-visions-locked"><LockIcon />Always asks you</span></div>)}
    </div>
  </section>;
}

/** The daily budget for all Visions together, and Pause all, which sets every Vision's pause
 *  but Atlas's. */
function LimitsGroup({ settings, patch, onError }: { settings: VisionsSettings; patch: (write: Patch) => void; onError: (message: string) => void }) {
  const { state } = useApp();
  const [budget, setBudget] = useState<string | null>(null);
  const saveBudget = () => {
    if (budget === null) return;
    const dollars = Number(budget);
    setBudget(null);
    if (budget.trim() === "" || !Number.isFinite(dollars) || dollars < MIN_BUDGET || dollars > MAX_BUDGET) {
      onError("Enter an amount from $0.10 to $100");
      return;
    }
    if (dollars !== settings.daily_budget_usd) patch(() => ({ daily_budget_usd: dollars }));
  };
  // Atlas keeps running, so the person can still ask it in # administration.
  const visions = state.visions.list.filter(vision => vision.id !== ADMIN_VISION_ID);
  const allPaused = visions.length > 0 && visions.every(vision => vision.paused);
  const pauseAll = async (paused: boolean) => {
    onError("");
    try {
      for (const vision of visions) if (vision.paused !== paused) await pauseVision(vision.id, paused);
    } catch (reason) { onError(describeError(reason)); }
  };
  return <SettingsGroup>
    <label className="themis-settings-control">Daily budget, all Visions<span className="themis-visions-dollars">$
      <input className="themis-settings-number" type="number" min={MIN_BUDGET} max={MAX_BUDGET} step={0.5} aria-label="Daily budget, all Visions" value={budget ?? String(settings.daily_budget_usd)}
        onChange={event => setBudget(event.target.value)} onBlur={saveBudget} onKeyDown={event => { if (event.key === "Enter") event.currentTarget.blur(); }} />
    </span></label>
    <label className="themis-settings-check">Pause all Visions<Switch checked={allPaused} disabled={visions.length === 0} onChange={event => void pauseAll(event.target.checked)} /></label>
  </SettingsGroup>;
}

/** A whole number saved when the box loses focus, within `min`..`max`. */
function CountField({ label, value, min, max, onSave, onError }: { label: string; value: number; min: number; max: number; onSave: (count: number) => void; onError: (message: string) => void }) {
  const [draft, setDraft] = useState<string | null>(null);
  const save = () => {
    if (draft === null) return;
    const count = Number(draft);
    setDraft(null);
    if (!Number.isInteger(count) || count < min || count > max) { onError(`Enter a number from ${min} to ${max}`); return; }
    if (count !== value) onSave(count);
  };
  return <label className="themis-settings-control">{label}
    <input className="themis-settings-number" type="number" min={min} max={max} step={1} aria-label={label} value={draft ?? String(value)}
      onChange={event => setDraft(event.target.value)} onBlur={save} onKeyDown={event => { if (event.key === "Enter") event.currentTarget.blur(); }} />
  </label>;
}

/** Teams: how many requests a channel sends a day, routines per chief, and quiet hours (routines
 *  and what agents start wait; your own messages always go through). */
function TeamsGroup({ settings, patch, onError }: { settings: VisionsSettings; patch: (write: Patch) => void; onError: (message: string) => void }) {
  const quiet = settings.quiet_hours;
  const setQuiet = (next: { start: string; end: string } | null) => {
    if (next && next.start === next.end) { onError("Quiet hours need two different times"); return; }
    patch(() => ({ quiet_hours: next }));
  };
  return <SettingsGroup>
    <CountField label="Requests between channels, a day" value={settings.requests_per_day} min={1} max={100} onError={onError} onSave={count => patch(() => ({ requests_per_day: count }))} />
    <CountField label="Routines per chief" value={settings.routines_per_chief} min={1} max={50} onError={onError} onSave={count => patch(() => ({ routines_per_chief: count }))} />
    <label className="themis-settings-check">Quiet hours<Switch checked={quiet !== null} onChange={event => setQuiet(event.target.checked ? { start: "22:00", end: "07:00" } : null)} /></label>
    {quiet && <div className="themis-settings-control">
      <input type="time" aria-label="Quiet from" value={quiet.start} onChange={event => event.target.value && setQuiet({ ...quiet, start: event.target.value })} />
      <input type="time" aria-label="Quiet until" value={quiet.end} onChange={event => event.target.value && setQuiet({ ...quiet, end: event.target.value })} />
    </div>}
  </SettingsGroup>;
}

/** Settings › Visions: field-level writes to the `visions` block of the user runtime.jsonc,
 *  refused when the file changed since the page loaded. Writes run one at a time, each naming
 *  the revision the one before it returned. */
export function VisionsSettingsSection({ onSaved }: { onSaved?: () => void } = {}) {
  const [view, setView] = useState<VisionsSettingsView | null>(null);
  const [error, setError] = useState("");
  const latest = useRef<VisionsSettingsView | null>(null);
  const queue = useRef<Promise<void>>(Promise.resolve());
  const show = (next: VisionsSettingsView) => { latest.current = next; setView(next); };
  const load = useCallback(() => getVisionsSettings()
    .then(next => { latest.current = next; setView(next); })
    .catch(reason => setError(describeError(reason))), []);
  const changes = useSettingsChanges("runtime");
  useEffect(() => { void load(); }, [changes, load]);
  if (!view) return error ? <p role="alert" className="themis-form-error">{error}</p> : null;
  const write = async (patch: Patch) => {
    const current = latest.current;
    const changes = current && patch(current.settings);
    if (!current || !changes) return;
    setError("");
    try {
      show(await updateVisionsSettings(changes, current.revision));
      onSaved?.();
    } catch (reason) {
      setError(describeError(reason));
      // A stale page shows the file's settings again.
      await load();
    }
  };
  const patch = (next: Patch) => { queue.current = queue.current.then(() => write(next)); };
  return <>
    {error && <p role="alert" className="themis-form-error">{error}</p>}
    {!view.can_run && <p role="status" className="themis-settings-hint">Visions can't run on this computer: it has no sandbox that can confine them.</p>}
    <LevelCard settings={view.settings} patch={patch} />
    <LimitsGroup settings={view.settings} patch={patch} onError={setError} />
    <TeamsGroup settings={view.settings} patch={patch} onError={setError} />
  </>;
}
