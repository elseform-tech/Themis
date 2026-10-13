// Background shell tasks in the chat's task feed: grouping and row text.
// Pure logic; BackgroundTaskRow renders it.
import type { ThreadTaskBackground, ThreadTasks } from "../lib/types";

export interface BackgroundGroups {
  attention: ThreadTaskBackground[];
  running: ThreadTaskBackground[];
  earlier: ThreadTaskBackground[];
}

/** A task needs attention when the server lost it or it ended abnormally. */
export function needsAttention(task: ThreadTaskBackground): boolean {
  if (task.state === "lost" || task.state === "timed_out") return true;
  return task.state === "exited" && task.exit_code !== 0;
}

export function groupBackground(tasks: ThreadTasks | undefined): BackgroundGroups {
  const groups: BackgroundGroups = { attention: [], running: [], earlier: [] };
  for (const task of [...(tasks?.background_tasks ?? [])].reverse()) {
    if (task.state === "running") groups.running.push(task);
    else if (needsAttention(task)) groups.attention.push(task);
    else groups.earlier.push(task);
  }
  return groups;
}

/** Counts that feed the header indicator and the Tasks tab. */
export function countBackground(tasks: ThreadTasks | undefined): { attention: number; running: number } {
  const groups = groupBackground(tasks);
  return { attention: groups.attention.length, running: groups.running.length };
}

const PORT = /(?:localhost|127\.0\.0\.1|0\.0\.0\.0|\[::1?\]):(\d{2,5})\b/;

/** First local port mentioned in the recent output, if any. */
export function detectPort(lines: string[]): string | null {
  for (let index = lines.length - 1; index >= 0; index -= 1) {
    const match = PORT.exec(lines[index]);
    if (match) return match[1];
  }
  const serving = /\bport (\d{2,5})\b/i;
  for (let index = lines.length - 1; index >= 0; index -= 1) {
    const match = serving.exec(lines[index]);
    if (match) return match[1];
  }
  return null;
}

export function clock(iso: string): string {
  const date = new Date(iso);
  return Number.isNaN(date.getTime()) ? iso : date.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit", hour12: false });
}

export function stateLabel(task: ThreadTaskBackground): string {
  switch (task.state) {
    case "running": return "Running";
    case "stopped": return "Stopped";
    case "timed_out": return "Timed out";
    case "lost": return "Lost";
    case "exited": return task.exit_code === 0 ? "Finished" : `Exited with code ${task.exit_code ?? "?"}`;
  }
}

/** "Started 14:05 by agent · ends when this chat closes · port 1420" and the finished variants. */
export function backgroundMeta(task: ThreadTaskBackground): string {
  const parts = [`Started ${clock(task.started_at)} by agent`];
  if (task.state === "running") {
    parts.push("ends when this chat closes");
    const port = detectPort(task.last_lines);
    if (port) parts.push(`port ${port}`);
  } else if (task.state === "lost") {
    parts.push("lost when the Themis server restarted; the program may still be running");
  } else {
    parts.push(`${stateLabel(task)}${task.ended_at ? ` ${clock(task.ended_at)}` : ""}`);
  }
  return parts.join(" · ");
}
