// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { initialState } from "../state/reducer";
import type { ChatMessage } from "../state/reducer";
import type { ApprovalRequest, SubagentEntry, ThreadTasks } from "../lib/types";
import { TasksView } from "./TasksView";
import { DONE_REQUESTS, doneCount, doneGroups, subagentTasks } from "./taskSections";

const dispatch = vi.fn();
let appState: typeof initialState;
vi.mock("../state/store", async importOriginal => ({
  ...await importOriginal<typeof import("../state/store")>(),
  useApp: () => ({ state: appState, dispatch }),
}));
vi.mock("../lib/tauri", () => ({
  stopThread: vi.fn(async () => undefined),
  stopSubagent: vi.fn(async () => undefined),
  setAutomationEnabled: vi.fn(),
  stopBackgroundTask: vi.fn(),
  getBackgroundTaskLog: vi.fn(),
  getSubagentTranscript: vi.fn(async () => []),
}));

const agent = (id: string, patch: Partial<SubagentEntry> = {}): SubagentEntry => ({ id, name: `Agent ${id}`, brief: "b", model: "m", confined: true, writePaths: [], status: "finished", text: "", result: `Result of ${id}`, toolCalls: 1, seconds: 5, ...patch });
const request = (runId: string, text: string, agents: SubagentEntry[]): ChatMessage[] => [
  { id: `u-${runId}`, role: "user", text, runId },
  { id: `s-${runId}`, role: "system", text: "", runId, subagents: agents },
];
const feed = (over: Partial<ThreadTasks> = {}): ThreadTasks => ({ thread_id: "t1", run: { running: false, awaiting_approval: false }, automations: [], recent: [], ...over });
const approval = (subagent: string): ApprovalRequest => ({ thread_id: "t1", approval_id: "p", tool: "shell", summary: "cargo test", risk: "execute", command: { argv: ["cargo", "test"] }, subagent: { id: subagent, name: subagent } });

beforeEach(() => {
  dispatch.mockClear();
  // Done's open state is saved in localStorage; a fresh store keeps one test's fold out of the next.
  const store = new Map<string, string>();
  vi.stubGlobal("localStorage", { getItem: (key: string) => store.get(key) ?? null, setItem: (key: string, value: string) => void store.set(key, value) });
});
afterEach(() => { cleanup(); vi.unstubAllGlobals(); });

describe("Done groups", () => {
  it("groups finished subagents by the request that started them, newest first", () => {
    const messages = [
      ...request("r1", "Review the stats module", [agent("a1", { at: "2030-01-01T20:06:00Z" }), agent("a2", { at: "2030-01-01T20:06:00Z" })]),
      ...request("r2", "Check the git history", [agent("a3", { at: "2030-01-01T21:13:00Z" })]),
    ];
    const groups = doneGroups(messages, subagentTasks(messages, undefined, []), [], [], new Date("2030-01-01T22:00:00Z"));
    expect(groups.map(group => group.label.split(" · ")[1])).toEqual(["Check the git history", "Review the stats module"]);
    expect(groups[0]!.label).toMatch(/^\d{1,2}:\d{2}/);
    expect(groups[1]!.subagents.map(task => task.agent.id)).toEqual(["a1", "a2"]);
    expect(doneCount(groups)).toBe(3);
  });

  it("leaves working subagents out and keeps the last requests only", () => {
    const messages = Array.from({ length: DONE_REQUESTS + 2 }, (_, index) => request(`r${index}`, `Request ${index}`, [agent(`a${index}`, { at: `2030-01-01T10:${String(index).padStart(2, "0")}:00Z` })])).flat();
    messages.push(...request("live", "Still working", [agent("w", { status: "running", at: "2030-01-01T11:00:00Z" })]));
    const groups = doneGroups(messages, subagentTasks(messages, "live", []), [], []);
    expect(groups).toHaveLength(DONE_REQUESTS);
    expect(groups.some(group => group.label.includes("Still working"))).toBe(false);
    expect(groups[0]!.label).toContain(`Request ${DONE_REQUESTS + 1}`);
  });
});

