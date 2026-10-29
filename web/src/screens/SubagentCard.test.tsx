// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { initialState, reducer } from "../state/reducer";
import type { ApprovalRequest, SubagentEntry } from "../lib/types";
import { clock, firstLine, modelNote, SubagentCard, subagentLine, subagentState } from "./SubagentCard";
import { stepsOf, SubagentDetail } from "./SubagentDetail";
import { SubagentSettingsSection } from "./SubagentSettings";

const dispatch = vi.fn();
let appState: typeof initialState;
vi.mock("../state/store", async importOriginal => ({
  ...await importOriginal<typeof import("../state/store")>(),
  useApp: () => ({ state: appState, dispatch }),
}));
const tauri = vi.hoisted(() => ({
  getSubagentTranscript: vi.fn(async () => [
    { kind: "assistant_text", text: "Starting with the retry loop." },
    { kind: "tool_started", tool: "read_file", summary: "read `src/runtime.rs`" },
    { kind: "tool_started", tool: "search_file", summary: "search `RunEnd`" },
    { kind: "assistant_text", text: "Bug found at runtime.rs:3496." },
    { kind: "finished", result: "Bug found at runtime.rs:3496." },
  ]),
  stopSubagent: vi.fn(async () => undefined),
  getSubagentSettings: vi.fn(async () => ({ settings: { enabled: true, max_parallel: 3, max_turns: 40, model: null, allow_writes: true }, revision: "r1" })),
  updateSubagentSettings: vi.fn(async (changes: object) => ({ settings: { enabled: true, max_parallel: 3, max_turns: 40, model: null, allow_writes: true, ...changes }, revision: "r2" })),
  listGoModels: vi.fn(async () => [{ id: "glm-5" }, { id: "beam-free" }]),
}));
vi.mock("../lib/tauri", () => tauri);

const thread = "t1";
const envelope = (event: object, run_id = "r1") => ({ type: "thread/event" as const, envelope: { thread_id: thread, run_id, event: event as never } });
const agent = (patch: Partial<SubagentEntry> = {}): SubagentEntry => ({ id: "a1", name: "Bug hunt", brief: "Find bugs in runtime.rs", model: "glm-5", confined: true, status: "running", text: "Reading runtime.rs", toolCalls: 3, ...patch });
const approval = (patch: Partial<ApprovalRequest> = {}): ApprovalRequest => ({ thread_id: thread, approval_id: "p1", tool: "shell", summary: "cargo test", risk: "execute", command: { argv: ["cargo", "test"] }, subagent: { id: "a1", name: "Bug hunt" }, ...patch });

beforeEach(() => { dispatch.mockClear(); appState = { ...initialState }; });
afterEach(() => cleanup());

describe("subagent events", () => {
  it("puts subagents started together on one card and updates each in place", () => {
    let state = reducer(initialState, envelope({ kind: "subagent_started", id: "a1", name: "Bug hunt", brief: "b1", model: "glm-5", confined: true }));
    state = reducer(state, envelope({ kind: "subagent_started", id: "a2", name: "Docs", brief: "b2", model: "glm-5", confined: false }));
    state = reducer(state, envelope({ kind: "subagent_status", id: "a2", text: "Reading docs", tool_calls: 2 }));
    state = reducer(state, envelope({ kind: "subagent_ended", id: "a1", status: "finished", result: "2 bugs", tool_calls: 5, seconds: 40 }));
    const cards = state.messages[thread]!.filter(row => row.subagents);
    expect(cards).toHaveLength(1);
    expect(cards[0]!.subagents!.map(entry => [entry.name, entry.status, entry.text, entry.toolCalls])).toEqual([
      ["Bug hunt", "finished", "", 5], ["Docs", "running", "Reading docs", 2],
    ]);
    expect(cards[0]!.subagents![0]!.startedAt).toBeTypeOf("number");
    // A later spawn after other work gets its own card.
    state = reducer(state, envelope({ kind: "tool_started", tool: "spawn_subagents", summary: "spawn" }));
    state = reducer(state, envelope({ kind: "subagent_started", id: "a3", name: "Tests", brief: "b3", model: "glm-5", confined: true }));
    expect(state.messages[thread]!.filter(row => row.subagents)).toHaveLength(2);
  });

  it("rebuilds the card from history, without live start times", () => {
    const state = reducer(initialState, { type: "thread/history-loaded", threadId: thread, history: [
      { kind: "user", run_id: "r1", text: "review" },
      { kind: "event", envelope: { thread_id: thread, run_id: "r1", event: { kind: "subagent_started", id: "a1", name: "Bug hunt", brief: "b", model: "m", confined: true } } },
      { kind: "event", envelope: { thread_id: thread, run_id: "r1", event: { kind: "subagent_ended", id: "a1", status: "stopped", result: "partial", tool_calls: 2, seconds: 12 } } },
    ] } as never);
    const card = state.messages[thread]!.find(row => row.subagents)!;
    expect(card.subagents![0]).toMatchObject({ status: "stopped", seconds: 12, startedAt: undefined });
  });
});

