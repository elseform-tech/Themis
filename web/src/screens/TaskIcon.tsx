import type { TaskKind } from "./threadTasks";
import "./TaskIcon.css";

/** A task's status glyph in the Workspace Tasks rows. */
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
