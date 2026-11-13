import type { ContextBudget, MemoryChange, SubagentEntry, ThreadInfo } from "../../lib/types";
import type { AppState, ChatMessage, ToolTraceEntry } from "./model";
import { newId } from "./model";

export { setThreadRunning, appendTrace, userMessage, byVision, updateSubagent, markTurnRejected, memoryChanges };

function setThreadRunning(state: AppState, threadId: string, running: boolean): AppState {
  const threadsByProject: Record<string, ThreadInfo[]> = {};
  const activeRunIds = { ...state.activeRunIds };
  if (!running) delete activeRunIds[threadId];
  for (const [root, threads] of Object.entries(state.threadsByProject)) {
    threadsByProject[root] = threads.map((t) =>
      t.id === threadId ? { ...t, running } : t,
    );
  }
  return {
    ...state,
    threadsByProject,
    running: { ...state.running, [threadId]: running },
    activeRunIds,
  };
}

function appendTrace(
  state: AppState,
  threadId: string,
  entry: ToolTraceEntry,
): AppState {
  return {
    ...state,
    traces: {
      ...state.traces,
      [threadId]: [...(state.traces[threadId] ?? []), entry],
    },
  };
}

/** The marker the runtime puts on the message that starts a goal turn. */
export const GOAL_TURN_MARKER = "[Goal — runtime, not from the user]";

/** The turn number of a goal-turn message, if `text` is one. */
export function goalTurnNumber(text: string): number | undefined {
  if (!text.startsWith(GOAL_TURN_MARKER)) return undefined;
  const match = /^\s*Turn (\d+)/.exec(text.slice(GOAL_TURN_MARKER.length));
  return match ? Number(match[1]) : undefined;
}

/** The marker the runtime puts on the message that starts a turn for a plugin hook's report. */
export const HOOK_WAKE_MARKER = "[Plugin hook \u2014 runtime, not from the user]";

/** The runtime's line about the turn it started for a hook's report, if `text` starts one. */
export function hookWakeSummary(text: string): string | undefined {
  if (!text.startsWith(HOOK_WAKE_MARKER)) return undefined;
  return text.slice(HOOK_WAKE_MARKER.length).split("\n")[0]!.trim();
}

/** The field naming the Vision that wrote a user message row, when one did. */
function byVision(visionId: string | undefined): Pick<ChatMessage, "visionId"> {
  return visionId ? { visionId } : {};
}

/** A user message row (a Vision's when `visionId` is set); a goal-turn message becomes a divider row. */
function userMessage(text: string, runId: string | undefined, attachments?: string[], visionId?: string): ChatMessage {
  const goalTurn = goalTurnNumber(text);
  const hookWake = hookWakeSummary(text);
  return {
    id: newId("msg"), role: "user", text, runId,
    ...(attachments?.length ? { attachments } : {}),
    ...(goalTurn === undefined ? {} : { goalTurn }),
    ...(hookWake === undefined ? {} : { hookWake }),
    ...byVision(visionId),
  };
}

function updateSubagent(state: AppState, threadId: string, id: string, change: (agent: SubagentEntry) => SubagentEntry): AppState {
  const rows = state.messages[threadId] ?? [];
  const updated = rows.map(row => row.subagents?.some(agent => agent.id === id)
    ? { ...row, subagents: row.subagents.map(agent => agent.id === id ? change(agent) : agent) }
    : row);
  return { ...state, messages: { ...state.messages, [threadId]: updated } };
}

/** Marks the replies of the goal turn the check just rejected: everything since it began. */
function markTurnRejected(state: AppState, threadId: string): AppState {
  const rows = state.messages[threadId] ?? [];
  const start = rows.map(row => row.role).lastIndexOf("user");
  const marked = rows.map((row, index) => index > start && row.role === "assistant" && !row.tool && row.text ? { ...row, goalRejected: true } : row);
  return { ...state, messages: { ...state.messages, [threadId]: marked } };
}

export function usedPercent(budget: ContextBudget): number {
  return budget.usable_tokens > 0 ? Math.min(100, Math.round(budget.used_tokens / budget.usable_tokens * 100)) : 0;
}

/** A chat saved before memory models carries one change in the event itself
 *  (`action` saved/updated/forgot, `scope`, `name`, `description`) instead of `changes`. */
function memoryChanges(event: Record<string, unknown>): MemoryChange[] {
  if (Array.isArray(event.changes)) return event.changes as MemoryChange[];
  if (typeof event.name !== "string" || (event.scope !== "global" && event.scope !== "project")) return [];
  const action = event.action === "forgot" ? "removed" : event.action === "updated" ? "updated" : "saved";
  return [{ action, scope: event.scope, name: event.name, description: typeof event.description === "string" ? event.description : "", model: "", model_index: -1 }];
}