describe("subagent card", () => {
  it("says what each subagent is doing, which one needs approval, and how long they took", () => {
    expect(subagentState(agent(), true, [])).toBe("running");
    expect(subagentState(agent(), true, [approval()])).toBe("approval");
    // Still marked running after its run is over: Themis closed under it.
    expect(subagentState(agent(), false, [])).toBe("lost");
    expect(subagentLine(agent(), "approval", [approval()])).toBe("Approval needed: cargo test");
    expect(subagentLine(agent(), "lost", [])).toBe("Ended when Themis closed");
    expect(subagentLine(agent({ status: "failed", result: "quota exceeded" }), "failed", [])).toBe("Failed: quota exceeded");
    expect(firstLine("\n**Bugs** in `a.rs`:\n1. `average` adds 1")).toBe("average adds 1");
    expect(firstLine("Findings:")).toBe("Findings:");
    expect(clock(112)).toBe("1:52");
  });

  it("shows one letter circle per subagent, ringed while it works, opening it in Workspace › Tasks", () => {
    appState = { ...initialState, approvals: [approval({ subagent: { id: "a2", name: "Test gaps" } })] };
    const { container } = render(<SubagentCard runLive agents={[agent(), agent({ id: "a2", name: "Test gaps" }), agent({ id: "a3", name: "Docs", status: "finished", result: "done", seconds: 48 })]} />);
    const circles = screen.getAllByRole("button", { name: /opens it in Tasks/ });
    expect(circles.map(circle => circle.textContent)).toEqual(["B", "T", "D"]);
    expect(circles[0]).toHaveAttribute("title", "Bug hunt");
    // Working (and waiting on an approval) are ringed; ended is still.
    expect(container.querySelectorAll(".themis-subagent-ring")).toHaveLength(2);
    // No status text in the chat: that lives in the overview box and Tasks.
    expect(screen.queryByText(/running|approval|tool call/i)).toBeNull();
    fireEvent.click(circles[1]!);
    expect(dispatch).toHaveBeenLastCalledWith({ type: "ui/diff-panel", open: true, tab: "tasks", subagent: "a2" });
    fireEvent.click(screen.getByRole("button", { name: "3 subagents" }));
    expect(dispatch).toHaveBeenLastCalledWith({ type: "ui/diff-panel", open: true, tab: "tasks" });
  });

  it("stops every ring once they ended, or when Themis closed under them", () => {
    const { container } = render(<SubagentCard runLive={false} agents={[agent(), agent({ id: "a2", status: "finished", result: "x" })]} />);
    expect(container.querySelectorAll(".themis-subagent-ring")).toHaveLength(0);
    expect(screen.getByRole("button", { name: "2 subagents" })).toBeInTheDocument();
  });
});

describe("subagent models", () => {
  it("records a fallback model from status and end events", () => {
    let state = reducer(initialState, envelope({ kind: "subagent_started", id: "a1", name: "Review", brief: "b", model: "glm-5", confined: true }));
    state = reducer(state, envelope({ kind: "subagent_status", id: "a1", text: "Reading", tool_calls: 1, model: "beam-free" }));
    expect(state.messages[thread]!.find(row => row.subagents)!.subagents![0]!.ranOn).toBe("beam-free");
  });

  it("names the model only when it is not the chat's, and shows a fallback", () => {
    expect(modelNote(agent({ model: "glm-5" }), "glm-5")).toBe("");
    expect(modelNote(agent({ model: "claude-haiku" }), "glm-5")).toBe("claude-haiku");
    expect(modelNote(agent({ model: "claude-haiku", ranOn: "glm-5" }), "glm-5")).toBe("claude-haiku → glm-5");
  });

  it("says in the detail which model it fell back to", async () => {
    render(<SubagentDetail threadId={thread} agent={agent({ model: "claude-haiku", ranOn: "glm-5", status: "finished", seconds: 9 })} runLive={false} onBack={() => {}} />);
    await act(async () => {});
    expect(screen.getByText(/^glm-5 \(claude-haiku unavailable\) · Read only/)).toBeInTheDocument();
  });
});

