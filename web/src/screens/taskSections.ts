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

/** Finished subagents by the request that started them, then finished automation runs and
 *  background tasks, newest first, at most [`DONE_REQUESTS`] groups. */
export function doneGroups(messages: ChatMessage[], tasks: SubagentTask[], earlier: TaskRow[], background: ThreadTaskBackground[], now?: Date): DoneGroup[] {
  const groups: DoneGroup[] = [];
  let request = "";
  for (const row of messages) {
    if (row.role === "user") request = row.text;
    if (!row.subagents) continue;
    const finished = tasks.filter(task => !working(task.state) && row.subagents!.some(agent => agent.id === task.agent.id));
    if (finished.length === 0) continue;
    const at = row.subagents.map(agent => agent.at ?? "").sort().pop() ?? "";
    const key = row.runId ?? row.id;
    const existing = groups.find(group => group.key === key);
    if (existing) {
      existing.subagents.push(...finished);
      if (at > existing.at) existing.at = at;
      continue;
    }
    const time = clockLabel(at, now);
    groups.push({ key, label: [time, plainSnippet(request, 60) || "Request"].filter(Boolean).join(" · "), at, subagents: finished, rows: [], background: [] });
  }
  if (earlier.length) {
    groups.push({ key: "automations", label: "Automations", at: earlier[0]!.at ?? "", subagents: [], rows: earlier, background: [] });
  }
  if (background.length) {
    const at = background.map(task => task.ended_at ?? task.started_at).sort().pop() ?? "";
    groups.push({ key: "background", label: "Background tasks", at, subagents: [], rows: [], background });
  }
  return groups.sort((left, right) => right.at.localeCompare(left.at)).slice(0, DONE_REQUESTS);
}

/** How many finished items Done holds. */
export function doneCount(groups: DoneGroup[]): number {
  return groups.reduce((sum, group) => sum + group.subagents.length + group.rows.length + group.background.length, 0);
}
