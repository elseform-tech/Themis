// Tasks of the current chat: one feed from the backend, grouped for the header
// indicator and the Workspace Tasks tab. Pure logic; the hook only refreshes.
import { useEffect } from "react";
import { getThreadTasks } from "../lib/tauri";
import type { ThreadTaskAutomation, ThreadTaskRun, ThreadTasks } from "../lib/types";
import { useApp } from "../state/store";
import { scheduleLabel, shortTime } from "./Automations";
import { countBackground } from "./backgroundTasks";

export type TaskKind = "attention" | "running" | "scheduled" | "finished";
export const TASK_KINDS: TaskKind[] = ["attention", "running", "scheduled", "finished"];

export interface TaskRow {
  key: string;
  kind: TaskKind | "earlier";
  failed?: boolean;
  name: string;
  tag: string;
  meta: string;
  /** Stops the chat's current run. */
  stop?: boolean;
  /** Turns the automation off. */
  pause?: string;
}

export interface TaskGroups {
  attention: TaskRow[];
  running: TaskRow[];
  scheduled: TaskRow[];
  earlier: TaskRow[];
}

export type TaskCounts = Record<TaskKind, number>;

const automationTag = (automation: Pick<ThreadTaskAutomation, "origin">) =>
  automation.origin === "agent" ? "Automation · by agent" : "Automation · continues this chat";

function snippet(text: string): string {
  const line = text.replace(/\s+/g, " ").trim();
  return line.length > 90 ? `${line.slice(0, 89)}…` : line;
}

function runRow(run: ThreadTaskRun, kind: TaskRow["kind"], automations: ThreadTaskAutomation[]): TaskRow {
  const automation = automations.find(item => item.id === run.automation_id);
  return {
    key: `run-${run.review_id}`,
    kind,
    failed: !run.ok,
    name: run.name,
    tag: automation ? automationTag(automation) : "Automation",
    meta: [run.ok ? "Finished" : "Failed", shortTime(run.finished_at), snippet(run.summary.replace(/^failed:\s*/, ""))].filter(Boolean).join(" · "),
  };
}

/** Automations run only while the global Settings switch is on. */
const isScheduled = (item: ThreadTaskAutomation, schedulerOn: boolean) => item.enabled && schedulerOn;

export function groupTasks(tasks: ThreadTasks | undefined, schedulerOn = true): TaskGroups {
  const groups: TaskGroups = { attention: [], running: [], scheduled: [], earlier: [] };
  if (!tasks) return groups;
  const { run, automations, recent } = tasks;
  const live = automations.find(item => item.running);
  const liveRow = (kind: "attention" | "running", meta: string): TaskRow => ({
    key: live ? `automation-${live.id}` : "run",
    kind,
    name: live?.name ?? (kind === "attention" ? "Waiting for your approval" : "Chat run"),
    tag: live ? automationTag(live) : "Run",
    meta,
    stop: true,
    pause: live?.id,
  });
  if (run.awaiting_approval) groups.attention.push(liveRow("attention", "Waiting for your approval"));
  else if (run.running) groups.running.push(liveRow("running", "Running now"));
  for (const item of recent) {
    if (!item.ok && !item.seen) groups.attention.push(runRow(item, "attention", automations));
    else groups.earlier.push(runRow(item, "earlier", automations));
  }
  for (const item of automations) {
    if (item === live && (run.running || run.awaiting_approval)) continue;
    groups.scheduled.push({
      key: `automation-${item.id}`,
      kind: "scheduled",
      name: item.name,
      tag: automationTag(item),
      meta: !item.enabled
        ? "Paused · resume it in Automations"
        : !schedulerOn
          ? "Automations are off in Settings"
          : `${scheduleLabel(item)} · next ${shortTime(item.next_run_at)}${item.last_run_at ? ` · last ${shortTime(item.last_run_at)}` : ""}`,
      pause: item.enabled ? item.id : undefined,
    });
  }
  return groups;
}

export function countTasks(tasks: ThreadTasks | undefined, schedulerOn = true): TaskCounts {
  const groups = groupTasks(tasks, schedulerOn);
  const background = countBackground(tasks);
  return {
    attention: groups.attention.length + background.attention,
    running: groups.running.length + background.running,
    scheduled: tasks?.automations.filter(item => isScheduled(item, schedulerOn) && !(item.running && (tasks.run.running || tasks.run.awaiting_approval))).length ?? 0,
    finished: tasks?.recent.filter(item => item.ok && !item.seen).length ?? 0,
  };
}

/** "1 needs attention, 2 running, 1 scheduled", empty when nothing is active. */
export function describeCounts(counts: TaskCounts): string {
  const parts: string[] = [];
  if (counts.attention) parts.push(`${counts.attention} ${counts.attention === 1 ? "needs" : "need"} attention`);
  if (counts.running) parts.push(`${counts.running} running`);
  if (counts.scheduled) parts.push(`${counts.scheduled} scheduled`);
  if (counts.finished) parts.push(`${counts.finished} finished`);
  return parts.join(", ");
}

/** Keeps the current chat's feed fresh from the events the store already receives. */
export function useThreadTasksSync(threadId: string | null) {
  const { state, dispatch } = useApp();
  const awaiting = threadId !== null && state.approvals.some(approval => approval.thread_id === threadId);
  const running = threadId !== null && (state.running[threadId] ?? false);
  // Only tools that can change task state refresh the feed.
  const automationEdits = threadId === null ? 0 : (state.traces[threadId] ?? []).filter(entry => entry.tool === "manage_automations" && entry.ok !== undefined).length;
  const { automations, reviews } = state;
  useEffect(() => {
    if (threadId === null) return;
    let cancelled = false;
    // A failed refresh keeps the last feed on screen.
    getThreadTasks(threadId)
      .then(tasks => { if (!cancelled) dispatch({ type: "tasks/loaded", threadId, tasks }); })
      .catch(() => undefined);
    return () => { cancelled = true; };
  }, [dispatch, threadId, awaiting, running, automationEdits, automations, reviews]);
}
