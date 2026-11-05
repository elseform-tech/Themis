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
