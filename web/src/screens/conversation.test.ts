import { expect, it } from "vitest";
import { groupConversation } from "./conversation";
import type { ChatMessage } from "../state/reducer";

it("groups tools under milestones and keeps only the current milestone active", () => {
  const messages: ChatMessage[] = [
    { id: "u", role: "user", text: "Inspect and verify", runId: "run-1" },
    { id: "a", role: "assistant", text: "Milestone: Inspect\nReading the file.", runId: "run-1" },
    { id: "t", role: "assistant", text: "{}", runId: "run-1", tool: { name: "read_file", ok: true, output: "hello" } },
    { id: "b", role: "assistant", text: "Milestone: Verify", runId: "run-1" },
    { id: "t2", role: "assistant", text: "{}", runId: "run-1", tool: { name: "read_file" } },
  ];
  const rows = groupConversation(messages, true, {}, "run-1");
  expect(rows).toHaveLength(2);
  expect(rows[1]).toMatchObject({ kind: "run", runId: "run-1", active: true, items: [
    { kind: "milestone", title: "Inspect", active: false, items: [{ text: "Reading the file." }, { tool: { output: "hello" } }] },
    { kind: "milestone", title: "Verify", active: true },
  ] });
  const done = groupConversation([...messages, { id: "f", role: "assistant", text: "Done", final: true }], false);
  expect(done[1]).toMatchObject({ kind: "run", active: false });
  expect(done[2]).toMatchObject({ kind: "message", message: { text: "Done" } });
});
it("keeps simple and legacy final answers outside buckets", () => {
  expect(groupConversation([{ id: "a", role: "assistant", text: "Hello" }], false)[0]?.kind).toBe("message");
});

it("puts completed run activity under one bucket while leaving its answer visible", () => {
  const rows = groupConversation([
    { id: "u", role: "user", text: "Fix it", runId: "run-1" },
    { id: "m", role: "assistant", text: "Milestone: Inspect\nChecking", runId: "run-1" },
    { id: "t", role: "assistant", text: "{}", runId: "run-1", tool: { name: "read_file", ok: true, output: "ok" } },
    { id: "f", role: "assistant", text: "Done", runId: "run-1", final: true },
  ], false, { "run-1": 12_500 });
  expect(rows).toHaveLength(3);
  expect(rows[1]).toMatchObject({ kind: "run", runId: "run-1", active: false, toolCount: 1, durationMs: 12_500, items: [{ kind: "milestone", title: "Inspect" }] });
  expect(rows[2]).toMatchObject({ kind: "message", message: { text: "Done" } });
});

it("does not mark an older run's completed activity active during a follow-up run", () => {
  const rows = groupConversation([
    { id: "old", role: "assistant", text: "Milestone: Previous\nOld response", runId: "old-run" },
  ], true, {}, "new-run");
  expect(rows[0]).toMatchObject({ kind: "run", runId: "old-run", active: false });
});

it("keeps untagged legacy replies static when the active run ID is unavailable", () => {
  const rows = groupConversation([
    { id: "old", role: "assistant", text: "Previous reply" },
  ], true);
  expect(rows[0]).toMatchObject({ kind: "message", message: { text: "Previous reply" } });
});
