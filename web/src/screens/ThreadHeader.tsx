import type { GoModel, ThreadInfo } from "../lib/types";
import { TaskIndicator } from "./TaskIndicator";
import type { TaskCounts } from "./threadTasks";
// Official OpenCode theme assets: https://github.com/anomalyco/opencode/tree/dev/packages/console/app/src/asset
import opencodeLogoDark from "../assets/opencode-logo-dark.svg";
import opencodeLogoLight from "../assets/opencode-logo-light.svg";

interface ThreadHeaderProps {
  thread: ThreadInfo;
  models: GoModel[];
  effortLevels: string[];
  effort: string;
  busy: boolean;
  switching: boolean;
  onProviderChange: (model: string) => void;
  onEffortChange: (effort: string) => void;
  changedFiles?: number;
  reviewOpen: boolean;
  onToggleReview: () => void;
  taskCounts?: TaskCounts;
  onOpenTasks?: () => void;
}

/** Model and effort live in one header pill; review opens as a side panel. */
export function ThreadHeader({ thread, models, effortLevels, effort, busy, switching, onProviderChange, onEffortChange, changedFiles, reviewOpen, onToggleReview, taskCounts, onOpenTasks }: ThreadHeaderProps) {
  const choices = ["", ...effortLevels];
  const effortIndex = Math.max(0, choices.indexOf(effort));
  return (
    <header className="themis-thread-head">
      <details className="themis-model-pill">
        <summary aria-label={`Model ${thread.model || "not chosen"}, reasoning effort ${effort || "Default"}`}>
          <span className="themis-model-pill-brand">Themis</span>
          <span className="themis-model-pill-meta">{thread.model || "Choose a model"} · {effort || "Default"}</span>
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" aria-hidden="true"><path d="m6 9 6 6 6-6" /></svg>
        </summary>
        <div className="themis-model-panel">
          <div className="themis-composer-model-picker">
            <span className="themis-composer-model-provider" aria-label="OpenCode Go">
              <img className="themis-composer-logo-dark" src={opencodeLogoDark} alt="" />
              <img className="themis-composer-logo-light" src={opencodeLogoLight} alt="" />
              <span>Go</span>
            </span>
            <select aria-label="Model" title="Model" value={thread.model} disabled={busy || switching} onChange={event => onProviderChange(event.target.value)}>
              {[...new Set([thread.model, ...models.map(model => model.id)])].map(model => (
                <option key={model} value={model}>{model || "Choose a Go model"}</option>
              ))}
            </select>
          </div>
          <div className="themis-composer-effort-panel" title="Reasoning effort">
            <label htmlFor="themis-effort">Reasoning effort <strong>{effort || "Default"}</strong></label>
            <input id="themis-effort" aria-label="Reasoning effort" type="range" min="0" max={Math.max(1, effortLevels.length)} step="1"
              value={effortIndex} disabled={busy || effortLevels.length === 0}
              aria-valuetext={effort || "Default"}
              onChange={event => onEffortChange(choices[Number(event.target.value)] ?? "")} />
            <div className="themis-composer-effort-ends"><span>Default</span><span>{effortLevels[effortLevels.length - 1] || "No catalog levels"}</span></div>
          </div>
          <p className="themis-model-note">Changes apply to the next run.</p>
        </div>
      </details>
      <span className="themis-thread-head-spacer" />
      {taskCounts && onOpenTasks && <TaskIndicator counts={taskCounts} onOpen={onOpenTasks} />}
      <button type="button" className={`themis-workspace-toggle${reviewOpen ? " is-open" : ""}`} aria-pressed={reviewOpen} title="Changes, files, branches, pull requests and this chat's tasks" onClick={onToggleReview}>
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" aria-hidden="true"><rect x="3" y="4" width="18" height="16" rx="3" /><path d="M14 4v16" /></svg>
        Workspace{changedFiles ? <span className="themis-review-count">{changedFiles} changed</span> : null}
      </button>
    </header>
  );
}
