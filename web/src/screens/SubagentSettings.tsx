import { useEffect, useState } from "react";
import { Switch } from "../components";
import { listGoModels, getSubagentSettings, updateSubagentSettings } from "../lib/tauri";
import type { GoModel, SubagentSettings } from "../lib/types";
import { describeError } from "../state/store";
import { SettingsGroup } from "./SettingsGroup";

const COUNTS = [1, 2, 3, 4, 5, 6];

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
  return <>
    {error && <p role="alert" className="themis-form-error">{error}</p>}
    <SettingsGroup>
      <label className="themis-settings-check">Allow subagents<Switch checked={settings.enabled} onChange={event => void patch({ enabled: event.target.checked })} /></label>
      <div className="themis-settings-control">
        <span id="subagents-max">Maximum subagents</span>
        <span className="themis-subagent-counts" role="radiogroup" aria-labelledby="subagents-max">
          {COUNTS.map(count => (
            <button key={count} type="button" role="radio" aria-checked={settings.max_parallel === count} disabled={!settings.enabled} onClick={() => void patch({ max_parallel: count })}>{count}</button>
          ))}
        </span>
      </div>
      <label className="themis-settings-control">Subagent model<span className="themis-settings-model-picker">
        <select aria-label="Subagent model" value={model} disabled={!settings.enabled} onChange={event => void patch({ model: event.target.value || null })}>
          <option value="">Chat model</option>
          {!known && <option value={model}>{model} (unavailable)</option>}
          {models.map(entry => <option key={entry.id} value={entry.id}>{entry.id}</option>)}
        </select>
      </span></label>
    </SettingsGroup>
  </>;
}
