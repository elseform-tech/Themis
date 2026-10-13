// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { initialState } from "../state/reducer";
import type { ThreadTaskAutomation, ThreadTaskRun, ThreadTasks } from "../lib/types";
import * as bridge from "../lib/tauri";
import { TaskIndicator } from "./TaskIndicator";
import { TasksView } from "./TasksView";
import { countTasks, describeCounts, groupTasks, useThreadTasksSync } from "./threadTasks";
import { reducer } from "../state/reducer";

const dispatch = vi.fn();
let appState: typeof initialState;
vi.mock("../state/store", async importOriginal => ({
  ...await importOriginal<typeof import("../state/store")>(),
  useApp: () => ({ state: appState, dispatch }),
}));
vi.mock("../lib/tauri", () => ({
  stopThread: vi.fn(async () => undefined),
  setAutomationEnabled: vi.fn(async (id: string, enabled: boolean) => ({ id, enabled })),
  getThreadTasks: vi.fn(),
}));

const automation = (over: Partial<ThreadTaskAutomation> = {}): ThreadTaskAutomation => ({
  id: "a1", name: "Morning review", enabled: true, origin: "user", interval_mins: 60,
  schedule: { repeat: "daily", time: "09:00", timezone: "Europe/Berlin", weekday: 0 },
  next_run_at: "2030-01-01T08:00:00Z", last_run_at: null, run_count: 0, running: false, ...over,
});
const finished = (over: Partial<ThreadTaskRun> = {}): ThreadTaskRun => ({
  review_id: "r1", automation_id: "a1", name: "Morning review", finished_at: "2030-01-01T08:00:00Z", ok: true, summary: "all good", seen: false, ...over,
});
const tasks = (over: Partial<ThreadTasks> = {}): ThreadTasks => ({
  thread_id: "t1", run: { running: false, awaiting_approval: false }, automations: [], recent: [], ...over,
});

beforeEach(() => { appState = { ...initialState }; dispatch.mockClear(); vi.mocked(bridge.stopThread).mockClear(); vi.mocked(bridge.setAutomationEnabled).mockClear(); });
afterEach(cleanup);

describe("task grouping", () => {
  it("counts each kind and describes them in order", () => {
    const feed = tasks({
      run: { running: false, awaiting_approval: true },
      automations: [automation(), automation({ id: "a2", name: "Second" })],
      recent: [finished(), finished({ review_id: "r2", ok: false, summary: "failed: boom" }), finished({ review_id: "r3", seen: true })],
    });
    const counts = countTasks(feed);
    expect(counts).toEqual({ attention: 2, running: 0, scheduled: 2, finished: 1 });
    expect(describeCounts(counts)).toBe("2 need attention, 2 scheduled, 1 finished");
    expect(describeCounts(countTasks(tasks()))).toBe("");
  });
  it("shows a running automation once and keeps it pausable", () => {
    const groups = groupTasks(tasks({ run: { running: true, awaiting_approval: false }, automations: [automation({ running: true, origin: "agent" })] }));
    expect(groups.running).toHaveLength(1);
    expect(groups.running[0]).toMatchObject({ name: "Morning review", tag: "Automation · by agent", stop: true, pause: "a1" });
    expect(groups.scheduled).toHaveLength(0);
  });
  it("lists paused automations without counting them", () => {
    const feed = tasks({ automations: [automation({ enabled: false })] });
    expect(countTasks(feed).scheduled).toBe(0);
    expect(groupTasks(feed).scheduled[0]).toMatchObject({ meta: "Paused · resume it in Automations" });
    expect(groupTasks(feed).scheduled[0]?.pause).toBeUndefined();
  });
});

describe("scheduler switch", () => {
  it("shows chat automations as off and leaves them out of the scheduled count", () => {
    const feed = tasks({ automations: [automation()] });
    expect(countTasks(feed, false).scheduled).toBe(0);
    expect(countTasks(feed, true).scheduled).toBe(1);
    const row = groupTasks(feed, false).scheduled[0];
    expect(row?.meta).toBe("Automations are off in Settings");
    expect(row?.meta).not.toMatch(/next/);
    expect(groupTasks(feed, true).scheduled[0]?.meta).toMatch(/next/);
  });
});

