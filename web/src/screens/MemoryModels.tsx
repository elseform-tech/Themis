import { useEffect, useState } from "react";
import { getMemorySettings, updateMemorySettings } from "../lib/tauri";
import type { GoModel } from "../lib/types";
import { describeError, useSettingsChanges } from "../state/store";
import { SettingsGroup } from "./SettingsGroup";

/** Fewest memory models `memory.models` may list. */
const MIN_MODELS = 3;
/** Most memory models `memory.models` may list. */
const MAX_MODELS = 8;

const chevron = (up: boolean) => <svg viewBox="0 0 24 24" aria-hidden="true"><path d={up ? "m6 15 6-6 6 6" : "m6 9 6 6 6-6"} /></svg>;

/** Settings › Models › Memory models: the models that keep memory, tried in order. Each change
 *  writes only `memory.models` in the user runtime.jsonc, refused when the file changed since
 *  the list loaded; the list never drops below three. */
export function MemoryModels({ models }: { models: GoModel[] }) {
  const [view, setView] = useState<{ models: string[]; budget: number; revision: string } | null>(null);
  const [error, setError] = useState("");
  const [saving, setSaving] = useState(false);
  const load = () => getMemorySettings()
    .then(loaded => setView({ models: loaded.settings.models, budget: loaded.settings.daily_budget_usd, revision: loaded.revision }))
    .catch(reason => setError(describeError(reason)));
  const changes = useSettingsChanges("runtime");
  useEffect(() => { void load(); }, [changes]);
  if (!view) return error ? <p role="alert" className="themis-form-error">{error}</p> : null;

  const write = async (next: string[]) => {
    setError(""); setSaving(true);
    try {
      const saved = await updateMemorySettings({ models: next }, view.revision);
      setView({ models: saved.settings.models, budget: saved.settings.daily_budget_usd, revision: saved.revision });
    } catch (reason) {
      setError(describeError(reason));
      void load();
    } finally { setSaving(false); }
  };
  const list = view.models;
  const move = (index: number, by: -1 | 1) => {
    const next = [...list];
    [next[index], next[index + by]] = [next[index + by]!, next[index]!];
    void write(next);
  };
  const addable = models.filter(model => !list.includes(model.id) && !model.unavailable_reason);

  return <>
    {error && <p role="alert" className="themis-form-error">{error}</p>}
    <SettingsGroup title="Memory models" hint={`They keep memory after each reply, in the background. The first that works does the job; the next takes over when one fails or is busy. All of them spend up to $${view.budget.toFixed(2)} a day together.`}>
      <ol className="themis-memory-models" aria-label="Memory models">
        {list.map((model, index) => (
          <li key={model} className="themis-settings-control">
            <span className="themis-memory-model-name">{model}</span>
            <span className="themis-memory-model-actions">
              <button type="button" aria-label={`Move ${model} up`} disabled={saving || index === 0} onClick={() => move(index, -1)}>{chevron(true)}</button>
              <button type="button" aria-label={`Move ${model} down`} disabled={saving || index === list.length - 1} onClick={() => move(index, 1)}>{chevron(false)}</button>
              <button type="button" aria-label={`Remove ${model}`} disabled={saving || list.length <= MIN_MODELS} onClick={() => void write(list.filter(other => other !== model))}>
                <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M6 6l12 12M18 6 6 18" /></svg>
              </button>
            </span>
          </li>
        ))}
      </ol>
      {list.length < MAX_MODELS && <label className="themis-settings-control">Add<span className="themis-settings-model-picker">
        <select aria-label="Add memory model" value="" disabled={saving || addable.length === 0} onChange={event => { if (event.target.value) void write([...list, event.target.value]); }}>
          <option value="">Add model</option>
          {addable.map(model => <option key={model.id} value={model.id}>{model.id}</option>)}
        </select>
      </span></label>}
    </SettingsGroup>
  </>;
}
