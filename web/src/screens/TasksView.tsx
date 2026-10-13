import { useState } from "react";
import { EmptyState } from "../components";
import { setAutomationEnabled, stopThread } from "../lib/tauri";
import { describeError, useApp } from "../state/store";
import { TaskIcon } from "./TaskIndicator";
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
  const groups = groupTasks(state.tasks[threadId]);
  async function act(row: TaskRow, what: "stop" | "pause") {
    setBusy(`${row.key}:${what}`); setError("");
    try {
      if (what === "stop") await stopThread(threadId);
      else if (row.pause) dispatch({ type: "automation/updated", automation: await setAutomationEnabled(row.pause, false) });
    } catch (reason) { setError(describeError(reason)); }
    finally { setBusy(null); }
  }
  const empty = SECTIONS.every(([group]) => groups[group].length === 0);
  return (
    <div className="themis-tasks">
      {error && <p className="themis-workspace-note" role="alert">{error}</p>}
      {empty ? <EmptyState title="No tasks in this chat" hint="Runs and automations that continue this chat appear here." /> : SECTIONS.map(([group, title]) => groups[group].length > 0 && (
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
          </ul>
        </section>
      ))}
      <p className="themis-workspace-note themis-tasks-footer">Shows this chat only. Stopping never undoes work already done.</p>
    </div>
  );
}