describe("TaskIndicator", () => {
  it("is hidden when nothing is active", () => {
    const { container } = render(<TaskIndicator counts={countTasks(tasks())} onOpen={() => undefined} />);
    expect(container).toBeEmptyDOMElement();
  });
  it("orders icons, shows +N beyond three kinds and names the tasks", () => {
    const onOpen = vi.fn();
    const feed = tasks({
      run: { running: true, awaiting_approval: false },
      automations: [automation({ id: "a2", name: "Other" })],
      recent: [finished({ ok: false, summary: "failed: x" }), finished({ review_id: "r2" }), finished({ review_id: "r3" })],
    });
    const { container } = render(<TaskIndicator counts={countTasks(feed)} onOpen={onOpen} />);
    const button = screen.getByRole("button", { name: "Tasks in this chat: 1 needs attention, 1 running, 1 scheduled, 2 finished. Opens Workspace tasks" });
    expect([...container.querySelectorAll(".themis-task-icon")].map(icon => icon.className.replace(/.*--/, ""))).toEqual(["attention", "running", "scheduled"]);
    expect(button).toHaveTextContent("+2");
    fireEvent.click(button);
    expect(onOpen).toHaveBeenCalled();
  });
  it("animates the running arc only through CSS that reduced motion disables", () => {
    const { container } = render(<TaskIndicator counts={countTasks(tasks({ run: { running: true, awaiting_approval: false } }))} onOpen={() => undefined} />);
    expect(container.querySelector(".themis-task-arc")).not.toBeNull();
  });
});

describe("TasksView", () => {
  it("groups tasks and dispatches Stop and Pause to the bridge", async () => {
    appState = { ...initialState, tasks: { t1: tasks({
      run: { running: true, awaiting_approval: false },
      automations: [automation({ running: true }), automation({ id: "a2", name: "Evening" })],
      recent: [finished({ review_id: "r9", seen: true })],
    }) } };
    render(<TasksView threadId="t1" />);
    for (const name of ["Running", "Scheduled", "Earlier"]) expect(screen.getByRole("region", { name })).toBeInTheDocument();
    expect(screen.queryByRole("region", { name: "Needs attention" })).toBeNull();
    expect(screen.getByText("Shows this chat only. Stopping never undoes work already done.")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Stop Morning review" }));
    await waitFor(() => expect(bridge.stopThread).toHaveBeenCalledWith("t1"));
    fireEvent.click(screen.getByRole("button", { name: "Pause Evening" }));
    await waitFor(() => expect(bridge.setAutomationEnabled).toHaveBeenCalledWith("a2", false));
    await waitFor(() => expect(dispatch).toHaveBeenCalledWith({ type: "automation/updated", automation: { id: "a2", enabled: false } }));
  });
  it("shows an empty state", () => {
    render(<TasksView threadId="t1" />);
    expect(screen.getByText("No tasks in this chat")).toBeInTheDocument();
  });
});

describe("task feed sync", () => {
  function Probe() { useThreadTasksSync("t1"); return null; }
  it("reloads the feed when the run state changes and stores it by thread", async () => {
    vi.mocked(bridge.getThreadTasks).mockResolvedValue(tasks());
    const view = render(<Probe />);
    await waitFor(() => expect(dispatch).toHaveBeenCalledWith({ type: "tasks/loaded", threadId: "t1", tasks: tasks() }));
    const calls = vi.mocked(bridge.getThreadTasks).mock.calls.length;
    appState = { ...appState, running: { t1: true } };
    view.rerender(<Probe />);
    await waitFor(() => expect(vi.mocked(bridge.getThreadTasks).mock.calls.length).toBe(calls + 1));
  });
  it("stores feeds and tab requests in the reducer", () => {
    let next = reducer(initialState, { type: "tasks/loaded", threadId: "t1", tasks: tasks() });
    expect(next.tasks.t1).toEqual(tasks());
    next = reducer(next, { type: "ui/diff-panel", open: true, tab: "tasks" });
    expect(next.diffPanelOpen).toBe(true);
    expect(next.workspaceTabRequest?.tab).toBe("tasks");
    next = reducer(next, { type: "ui/tab-request-consumed" });
    expect(next.workspaceTabRequest).toBeNull();
  });
});
