// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { initialState } from "../state/reducer";
import { applyThreadEvent } from "../state/reducer";
import type { ThreadTaskBackground, ThreadTasks } from "../lib/types";
import * as bridge from "../lib/tauri";
import { TaskIndicator } from "./TaskIndicator";
import { TasksView } from "./TasksView";
import { backgroundMeta, countBackground, detectPort, groupBackground, needsAttention } from "./backgroundTasks";
import { countTasks, describeCounts } from "./threadTasks";

const dispatch = vi.fn();
let appState: typeof initialState;
vi.mock("../state/store", async importOriginal => ({
  ...await importOriginal<typeof import("../state/store")>(),
  useApp: () => ({ state: appState, dispatch }),
}));
vi.mock("../lib/tauri", () => ({
  stopThread: vi.fn(async () => undefined),
  setAutomationEnabled: vi.fn(),
  getThreadTasks: vi.fn(),
  stopBackgroundTask: vi.fn(async () => undefined),
  getBackgroundTaskLog: vi.fn(),
}));

const task = (over: Partial<ThreadTaskBackground> = {}): ThreadTaskBackground => ({
  id: "bg_a1b2c3", command: "npm run dev", cwd: "/p/web", state: "running", exit_code: null, pid: 4242,
  started_at: "2030-01-01T14:05:00Z", ended_at: null, timeout_secs: 7200,
  last_lines: ["VITE ready", "Local: http://localhost:1420/"], log_bytes: 40, ...over,
});
const feed = (background: ThreadTaskBackground[]): ThreadTasks => ({
  thread_id: "t1", run: { running: false, awaiting_approval: false }, automations: [], recent: [], background_tasks: background,
});

beforeEach(() => { appState = { ...initialState }; vi.clearAllMocks(); });
afterEach(cleanup);

describe("background task grouping", () => {
  it("sorts running, lost or failed, and finished tasks, newest first", () => {
    const groups = groupBackground(feed([
      task({ id: "bg_000001", state: "exited", exit_code: 0 }),
      task({ id: "bg_000002", state: "lost" }),
      task({ id: "bg_000003", state: "exited", exit_code: 2 }),
      task({ id: "bg_000004", state: "stopped" }),
      task({ id: "bg_000005" }),
      task({ id: "bg_000006", state: "timed_out" }),
    ]));
    expect(groups.running.map(item => item.id)).toEqual(["bg_000005"]);
    expect(groups.attention.map(item => item.id)).toEqual(["bg_000006", "bg_000003", "bg_000002"]);
    expect(groups.earlier.map(item => item.id)).toEqual(["bg_000004", "bg_000001"]);
    expect(groupBackground(undefined)).toEqual({ attention: [], running: [], earlier: [] });
    expect(needsAttention(task({ state: "stopped" }))).toBe(false);
  });
  it("counts into the indicator as running and attention", () => {
    const tasks = feed([task(), task({ id: "bg_2", state: "lost" }), task({ id: "bg_3", state: "exited", exit_code: 0 })]);
    expect(countBackground(tasks)).toEqual({ attention: 1, running: 1 });
    expect(countTasks(tasks)).toEqual({ attention: 1, running: 1, scheduled: 0, finished: 0 });
    expect(describeCounts(countTasks(tasks))).toBe("1 needs attention, 1 running");
    const { container } = render(<TaskIndicator counts={countTasks(tasks)} onOpen={() => undefined} />);
    expect(container.querySelectorAll(".themis-task-icon")).toHaveLength(2);
  });
  it("finds the local port and words the meta line", () => {
    expect(detectPort(["boot", "Listening on http://127.0.0.1:8765/"])).toBe("8765");
    expect(detectPort(["Serving HTTP on :: port 9000"])).toBe("9000");
    expect(detectPort(["nothing"])).toBeNull();
    expect(backgroundMeta(task())).toMatch(/^Started .+ by agent · ends when this chat closes · port 1420$/);
    expect(backgroundMeta(task({ state: "lost" }))).toMatch(/lost when the Themis server restarted/);
    expect(backgroundMeta(task({ state: "exited", exit_code: 3, ended_at: "2030-01-01T15:00:00Z" }))).toMatch(/Exited with code 3/);
  });
});