describe("Tasks sections", () => {
  it("nests a reply's working subagents under it and lists one waiting on you under Needs you", () => {
    const messages = request("r1", "Review", [agent("a1", { status: "running", name: "Bug hunt" }), agent("a2", { status: "running", name: "Test gaps" }), agent("a3", { name: "Docs check" })]);
    appState = { ...initialState, messages: { t1: messages }, activeRunIds: { t1: "r1" }, approvals: [approval("a2")], tasks: { t1: feed({ run: { running: true, awaiting_approval: false } }) } };
    render(<TasksView threadId="t1" />);
    expect(screen.getByRole("region", { name: "Needs you" })).toHaveTextContent("Test gaps");
    const running = screen.getByRole("region", { name: "Running" });
    // The reply is titled by its request and says what it waits for.
    expect(running).toHaveTextContent("ReviewReply");
    expect(running).toHaveTextContent("Waiting for 1 subagent");
    expect(within(within(running).getByRole("listitem", { name: "Subagents of this reply" })).getByText("Bug hunt")).toBeInTheDocument();
    // The finished one waits, folded, in Done.
    const fold = screen.getByRole("button", { name: "Done · 1" });
    expect(fold).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByText("Docs check")).toBeNull();
    fireEvent.click(fold);
    expect(screen.getByRole("region", { name: "Done" })).toHaveTextContent("Docs check");
  });

  it("opens only the newest request in Done, and says when nothing runs", () => {
    const messages = [
      ...request("r1", "Older request", [agent("a1", { at: "2030-01-01T20:00:00Z", name: "Old one" })]),
      ...request("r2", "Newer request", [agent("a2", { at: "2030-01-01T21:00:00Z", name: "New one" })]),
    ];
    appState = { ...initialState, messages: { t1: messages }, tasks: { t1: feed() } };
    render(<TasksView threadId="t1" />);
    expect(screen.getByText("Nothing running")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Done · 2" }));
    expect(screen.getByText("New one")).toBeInTheDocument();
    expect(screen.queryByText("Old one")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: /Older request/ }));
    expect(screen.getByText("Old one")).toBeInTheDocument();
  });

  it("keeps a reply waiting for approval titled by its request, with its subagents under it", () => {
    const messages = request("r1", "Review the stats module", [agent("a1", { status: "running", name: "Bug hunt" })]);
    appState = { ...initialState, messages: { t1: messages }, activeRunIds: { t1: "r1" }, tasks: { t1: feed({ run: { running: true, awaiting_approval: true } }) } };
    render(<TasksView threadId="t1" />);
    const needs = screen.getByRole("region", { name: "Needs you" });
    expect(needs).toHaveTextContent("Review the stats moduleReply");
    expect(needs).toHaveTextContent("Waiting for your approval");
    expect(within(within(needs).getByRole("listitem", { name: "Subagents of this reply" })).getByText("Bug hunt")).toBeInTheDocument();
    expect(screen.queryByRole("region", { name: "Running" })).toBeNull();
  });

  it("gives a running automation the same waiting line, keeping its own name", () => {
    const messages = request("r1", "Nightly check", [agent("a1", { status: "running", name: "Bug hunt" })]);
    const automation = { id: "w1", name: "Nightly tests", enabled: true, origin: "user" as const, interval_mins: 60, next_run_at: "2030-01-01T02:00:00Z", last_run_at: null, run_count: 3, running: true };
    appState = { ...initialState, messages: { t1: messages }, activeRunIds: { t1: "r1" }, tasks: { t1: feed({ run: { running: true, awaiting_approval: false }, automations: [automation] }) } };
    render(<TasksView threadId="t1" />);
    const running = screen.getByRole("region", { name: "Running" });
    expect(running).toHaveTextContent("Nightly tests");
    expect(running).toHaveTextContent("Waiting for 1 subagent");
    expect(within(running).getByRole("listitem", { name: "Subagents of this reply" })).toHaveTextContent("Bug hunt");
  });

  it("puts a finished background task with the request that started it", () => {
    const messages = request("r1", "Start the dev server", [agent("a1", { at: "2030-01-01T20:00:00Z" })]);
    const task = { id: "bg1", run_id: "r1", command: "npm run dev", cwd: ".", state: "stopped" as const, exit_code: null, pid: 1, started_at: "2030-01-01T20:01:00Z", ended_at: "2030-01-01T20:05:00Z", timeout_secs: 0, last_lines: [], log_bytes: 0 };
    const orphan = { ...task, id: "bg2", run_id: undefined, command: "sleep 9" };
    const groups = doneGroups(messages, subagentTasks(messages, undefined, []), [], [task, orphan]);
    expect(groups.find(group => group.key === "r1")!.background.map(item => item.command)).toEqual(["npm run dev"]);
    expect(groups.find(group => group.key === "background")!.background.map(item => item.command)).toEqual(["sleep 9"]);
  });
});

