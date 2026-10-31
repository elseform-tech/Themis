import { useEffect, useState } from "react";
import { Switch } from "../components";
import { listGoModels, getSubagentSettings, updateSubagentSettings } from "../lib/tauri";
import type { GoModel, SubagentSettings } from "../lib/types";
import { describeError } from "../state/store";
import { SettingsGroup } from "./SettingsGroup";

const COUNTS = [1, 2, 3, 4, 5, 6];
/** Most further models subagents may use. */
const MAX_MODELS = 3;

/** Settings › Subagents (board 28): field-level writes to the `subagents` block of the
 *  user runtime.jsonc, refused when the file changed since the page loaded. */
export function SubagentSettingsSection({ onSaved }: { onSaved?: () => void } = {}) {
  const [view, setView] = useState<{ settings: SubagentSettings; revision: string } | null>(null);
  const [models, setModels] = useState<GoModel[]>([]);
  const [error, setError] = useState("");
  const load = () => getSubagentSettings().then(setView).catch(reason => setError(describeError(reason)));
  useEffect(() => {
    void load();
    void listGoModels().then(setModels).catch(() => setModels([]));
  }, []);
  if (!view) return error ? <p role="alert" className="themis-form-error">{error}</p> : null;
  const { settings } = view;
  const patch = async (changes: Parameters<typeof updateSubagentSettings>[0]) => {
    setError("");
    try {
      setView(await updateSubagentSettings(changes, view.revision));
      onSaved?.();
    } catch (reason) {
      setError(describeError(reason));
      // A stale page shows the file's settings again.
      void load();
    }
  };
  const model = settings.model ?? "";
  const known = model === "" || models.some(entry => entry.id === model);
  const others = settings.models ?? [];
  const addable = models.filter(entry => entry.id !== model && !others.includes(entry.id));
  return <>
    {error && <p role="alert" className="themis-form-error">{error}</p>}
    <SettingsGroup>
      <label className="themis-settings-check">Allow subagents<Switch checked={settings.enabled} onChange={event => void patch({ enabled: event.target.checked })} /></label>
      <label className="themis-settings-check">Allow file writes<Switch checked={settings.allow_writes} disabled={!settings.enabled} onChange={event => void patch({ allow_writes: event.target.checked })} /></label>
      <label className="themis-settings-check">Allow connections<Switch checked={settings.allow_mcp} disabled={!settings.enabled} onChange={event => void patch({ allow_mcp: event.target.checked })} /></label>
      <div className="themis-settings-control">
        <span id="subagents-max">Maximum subagents</span>
        <span className="themis-subagent-counts" role="radiogroup" aria-labelledby="subagents-max">
          {COUNTS.map(count => (
            <button key={count} type="button" role="radio" aria-checked={settings.max_parallel === count} disabled={!settings.enabled} onClick={() => void patch({ max_parallel: count })}>{count}</button>
          ))}
        </span>
      </div>
      <label className="themis-settings-control">Default model<span className="themis-settings-model-picker">
        <select aria-label="Default model" value={model} disabled={!settings.enabled} onChange={event => void patch({ model: event.target.value || null })}>
          <option value="">Chat model</option>
          {!known && <option value={model}>{model} (unavailable)</option>}
          {models.map(entry => <option key={entry.id} value={entry.id}>{entry.id}</option>)}
        </select>
      </span></label>
      <div className="themis-settings-control">
        <span id="subagents-others">Allowed models</span>
        <span className="themis-subagent-models" role="group" aria-labelledby="subagents-others">
          {others.map(entry => (
            <span key={entry} className="themis-subagent-model">{entry}
              <button type="button" aria-label={`Remove ${entry}`} disabled={!settings.enabled} onClick={() => void patch({ models: others.filter(other => other !== entry) })}>
                <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M6 6l12 12M18 6 6 18" /></svg>
              </button>
            </span>
          ))}
          {others.length < MAX_MODELS && (
            <span className="themis-settings-model-picker">
              <select aria-label="Add model" value="" disabled={!settings.enabled || addable.length === 0} onChange={event => { if (event.target.value) void patch({ models: [...others, event.target.value] }); }}>
                <option value="">Add model</option>
                {addable.map(entry => <option key={entry.id} value={entry.id}>{entry.id}</option>)}
              </select>
            </span>
          )}
        </span>
      </div>
    </SettingsGroup>
  </>;
}
