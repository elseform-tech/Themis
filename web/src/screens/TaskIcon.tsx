import { describeCounts, TASK_KINDS, type TaskCounts, type TaskKind } from "./threadTasks";
import "./TaskIndicator.css";

const MAX_ICONS = 3;
const LABELS: Record<TaskKind, string> = { attention: "Needs attention", running: "Running", scheduled: "Scheduled", finished: "Finished" };

export function TaskIcon({ kind }: { kind: TaskKind }) {
  return (
    <svg viewBox="0 0 16 16" aria-hidden="true">
      {kind === "attention" && <><circle className="themis-task-fill" cx="8" cy="8" r="6.5" /><path d="M8 4.8v3.8M8 11.1v.1" /></>}
      {kind === "running" && <><circle className="themis-task-track" cx="8" cy="8" r="6" /><circle className="themis-task-arc" cx="8" cy="8" r="6" /></>}
      {kind === "scheduled" && <><circle cx="8" cy="8" r="6" /><path d="M8 4.8V8l2.2 1.4" /></>}
      {kind === "finished" && <path d="m4.2 8.4 2.5 2.4 5.1-5.3" />}
    </svg>
  );
}

/** Stacked icons for this chat's tasks; hidden when nothing is active. */
export function TaskIndicator({ counts, onOpen }: { counts: TaskCounts; onOpen: () => void }) {
  const kinds = TASK_KINDS.filter(kind => counts[kind] > 0);
  if (kinds.length === 0) return null;
  const hidden = kinds.slice(MAX_ICONS).reduce((sum, kind) => sum + counts[kind], 0);
  const summary = describeCounts(counts);
  return (
    <button type="button" className="themis-task-indicator" aria-label={`Tasks in this chat: ${summary}. Opens Workspace tasks`} title={`Tasks in this chat: ${summary}`} onClick={onOpen}>
      {kinds.slice(0, MAX_ICONS).map(kind => (
        <span key={kind} className={`themis-task-icon themis-task-icon--${kind}`} title={LABELS[kind]}><TaskIcon kind={kind} /></span>
      ))}
      {hidden > 0 && <span className="themis-task-more">+{hidden}</span>}
    </button>
  );
}
