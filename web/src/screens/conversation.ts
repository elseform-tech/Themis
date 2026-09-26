import type { ChatMessage } from "../state/reducer";

type MessageRow = { kind: "message"; message: ChatMessage };
type MilestoneRow = { kind: "milestone"; id: string; title: string; items: ChatMessage[]; active: boolean };
type RunRow = { kind: "run"; id: string; runId: string; items: MilestoneRow[]; toolCount: number; durationMs?: number; active: boolean };
export type ConversationRow = MessageRow | MilestoneRow | RunRow;

function milestones(messages: ChatMessage[], running: boolean): (MessageRow | MilestoneRow)[] {
  const rows: (MessageRow | MilestoneRow)[] = [];
  let bucket: MilestoneRow | undefined;
  for (let index = 0; index < messages.length; index++) {
    const message = messages[index]!;
    const next = messages[index + 1];
    // Older stored replies predate the explicit final flag.
    const oldFinal = !message.tool && message.role === "assistant" && (next ? next.role !== "assistant" : !running);
    if (message.role !== "assistant" || message.final || oldFinal) {
      if (bucket) bucket.active = false;
      bucket = undefined;
      rows.push({ kind: "message", message });
      continue;
    }
    const heading = !message.tool ? /^\s*(?:\*\*)?Milestone:\s*([^\n]+)\n?([\s\S]*)$/i.exec(message.text) : null;
    const title = heading?.[1]?.replace(/\*\*$/, "").trim();
    if (!bucket || (title && title !== bucket.title)) {
      if (bucket) bucket.active = false;
      bucket = { kind: "milestone", id: message.id, title: title || "Working on your request", items: [], active: running };
      rows.push(bucket);
    }
    const text = heading ? heading[2]!.trim() : message.text;
    if (text || message.tool) bucket.items.push({ ...message, text });
  }
  return rows;
}

/** Group a completed run's milestones under one row; retain legacy messages. */
export function groupConversation(messages: ChatMessage[], running: boolean, runDurations: Record<string, number> = {}, activeRunId?: string): ConversationRow[] {
  const rows: ConversationRow[] = [];
  let activity: ChatMessage[] = [];
  let runId: string | undefined;
  const flush = (active: boolean) => {
    if (!activity.length) return;
    const isActive = active && (runId === undefined ? activeRunId === undefined : runId === activeRunId);
    if (runId) {
      const grouped = milestones(activity, true).filter((row): row is MilestoneRow => row.kind === "milestone");
      rows.push({
        kind: "run", id: activity[0]!.id, runId,
        items: grouped.map((row, index) => ({ ...row, active: isActive && index === grouped.length - 1 })),
        toolCount: activity.filter(message => message.tool).length,
        ...(runDurations[runId] === undefined ? {} : { durationMs: runDurations[runId] }),
        active: isActive,
      });
    } else rows.push(...milestones(activity, isActive));
    activity = [];
    runId = undefined;
  };
  for (const message of messages) {
    const isActivity = message.role === "assistant" && !message.final;
    if (isActivity) {
      if (activity.length && message.runId !== runId) flush(false);
      runId = message.runId;
      activity.push(message);
    } else {
      flush(false);
      rows.push({ kind: "message", message });
    }
  }
  flush(running);
  return rows;
}
