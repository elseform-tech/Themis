// A chief's routines on its profile (board ChiefProfile): each with its schedule, its instruction,
// an on/off switch and Remove. The person may pause or remove any routine; adding one is the
// chief's. Nothing shows while the chief has none.
import { useState } from "react";
import { Switch } from "../../components";
import { describeError } from "../../state/store";
import { CloseIcon } from "./SidePanel";
import { scheduleWords, type Routine } from "./team";

export interface RoutineControls {
  list: Routine[];
  setEnabled: (routineId: string, enabled: boolean) => Promise<void>;
  remove: (routineId: string) => Promise<void>;
}

export function Routines({ list, setEnabled, remove }: RoutineControls) {
  const [error, setError] = useState("");
  if (list.length === 0) return null;
  const act = (work: Promise<void>) => { setError(""); void work.catch(reason => setError(describeError(reason))); };
  return (
    <section className="themis-profile-block">
      <h2 className="themis-visions-label">Routines</h2>
      {error && <p role="alert" className="themis-form-error">{error}</p>}
      <ul className="themis-profile-box themis-routines" aria-label="Routines">
        {list.map(routine => {
          const when = scheduleWords(routine.schedule);
          return (
            <li key={routine.id} className="themis-routine">
              <svg className="themis-routine-icon" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><circle cx="12" cy="12" r="9" /><path d="M12 7v5h4" /></svg>
              <b>{when}</b>
              <span className="themis-routine-what">{routine.instruction}</span>
              <Switch role="switch" aria-label={when} checked={routine.enabled} onChange={event => act(setEnabled(routine.id, event.target.checked))} />
              <button type="button" className="themis-visions-panel-icon" aria-label={`Remove ${when}`} onClick={() => act(remove(routine.id))}><CloseIcon /></button>
            </li>
          );
        })}
      </ul>
    </section>
  );
}
