// Finished work of the chat for the Tasks tab's Done section: grouped by the request it
// came from, newest first. Pure logic; TasksView renders it.
import type { ApprovalRequest, SubagentEntry, ThreadTaskBackground } from "../lib/types";
import type { ChatMessage } from "../state/reducer";
import { plainSnippet } from "../lib/plainText";
import { subagentState, type SubagentState } from "./SubagentCard";
import type { TaskRow } from "./threadTasks";

/** Requests kept in Done; older ones stay reachable from the chat. */
export const DONE_REQUESTS = 10;

export interface SubagentTask {
  agent: SubagentEntry;
  runLive: boolean;
  state: SubagentState;
  /** Its place in its reply, for the letter circle's tone. */
  index: number;
}

export interface DoneGroup {
  key: string;
  /** "23:07 · Review src/ for bugs" */
  label: string;
  /** Newest time in the group, for ordering. */
  at: string;
  subagents: SubagentTask[];
  rows: TaskRow[];
  background: ThreadTaskBackground[];
}

/** "23:07", or "Oct 20" for another day. */
export function clockLabel(iso: string | undefined, now: Date = new Date()): string {
  if (!iso) return "";
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return "";
  return date.toDateString() === now.toDateString()
    ? date.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" })
    : date.toLocaleDateString(undefined, { month: "short", day: "numeric" });
}

/** Every subagent of the chat with its state; `activeRun` is the run still going. */
export function subagentTasks(messages: ChatMessage[], activeRun: string | undefined, approvals: ApprovalRequest[]): SubagentTask[] {
  return messages.flatMap(row => (row.subagents ?? []).map((agent, index) => {
    const runLive = row.runId !== undefined && row.runId === activeRun;
    return { agent, runLive, state: subagentState(agent, runLive, approvals), index };
  }));
}

const working = (state: SubagentState) => state === "running" || state === "approval";

/** Finished work by the request that started it (its subagents and background tasks), then
 *  finished automation runs and background tasks with no known request, newest first, at
 *  most [`DONE_REQUESTS`] groups. */
export function doneGroups(messages: ChatMessage[], tasks: SubagentTask[], earlier: TaskRow[], background: ThreadTaskBackground[], now?: Date): DoneGroup[] {
  const requests = new Map<string, string>();
  for (const row of messages) if (row.role === "user" && row.runId) requests.set(row.runId, row.text);
  const byRun = new Map<string, DoneGroup>();
  const group = (runId: string): DoneGroup => {
    let found = byRun.get(runId);
    if (!found) {
      found = { key: runId, label: "", at: "", subagents: [], rows: [], background: [] };
      byRun.set(runId, found);
    }
    return found;
  };
  const later = (current: string, at: string | undefined) => (at && at > current ? at : current);
  for (const row of messages) {
    if (!row.subagents || !row.runId) continue;
    for (const task of tasks) {
      if (working(task.state) || !row.subagents.some(agent => agent.id === task.agent.id)) continue;
      const target = group(row.runId);
      target.subagents.push(task);
      target.at = later(target.at, task.agent.at);
    }
  }
  const unassigned: ThreadTaskBackground[] = [];
  for (const task of background) {
    if (task.run_id && requests.has(task.run_id)) {
      const target = group(task.run_id);
      target.background.push(task);
      target.at = later(target.at, task.started_at);
    } else {
      unassigned.push(task);
    }
  }
  const groups = [...byRun.values()].map(found => ({
    ...found,
    label: [clockLabel(found.at, now), plainSnippet(requests.get(found.key) ?? "", 60) || "Request"].filter(Boolean).join(" · "),
  }));
  if (earlier.length) {
    groups.push({ key: "automations", label: "Automations", at: earlier[0]!.at ?? "", subagents: [], rows: earlier, background: [] });
  }
  if (unassigned.length) {
    const at = unassigned.map(task => task.ended_at ?? task.started_at).sort().pop() ?? "";
    groups.push({ key: "background", label: "Background tasks", at, subagents: [], rows: [], background: unassigned });
  }
  return groups.sort((left, right) => right.at.localeCompare(left.at)).slice(0, DONE_REQUESTS);
}

/** How many finished items Done holds. */
export function doneCount(groups: DoneGroup[]): number {
  return groups.reduce((sum, group) => sum + group.subagents.length + group.rows.length + group.background.length, 0);
}