describe("TasksView background rows", () => {
  it("shows a running task with its latest output and Stop", async () => {
    appState = { ...initialState, tasks: { t1: feed([task()]) } };
    render(<TasksView threadId="t1" />);
    const running = screen.getByRole("region", { name: "Running" });
    expect(running).toHaveTextContent("npm run dev");
    expect(running).toHaveTextContent("Background task");
    expect(running).toHaveTextContent("ends when this chat closes");
    expect(running).toHaveTextContent("port 1420");
    expect(screen.getByLabelText("Latest output of npm run dev")).toHaveTextContent("Local: http://localhost:1420/");
    fireEvent.click(screen.getByRole("button", { name: "Stop npm run dev" }));
    await waitFor(() => expect(bridge.stopBackgroundTask).toHaveBeenCalledWith("t1", "bg_a1b2c3"));
  });
  it("puts lost tasks under Needs attention and finished ones under Earlier, without Stop", () => {
    appState = { ...initialState, tasks: { t1: feed([task({ id: "bg_l", state: "lost", command: "python3 -m http.server" }), task({ id: "bg_f", state: "stopped", command: "sleep 9" })]) } };
    render(<TasksView threadId="t1" />);
    expect(screen.getByRole("region", { name: "Needs attention" })).toHaveTextContent("python3 -m http.server");
    expect(screen.getByRole("region", { name: "Earlier" })).toHaveTextContent("sleep 9");
    expect(screen.queryByRole("button", { name: /^Stop / })).toBeNull();
    expect(screen.queryByText("No tasks in this chat")).toBeNull();
  });
  it("reads the whole bounded log page by page and hides it again", async () => {
    vi.mocked(bridge.getBackgroundTaskLog)
      .mockResolvedValueOnce({ id: "bg_a1b2c3", state: "running", text: "first\n", offset: 0, next_offset: 6, total_bytes: 12, oldest_retained_offset: 0 })
      .mockResolvedValueOnce({ id: "bg_a1b2c3", state: "running", text: "second", offset: 6, next_offset: 12, total_bytes: 12, oldest_retained_offset: 0 });
    appState = { ...initialState, tasks: { t1: feed([task()]) } };
    render(<TasksView threadId="t1" />);
    fireEvent.click(screen.getByRole("button", { name: "Show full log of npm run dev" }));
    const log = await screen.findByLabelText("Full log of npm run dev");
    expect(log.textContent).toBe("first\nsecond");
    expect(bridge.getBackgroundTaskLog).toHaveBeenNthCalledWith(2, "t1", "bg_a1b2c3", 6, 16384);
    fireEvent.click(screen.getByRole("button", { name: "Hide full log of npm run dev" }));
    expect(screen.queryByLabelText("Full log of npm run dev")).toBeNull();
  });
  it("says when older output rotated out and reports failures", async () => {
    vi.mocked(bridge.getBackgroundTaskLog).mockResolvedValueOnce({ id: "bg_a1b2c3", state: "running", text: "tail", offset: 100, next_offset: 104, total_bytes: 104, oldest_retained_offset: 100 });
    appState = { ...initialState, tasks: { t1: feed([task()]) } };
    render(<TasksView threadId="t1" />);
    fireEvent.click(screen.getByRole("button", { name: "Show full log of npm run dev" }));
    expect((await screen.findByLabelText("Full log of npm run dev")).textContent).toMatch(/Earlier output rotated out.*tail/s);
    cleanup();
    vi.mocked(bridge.stopBackgroundTask).mockRejectedValueOnce(new Error("unknown background task"));
    render(<TasksView threadId="t1" />);
    fireEvent.click(screen.getByRole("button", { name: "Stop npm run dev" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("unknown background task");
  });
});

describe("background change event", () => {
  it("leaves reducer state untouched; the store refreshes the feed", () => {
    expect(applyThreadEvent(initialState, { thread_id: "t1", run_id: "", event: { kind: "background_tasks_changed" } })).toBe(initialState);
  });
});
