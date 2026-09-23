import type { ChatMessage } from "../state/reducer";

export type ConversationRow = { kind: "message"; message: ChatMessage } | { kind: "milestone"; id: string; title: string; items: ChatMessage[]; active: boolean };

/** Public milestone headings organize tool activity; old runs fall back to one bucket. */
export function groupConversation(messages: ChatMessage[], running: boolean): ConversationRow[] {
  const rows: ConversationRow[] = [];
  let bucket: Extract<ConversationRow, { kind: "milestone" }> | undefined;
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
