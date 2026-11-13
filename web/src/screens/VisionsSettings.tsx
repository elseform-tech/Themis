import { useCallback, useEffect, useRef, useState } from "react";
import { Switch } from "../components";
import { ADMIN_VISION_ID, getVisionsSettings, pauseVision, updateVisionsSettings, type VisionsSettings, type VisionsSettingsView } from "../lib/visions";
import { describeError, useApp, useSettingsChanges } from "../state/store";
import { SettingsGroup } from "./SettingsGroup";
import { AUTONOMY_STOPS, DECISIONS, LEVELS, presetDecides, presetRows } from "./visions/autonomy";
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

/** One kind of decision: who makes it. Editable only under Custom; the last two never. */
function DecisionRow({ decision, settings, onSet }: { decision: (typeof DECISIONS)[number]; settings: VisionsSettings; onSet: (id: string, decides: boolean) => void }) {
  let answer;
  if (decision.rank === null) {
    answer = <span className="themis-visions-locked"><LockIcon />Always asks you</span>;
  } else if (settings.autonomy === "custom") {
    const decides = settings.custom[decision.id] === true;
    answer = <span className="themis-visions-choice" role="radiogroup" aria-label={decision.label}>
      <button type="button" role="radio" aria-checked={!decides} onClick={() => { if (decides) onSet(decision.id, false); }}>Asks you</button>
      <button type="button" role="radio" aria-checked={decides} onClick={() => { if (!decides) onSet(decision.id, true); }}>Decides</button>
    </span>;
  } else {
    answer = presetDecides(settings.autonomy, decision)
      ? <span className="themis-visions-decides">Decides</span>
      : <span className="themis-visions-asks">Asks you</span>;
  }
  return <div className="themis-visions-decision"><span>{decision.label}</span>{answer}</div>;
}

/** The five-stop slider and what the chosen level means, row by row (canvas VisionsSettings). A
 *  drag shows where it is and saves once, where it is let go. Custom starts from the rows of the
 *  level it came from, so nothing changes until a row is edited. */
function AutonomyCard({ settings, patch }: { settings: VisionsSettings; patch: (write: Patch) => void }) {
  const [dragged, setDragged] = useState<number | null>(null);
  const level = settings.autonomy;
  const shown = dragged === null ? level : AUTONOMY_STOPS[dragged]!;
  const commit = () => {
    if (dragged === null) return;
    const next = AUTONOMY_STOPS[dragged]!;
    setDragged(null);
    patch(latest => {
      if (next === latest.autonomy) return null;
      return next === "custom" ? { autonomy: next, custom: presetRows(latest.autonomy) } : { autonomy: next };
    });
  };
  return <section className="themis-settings-card themis-visions-autonomy" aria-label="Autonomy">
    <div className="themis-visions-autonomy-head"><label htmlFor="visions-autonomy">Autonomy</label></div>
    <input id="visions-autonomy" type="range" min={1} max={5} step={1} value={AUTONOMY_STOPS.indexOf(shown) + 1} aria-valuetext={LEVELS[shown]}
      onChange={event => setDragged(Number(event.target.value) - 1)} onPointerUp={commit} onKeyUp={commit} onBlur={commit} />
    <div className="themis-visions-stops" aria-hidden="true">
      {AUTONOMY_STOPS.map(stop => <span key={stop} className={stop === shown ? "is-current" : undefined}>{LEVELS[stop]}</span>)}
    </div>
    <div className="themis-visions-decisions">
      {DECISIONS.map(decision => <DecisionRow key={decision.id} decision={decision} settings={settings} onSet={(id, decides) => patch(latest => ({ custom: { ...latest.custom, [id]: decides } }))} />)}
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
  // Atlas keeps running, so the person can still ask it in # hq.
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
    <AutonomyCard settings={view.settings} patch={patch} />
    <LimitsGroup settings={view.settings} patch={patch} onError={setError} />
  </>;
}
