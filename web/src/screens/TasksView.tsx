import { useState } from "react";
import { EmptyState } from "../components";
import { setAutomationEnabled, stopBackgroundTask, stopThread } from "../lib/tauri";
import type { ThreadTaskBackground } from "../lib/types";
import { describeError, useApp } from "../state/store";
import { BackgroundTaskRow } from "./BackgroundTaskRow";
import { groupBackground } from "./backgroundTasks";
import { TaskIcon } from "./TaskIcon";
import { groupTasks, type TaskKind, type TaskRow } from "./threadTasks";

const SECTIONS: Array<["attention" | "running" | "scheduled" | "earlier", string]> = [
  ["attention", "Needs attention"], ["running", "Running"], ["scheduled", "Scheduled"], ["earlier", "Earlier"],
];

const iconKind = (row: TaskRow): TaskKind => row.failed ? "attention" : row.kind === "earlier" ? "finished" : row.kind;

/** This chat's runs and automations. Stop and Pause are the only actions; neither undoes work. */
export function TasksView({ threadId }: { threadId: string }) {
  const { state, dispatch } = useApp();
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState("");
  const groups = groupTasks(state.tasks[threadId], state.settings.automations_enabled);
  const background = groupBackground(state.tasks[threadId]);
  async function stopBackground(task: ThreadTaskBackground) {
    setBusy(`background:${task.id}`); setError("");
    try { await stopBackgroundTask(threadId, task.id); } catch (reason) { setError(describeError(reason)); }
    finally { setBusy(null); }
  }
  async function act(row: TaskRow, what: "stop" | "pause") {
    setBusy(`${row.key}:${what}`); setError("");
    try {
      if (what === "stop") await stopThread(threadId);
      else if (row.pause) dispatch({ type: "automation/updated", automation: await setAutomationEnabled(row.pause, false) });
    } catch (reason) { setError(describeError(reason)); }
    finally { setBusy(null); }
  }
  const backgroundIn = (group: string): ThreadTaskBackground[] => group === "attention" || group === "running" || group === "earlier" ? background[group] : [];
  const empty = SECTIONS.every(([group]) => groups[group].length === 0 && backgroundIn(group).length === 0);
  return (
    <div className="themis-tasks">
      {error && <p className="themis-workspace-note" role="alert">{error}</p>}
      {empty ? <EmptyState title="No tasks in this chat" hint="Runs, automations and background programs of this chat appear here." /> : SECTIONS.map(([group, title]) => groups[group].length + backgroundIn(group).length > 0 && (
        <section key={group} aria-label={title}>
          <h3 className="themis-tasks-heading">{title}</h3>
          <ul className="themis-workspace-list">
            {groups[group].map(row => {
              const icon = iconKind(row);
              return (
              <li key={row.key} className="themis-task-row">
                <span className={`themis-task-row-icon themis-task-row-icon--${icon}`} aria-hidden="true">
                  <TaskIcon kind={icon} />
                </span>
                <span className="themis-task-row-text">
                  <span className="themis-workspace-list-title">{row.name}<em>{row.tag}</em></span>
                  <span className="themis-workspace-list-meta">{row.meta}</span>
                </span>
                <span className="themis-task-row-actions">
                  {row.stop && <button type="button" aria-label={`Stop ${row.name}`} disabled={busy !== null} onClick={() => void act(row, "stop")}>Stop</button>}
                  {row.pause && <button type="button" aria-label={`Pause ${row.name}`} disabled={busy !== null} onClick={() => void act(row, "pause")}>Pause</button>}
                </span>
              </li>
              );
            })}
            {backgroundIn(group).map(task => <BackgroundTaskRow key={`background-${task.id}`} threadId={threadId} task={task} busy={busy !== null} onStop={task => void stopBackground(task)} />)}
          </ul>
        </section>
      ))}
      <p className="themis-workspace-note themis-tasks-footer">Shows this chat only. Stopping never undoes work already done.</p>
    </div>
  );
}
