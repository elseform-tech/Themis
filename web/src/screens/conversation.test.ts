import { expect, it } from "vitest";
import { groupConversation } from "./conversation";
import type { ChatMessage } from "../state/reducer";

it("groups tools under milestones and keeps only the current milestone active", () => {
  const messages: ChatMessage[] = [
    { id: "u", role: "user", text: "Inspect and verify" },
    { id: "a", role: "assistant", text: "Milestone: Inspect\nReading the file." },
    { id: "t", role: "assistant", text: "{}", tool: { name: "read_file", ok: true, output: "hello" } },
    { id: "b", role: "assistant", text: "Milestone: Verify" },
    { id: "t2", role: "assistant", text: "{}", tool: { name: "read_file" } },
  ];
  const rows = groupConversation(messages, true);
  expect(rows).toHaveLength(3);
  expect(rows[1]).toMatchObject({ kind: "milestone", title: "Inspect", active: false, items: [{ text: "Reading the file." }, { tool: { output: "hello" } }] });
  expect(rows[2]).toMatchObject({ kind: "milestone", title: "Verify", active: true });
  const done = groupConversation([...messages, { id: "f", role: "assistant", text: "Done", final: true }], false);
  expect(done[2]).toMatchObject({ active: false });
  expect(done[3]).toMatchObject({ kind: "message", message: { text: "Done" } });
});
it("keeps simple and legacy final answers outside buckets", () => {
  expect(groupConversation([{ id: "a", role: "assistant", text: "Hello" }], false)[0]?.kind).toBe("message");
});
