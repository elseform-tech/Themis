import { useState } from "react";
import { getBackgroundTaskLog } from "../lib/tauri";
import type { BackgroundTaskLog, ThreadTaskBackground } from "../lib/types";
import { describeError } from "../state/store";
import { TaskIcon } from "./TaskIndicator";
import { backgroundMeta, needsAttention } from "./backgroundTasks";

/** Whole bounded log: at most the server's 64 KiB window, read in 16 KiB pages. */
const LOG_PAGE_BYTES = 16 * 1024;
const MAX_LOG_PAGES = 5;

async function loadLog(threadId: string, taskId: string): Promise<string> {
  let text = "";
  let offset = 0;
  let notice = "";
  for (let page = 0; page < MAX_LOG_PAGES; page += 1) {
    const chunk: BackgroundTaskLog = await getBackgroundTaskLog(threadId, taskId, offset, LOG_PAGE_BYTES);
    if (page === 0 && chunk.oldest_retained_offset > 0) notice = `[Earlier output rotated out; showing the last ${chunk.total_bytes - chunk.oldest_retained_offset} of ${chunk.total_bytes} bytes]\n`;
    text += chunk.text;
    if (chunk.next_offset >= chunk.total_bytes || chunk.next_offset <= offset) break;
    offset = chunk.next_offset;
  }
  return notice + text;
}

interface Props {
  threadId: string;
  task: ThreadTaskBackground;
  busy: boolean;
  onStop: (task: ThreadTaskBackground) => void;
}

/** One background shell task: Stop (while running) and a read-only bounded log. */
export function BackgroundTaskRow({ threadId, task, busy, onStop }: Props) {
  const [log, setLog] = useState<string | null>(null);
  const [error, setError] = useState("");
  const running = task.state === "running";
  const icon = running ? "running" : needsAttention(task) ? "attention" : "finished";
  async function toggleLog() {
    if (log !== null) { setLog(null); return; }
    setError("");
    try { setLog(await loadLog(threadId, task.id)); } catch (reason) { setError(describeError(reason)); }
  }
  return (
    <li className="themis-task-row themis-background-row">
      <span className={`themis-task-row-icon themis-task-row-icon--${icon}`} aria-hidden="true"><TaskIcon kind={icon} /></span>
      <span className="themis-task-row-text">
        <span className="themis-workspace-list-title"><code className="themis-background-command" title={task.command}>{task.command}</code><em>Background task</em></span>
        <span className="themis-workspace-list-meta">{backgroundMeta(task)}</span>
        {task.last_lines.length > 0 && <pre className="themis-background-tail" aria-label={`Latest output of ${task.command}`}>{task.last_lines.join("\n")}</pre>}
        {error && <span className="themis-workspace-note" role="alert">{error}</span>}
        {log !== null && <pre className="themis-background-log" aria-label={`Full log of ${task.command}`}>{log || "No output yet."}</pre>}
      </span>
      <span className="themis-task-row-actions">
        {running && <button type="button" aria-label={`Stop ${task.command}`} disabled={busy} onClick={() => onStop(task)}>Stop</button>}
        {(!task.last_lines_complete || log !== null) && <button type="button" aria-label={`${log === null ? "Show" : "Hide"} full log of ${task.command}`} aria-expanded={log !== null} onClick={() => void toggleLog()}>{log === null ? "Full log" : "Hide log"}</button>}
      </span>
    </li>
  );
}
