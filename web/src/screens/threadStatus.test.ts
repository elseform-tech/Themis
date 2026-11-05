import { describe, expect, it } from "vitest";
import { initialState } from "../state/reducer";
import type { ThreadInfo } from "../lib/types";
import { threadStatus } from "./Sidebar";

const thread = (last_outcome?: ThreadInfo["last_outcome"]): ThreadInfo => ({
  id: "t", title: "T", provider: "go", model: "m", running: false, worktree_path: null, branch: null,
  base_branch: null, recovered: false, approval_mode: "custom", ...(last_outcome ? { last_outcome } : {}),
} as ThreadInfo);

describe("threadStatus for a chat not opened yet", () => {
  it("takes the last run's end from the chat list", () => {
    expect(threadStatus(initialState, thread("failed"))).toBe("Failed");
    expect(threadStatus(initialState, thread("stopped"))).toBe("Stopped");
    expect(threadStatus(initialState, thread("done"))).toBe("Done");
    expect(threadStatus(initialState, thread("incomplete"))).toBe("Idle");
    expect(threadStatus(initialState, thread())).toBe("Idle");
  });

  it("uses the loaded messages once the chat is open", () => {
    const state = { ...initialState, historyLoaded: { t: true as const }, messages: { t: [] } };
    expect(threadStatus(state, thread("failed"))).toBe("Idle");
  });
});

describe("threadStatus for chats waiting for the person", () => {
  const base = thread();
  it("puts an approval first, then a question, then a held goal", () => {
    const approval = { thread_id: "t" } as never;
    const question = { id: "q", run_id: "r", question: "Which?", asked_at: 0, blocking: false } as never;
    const asking = { ...initialState, historyLoaded: { t: true as const }, messages: { t: [{ id: "m", role: "system" as const, text: "Which?", question }] } };
    expect(threadStatus({ ...asking, approvals: [approval] }, base)).toBe("Needs approval");
    expect(threadStatus({ ...asking, goals: { t: { status: "paused" } as never } }, base)).toBe("Question");
    expect(threadStatus({ ...initialState, goals: { t: { status: "paused" } as never } }, base)).toBe("Goal paused");
  });

  it("reads questions and goals from the list for a chat not opened yet", () => {
    expect(threadStatus(initialState, { ...base, open_questions: 2 })).toBe("Question");
    expect(threadStatus(initialState, { ...base, goal_status: "blocked" })).toBe("Goal blocked");
    expect(threadStatus(initialState, { ...base, goal_status: "waiting" })).toBe("Goal waiting");
    // A live goal change wins over the list.
    expect(threadStatus({ ...initialState, goals: { t: null } }, { ...base, goal_status: "paused" })).toBe("Idle");
    expect(threadStatus(initialState, { ...base, goal_status: "running", running: true })).toBe("Working");
  });

  it("an answered question no longer waits", () => {
    const question = { id: "q", run_id: "r", question: "Which?", asked_at: 0, blocking: false, answer: "A" } as never;
    const answered = { ...initialState, historyLoaded: { t: true as const }, messages: { t: [{ id: "m", role: "system" as const, text: "Which?", question }] } };
    expect(threadStatus(answered, base)).toBe("Idle");
  });
});