describe("subagent detail", () => {
  it("groups tool calls between the subagent's own words, without repeating its result", () => {
    expect(stepsOf([
      { kind: "assistant_text", text: "A" },
      { kind: "tool_started", tool: "read_file", summary: "read x" },
      { kind: "tool_started", tool: "read_file", summary: "read y" },
      { kind: "finished", result: "A" },
    ] as never)).toEqual([{ kind: "text", text: "A" }, { kind: "tools", calls: ["read x", "read y"] }]);
    expect(stepsOf([{ kind: "finished", result: "Only the result." }] as never)).toEqual([{ kind: "text", text: "Only the result." }]);
  });

  it("shows the brief, its steps, and Stop while it runs", async () => {
    const back = vi.fn();
    render(<SubagentDetail threadId={thread} agent={agent({ confined: false })} runLive onBack={back} />);
    await act(async () => {});
    expect(screen.getByText("Subagent")).toBeInTheDocument();
    expect(screen.getByText("glm-5 · Read only · Shell not confined · 3 tool calls")).toBeInTheDocument();
    expect(screen.getByText("Find bugs in runtime.rs")).toBeInTheDocument();
    expect(screen.getByText("2 tool calls")).toBeInTheDocument();
    expect(screen.getAllByText("Bug found at runtime.rs:3496.")).toHaveLength(1);
    fireEvent.click(screen.getByRole("button", { name: "Stop" }));
    await act(async () => {});
    expect(tauri.stopSubagent).toHaveBeenCalledWith(thread, "a1");
    fireEvent.click(screen.getByRole("button", { name: "Back to Tasks" }));
    expect(back).toHaveBeenCalled();
  });

  it("offers no Stop once it ended", async () => {
    render(<SubagentDetail threadId={thread} agent={agent({ status: "finished", seconds: 30 })} runLive={false} onBack={() => {}} />);
    await act(async () => {});
    expect(screen.queryByRole("button", { name: "Stop" })).toBeNull();
  });
});

describe("Settings › Subagents", () => {
  it("saves each field on its own with the page's revision", async () => {
    const saved = vi.fn();
    render(<SubagentSettingsSection onSaved={saved} />);
    await act(async () => {});
    expect(screen.getByRole("radio", { name: "3" })).toHaveAttribute("aria-checked", "true");
    fireEvent.click(screen.getByRole("radio", { name: "5" }));
    await act(async () => {});
    expect(tauri.updateSubagentSettings).toHaveBeenLastCalledWith({ max_parallel: 5 }, "r1");
    fireEvent.change(screen.getByRole("combobox", { name: "Subagent model" }), { target: { value: "beam-free" } });
    await act(async () => {});
    expect(tauri.updateSubagentSettings).toHaveBeenLastCalledWith({ model: "beam-free" }, "r2");
    fireEvent.change(screen.getByRole("combobox", { name: "Subagent model" }), { target: { value: "" } });
    await act(async () => {});
    expect(tauri.updateSubagentSettings).toHaveBeenLastCalledWith({ model: null }, "r2");
    expect(saved).toHaveBeenCalledTimes(3);
    // Other models: added from the list, at most three, removed one by one.
    fireEvent.change(screen.getByRole("combobox", { name: "Add a model" }), { target: { value: "glm-5" } });
    await act(async () => {});
    expect(tauri.updateSubagentSettings).toHaveBeenLastCalledWith({ models: ["glm-5"] }, "r2");
    fireEvent.click(screen.getByRole("button", { name: "Remove glm-5" }));
    await act(async () => {});
    expect(tauri.updateSubagentSettings).toHaveBeenLastCalledWith({ models: [] }, "r2");
    // No hint lines and no file-writes switch before write paths exist.
    expect(screen.queryByText(/file writes/i)).toBeNull();
  });
});
