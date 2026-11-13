import { useEffect, useState } from "react";
import { Switch } from "../components";
import { getVisionsSettings, pauseVision, updateVisionsSettings, type VisionsSettings, type VisionsSettingsView } from "../lib/visions";
import { describeError, useApp, useSettingsChanges } from "../state/store";
import { SettingsGroup } from "./SettingsGroup";
import { AUTONOMY_STOPS, DECISIONS, LEVELS, presetDecides } from "./visions/autonomy";
import "./visions/VisionsPanels.css";

type Changes = Parameters<typeof updateVisionsSettings>[0];

function LockIcon() {
  return <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><rect x="5" y="11" width="14" height="10" rx="2" /><path d="M8 11V7a4 4 0 0 1 8 0v4" /></svg>;
}

/** One kind of decision: who makes it. Editable only under Custom; the last two never. */
function DecisionRow({ decision, settings, onSet }: { decision: (typeof DECISIONS)[number]; settings: VisionsSettings; onSet: (id: string, decides: boolean) => void }) {
  let answer;
  if (decision.rank === null) {
    answer = <span className="themis-visions-locked"><LockIcon />Always you</span>;
  } else if (settings.autonomy === "custom") {
    const decides = settings.custom[decision.id] === true;
    answer = <span className="themis-visions-choice" role="radiogroup" aria-label={decision.label}>
      <button type="button" role="radio" aria-checked={!decides} onClick={() => { if (decides) onSet(decision.id, false); }}>Asks you</button>
      <button type="button" role="radio" aria-checked={decides} onClick={() => { if (!decides) onSet(decision.id, true); }}>Vision decides</button>
    </span>;
  } else {
    answer = presetDecides(settings.autonomy, decision)
      ? <span className="themis-visions-decides">Vision decides</span>
      : <span className="themis-visions-asks">Asks you</span>;
  }
  return <div className="themis-visions-decision"><span>{decision.label}</span>{answer}</div>;
}

/** The five-stop slider and what the chosen level means, row by row (canvas VisionsSettings). */
function AutonomyCard({ settings, patch }: { settings: VisionsSettings; patch: (changes: Changes) => void }) {
  const level = settings.autonomy;
  const at = AUTONOMY_STOPS.indexOf(level);
  return <section className="themis-settings-card themis-visions-autonomy" aria-label="Autonomy">
    <div className="themis-visions-autonomy-head">
      <label htmlFor="visions-autonomy">Autonomy</label>
      <span>What a Vision decides when its chat asks something</span>
    </div>
    <input id="visions-autonomy" type="range" min={1} max={5} step={1} value={at + 1} aria-valuetext={LEVELS[level]}
      onChange={event => { const next = AUTONOMY_STOPS[Number(event.target.value) - 1]; if (next && next !== level) patch({ autonomy: next }); }} />
    <div className="themis-visions-stops" aria-hidden="true">
      {AUTONOMY_STOPS.map(stop => <span key={stop} className={stop === level ? "is-current" : undefined}>{LEVELS[stop]}</span>)}
    </div>
    <p className="themis-visions-meta">{level === "custom" ? "Custom: you choose each kind of decision. The last two stay yours." : `${LEVELS[level]}: shown for reference. Pick Custom to change single rows.`}</p>
    <div className="themis-visions-decisions">
      {DECISIONS.map(decision => <DecisionRow key={decision.id} decision={decision} settings={settings} onSet={(id, decides) => patch({ custom: { ...settings.custom, [id]: decides } })} />)}
    </div>
    <p className="themis-visions-meta">Approvals stay yours at every level. A Vision that is not sure how much a choice matters asks you, and its answers show in the channel so you can change them.</p>
  </section>;
}

/** The daily budget for all Visions together, and Pause all, which sets every Vision's pause. */
function LimitsGroup({ settings, patch, onError }: { settings: VisionsSettings; patch: (changes: Changes) => void; onError: (message: string) => void }) {
  const { state } = useApp();
  const [budget, setBudget] = useState<string | null>(null);
  const saveBudget = () => {
    if (budget === null) return;
    const dollars = Number(budget);
    setBudget(null);
    if (budget.trim() !== "" && Number.isFinite(dollars) && dollars >= 0 && dollars !== settings.daily_budget_usd) patch({ daily_budget_usd: dollars });
  };
  const visions = state.visions.list;
  const allPaused = visions.length > 0 && visions.every(vision => vision.paused);
  const pauseAll = async (paused: boolean) => {
    onError("");
    try {
      for (const vision of visions) if (vision.paused !== paused) await pauseVision(vision.id, paused);
    } catch (reason) { onError(describeError(reason)); }
  };
  return <SettingsGroup>
    <label className="themis-settings-control">Daily budget, all Visions<span className="themis-visions-dollars">$
      <input className="themis-settings-number" type="number" min={0} step={0.5} aria-label="Daily budget, all Visions" value={budget ?? String(settings.daily_budget_usd)}
        onChange={event => setBudget(event.target.value)} onBlur={saveBudget} onKeyDown={event => { if (event.key === "Enter") event.currentTarget.blur(); }} />
    </span></label>
    <label className="themis-settings-check">Pause all Visions<Switch checked={allPaused} disabled={visions.length === 0} onChange={event => void pauseAll(event.target.checked)} /></label>
  </SettingsGroup>;
}

/** Settings › Visions: field-level writes to the `visions` block of the user runtime.jsonc,
 *  refused when the file changed since the page loaded. */
export function VisionsSettingsSection({ onSaved }: { onSaved?: () => void } = {}) {
  const [view, setView] = useState<VisionsSettingsView | null>(null);
  const [error, setError] = useState("");
  const load = () => getVisionsSettings().then(setView).catch(reason => setError(describeError(reason)));
  const changes = useSettingsChanges("runtime");
  useEffect(() => { void load(); }, [changes]);
  if (!view) return error ? <p role="alert" className="themis-form-error">{error}</p> : null;
  const patch = async (changes: Changes) => {
    setError("");
    try {
      setView(await updateVisionsSettings(changes, view.revision));
      onSaved?.();
    } catch (reason) {
      setError(describeError(reason));
      // A stale page shows the file's settings again.
      void load();
    }
  };
  return <>
    {error && <p role="alert" className="themis-form-error">{error}</p>}
    <AutonomyCard settings={view.settings} patch={changes => void patch(changes)} />
    <LimitsGroup settings={view.settings} patch={changes => void patch(changes)} onError={setError} />
  </>;
}
