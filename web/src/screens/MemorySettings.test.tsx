// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import * as bridge from "../lib/tauri";
import type { MemoryChange, MemoryList, MemoryWaiting } from "../lib/types";
import { initialState, reducer } from "../state/reducer";
import { MEMORY_SETTLE_MS, MEMORY_WAITING_MS, MemoryLine, MemorySettingsSection, MemoryWorkingLine, memoryDate, memoryTitle } from "./MemorySettings";

const dispatch = vi.fn();
vi.mock("../lib/tauri", () => ({
  getMemorySettings: vi.fn(),
  updateMemorySettings: vi.fn(),
  listMemories: vi.fn(),
  deleteMemory: vi.fn(async () => ({ revision: "r2" })),
}));
vi.mock("../state/store", async original => ({
  ...await original<typeof import("../state/store")>(),
  useApp: () => ({ state: { ...initialState, activeProjectRoot: "/projects/Themis", projects: [{ root: "/projects/Themis", name: "Themis", is_git: true }] }, dispatch }),
  useActiveProject: () => ({ root: "/projects/Themis", name: "Themis", is_git: true }),
}));
afterEach(() => { cleanup(); vi.clearAllMocks(); });

const PROJECT: MemoryList = {
  memories: [
    { name: "merges-need-an-ok", description: "Open PRs freely; merge only after an explicit yes.", type: "project", saved: "2026-10-31", body: "Merging needs the owner's approval.\n\nHow to apply: ask first." },
    { name: "burndown-layout", description: "A chart layout from a blog post.", type: "reference", saved: "2026-10-30", source: "web page https://example.com/post", body: "X axis days." },
  ],
  skipped: 0, index_lines: 2, max_lines: 200, revision: "r1", enabled: true,
};
const GLOBAL: MemoryList = {
  memories: [{ name: "free-model-for-tests", description: "Use the free model for test runs.", type: "feedback", saved: "2026-11-02", body: "Use it." }],
  skipped: 0, index_lines: 1, max_lines: 200, revision: "g1", enabled: true,
};

const MODELS = ["muse-spark-1.3-contributor", "claude-haiku-5-5", "deepseek-v4.1-flash"];

function setup(enabled = true, waiting: MemoryWaiting | null = null) {
  vi.mocked(bridge.getMemorySettings).mockResolvedValue({ settings: { enabled, models: MODELS, daily_budget_usd: 0.5, consolidate_every_hours: 24 }, revision: "s1", waiting });
  vi.mocked(bridge.listMemories).mockImplementation(async scope => scope === "global" ? GLOBAL : PROJECT);
}

describe("Settings › Memory", () => {
  it("groups this project's memories by kind with dates, sources and the index meter", async () => {
    setup();
    await act(async () => { render(<MemorySettingsSection />); });
    expect(bridge.listMemories).toHaveBeenCalledWith("project", "/projects/Themis");
    expect(screen.getByRole("button", { name: "Themis project" })).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByText("2 memories")).toBeVisible();
    expect(screen.getByText("index 2 of 200 lines")).toBeVisible();
    expect(screen.getByRole("region", { name: "Project facts" })).toHaveTextContent("Merges need an ok");
    const pointers = screen.getByRole("region", { name: "Pointers" });
    expect(within(pointers).getByText("from a web page")).toHaveAttribute("title", "web page https://example.com/post");
    // Kinds with nothing in them are left out.
    expect(screen.queryByRole("region", { name: "About you" })).toBeNull();
  });

  it("shows global memories under Everywhere", async () => {
    setup();
    await act(async () => { render(<MemorySettingsSection />); });
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Everywhere" })); });
    expect(bridge.listMemories).toHaveBeenLastCalledWith("global", null);
    expect(screen.getByRole("region", { name: "Your feedback" })).toHaveTextContent("Free model for tests");
    expect(screen.getByText("1 memory")).toBeVisible();
  });

  it("opens a memory to read it and deletes it only after the inline confirm", async () => {
    setup();
    await act(async () => { render(<MemorySettingsSection />); });
    fireEvent.click(screen.getByText("Merges need an ok"));
    expect(screen.getByText(/Merging needs the owner's approval/)).toBeVisible();
    fireEvent.click(screen.getAllByRole("button", { name: "Delete" })[0]!);
    expect(screen.getByText("Delete this memory? The agent won't see it again.")).toBeVisible();
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(bridge.deleteMemory).not.toHaveBeenCalled();
    fireEvent.click(screen.getAllByRole("button", { name: "Delete" })[0]!);
    const confirm = screen.getAllByRole("button", { name: "Delete" }).find(button => button.closest(".themis-memory-actions")?.textContent?.includes("Cancel"))!;
    await act(async () => { fireEvent.click(confirm); });
    expect(bridge.deleteMemory).toHaveBeenCalledWith("project", "/projects/Themis", "merges-need-an-ok", "r1");
    expect(bridge.listMemories).toHaveBeenCalledTimes(2);
  });

  it("opens on the memory a chat line pointed to", async () => {
    setup();
    await act(async () => { render(<MemorySettingsSection focusKey="global:free-model-for-tests" />); });
    expect(bridge.listMemories).toHaveBeenCalledWith("global", null);
    expect(screen.getByText("Use it.")).toBeVisible();
  });

  it("turns memory off in place and then shows no list", async () => {
    setup();
    vi.mocked(bridge.updateMemorySettings).mockResolvedValue({ settings: { enabled: false, models: MODELS, daily_budget_usd: 0.5, consolidate_every_hours: 24 }, revision: "s2" });
    const saved = vi.fn();
    await act(async () => { render(<MemorySettingsSection onSaved={saved} />); });
    await act(async () => { fireEvent.click(screen.getByRole("checkbox", { name: /Remember things across chats/ })); });
    expect(bridge.updateMemorySettings).toHaveBeenCalledWith({ enabled: false }, "s1");
    expect(saved).toHaveBeenCalled();
    expect(screen.getByText(/Memory is off/)).toBeVisible();
    expect(screen.queryByText("2 memories")).toBeNull();
    expect(screen.queryByRole("button", { name: "Everywhere" })).toBeNull();
  });

  it("has one switch: memory is kept automatically, with no noticing switch", async () => {
    setup();
    await act(async () => { render(<MemorySettingsSection />); });
    expect(screen.getAllByRole("checkbox")).toHaveLength(1);
    expect(screen.queryByText(/Notice what's worth remembering/)).toBeNull();
    expect(screen.queryByRole("status")).toBeNull();
  });

  it("says when memory jobs wait for the memory models, and why", async () => {
    setup(true, { jobs: 2, reasons: ["rate_limited", "provider_error", "over_budget"], next_at: null });
    await act(async () => { render(<MemorySettingsSection />); });
    expect(screen.getByRole("status")).toHaveTextContent("Waiting for the memory models: 2 jobs (rate limited, unavailable, daily cap reached)");
  });
});

const change = (over: Partial<MemoryChange> = {}): MemoryChange => ({ action: "saved", scope: "global", name: "python-preference", description: "Prefers Python", model: "muse-spark-1.3-contributor", model_index: 0, ...over });

describe("memory in the chat", () => {
  it("shimmers while the memory models check the turn, then shows what it writes, then settles", () => {
    vi.useFakeTimers();
    const { unmount } = render(<MemoryWorkingLine step="checking" />);
    expect(screen.getByRole("status")).toHaveTextContent("Checking what to remember…");
    expect(screen.getByRole("status").querySelector(".themis-shimmer")).not.toBeNull();
    unmount();
    render(<MemoryLine changes={[change()]} settling />);
    expect(screen.getByRole("status")).toHaveTextContent("Saving python-preference.md");
    act(() => { vi.advanceTimersByTime(MEMORY_SETTLE_MS); });
    expect(screen.queryByRole("status")).toBeNull();
    expect(screen.getByRole("note")).toHaveTextContent("Saved python-preference.md · global");
    vi.useRealTimers();
  });

  it("says quietly that memory will retry when every model failed, then fades", () => {
    vi.useFakeTimers();
    render(<MemoryWorkingLine step="waiting" />);
    expect(screen.getByRole("status")).toHaveTextContent("Memory will retry later");
    act(() => { vi.advanceTimersByTime(MEMORY_WAITING_MS); });
    expect(screen.queryByRole("status")).toBeNull();
    vi.useRealTimers();
  });

  it("tracks the pass live and marks a line that arrived while it was shown", () => {
    const event = (body: object) => ({ type: "thread/event", envelope: { thread_id: "t", run_id: "r", event: body } }) as never;
    let state = reducer(initialState, event({ kind: "memory_working", step: "checking" }));
    expect(state.memoryWorking.t).toBe("checking");
    state = reducer(state, event({ kind: "memory_changed", changes: [change()] }));
    expect(state.memoryWorking.t).toBeUndefined();
    const rows = state.messages.t ?? [];
    expect(rows[rows.length - 1]?.memorySettling).toBe(true);
    // A line read back from history (no live pass) does not replay the animation.
    const replayed = reducer(initialState, event({ kind: "memory_changed", changes: [change()] }));
    expect((replayed.messages.t ?? [])[0]?.memorySettling).toBeUndefined();
    // Nothing worth keeping: the shimmer just goes.
    state = reducer(reducer(state, event({ kind: "memory_working", step: "checking" })), event({ kind: "memory_working", step: "done" }));
    expect(state.memoryWorking.t).toBeUndefined();
  });

  it("is one slim line with the dot-grid mark, whose name opens the memory in Settings", () => {
    render(<MemoryLine changes={[change()]} />);
    const line = screen.getByRole("note");
    expect(line).toHaveTextContent("Saved python-preference.md · global");
    expect(line.querySelector(".themis-memory-icon svg")).not.toBeNull();
    const name = screen.getByRole("button", { name: /python-preference\.md/ });
    expect(name).toHaveAttribute("title", "memory/python-preference.md · by muse-spark-1.3-contributor");
    fireEvent.click(name);
    expect(dispatch).toHaveBeenCalledWith({ type: "ui/view", view: "settings", settings: { section: "Memory", key: "global:python-preference" } });
  });

  it("says Removed for a deleted memory, with nothing to open", () => {
    render(<MemoryLine changes={[change({ action: "removed", scope: "project", name: "old-fact" })]} />);
    expect(screen.getByRole("note")).toHaveTextContent("Removed old-fact.md · project");
    expect(screen.queryByRole("button")).toBeNull();
  });

  it("puts several changes of one turn on one line, the names on expand", () => {
    render(<MemoryLine changes={[change(), change({ name: "uses-pnpm" }), change({ action: "updated", name: "deploy-branch", scope: "project", model: "claude-haiku-5-5", model_index: 1 })]} />);
    const toggle = screen.getByRole("button", { name: "Saved 2 · updated 1" });
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByRole("button", { name: /uses-pnpm\.md/ })).toBeNull();
    fireEvent.click(toggle);
    expect(toggle).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByRole("button", { name: /deploy-branch\.md/ })).toHaveAttribute("title", "memory/deploy-branch.md · by claude-haiku-5-5");
    expect(screen.getAllByRole("listitem").map(item => item.textContent)).toEqual(["Saved python-preference.md · global", "Saved uses-pnpm.md · global", "Updated deploy-branch.md · project"]);
  });

  it("comes from the memory_changed event and stays in the run", () => {
    const changes = [change({ action: "updated", scope: "project", name: "a-fact", project_root: "/p" })];
    const state = reducer(initialState, { type: "thread/event", envelope: { thread_id: "t", run_id: "r", event: { kind: "memory_changed", changes } } } as never);
    const rows = state.messages.t ?? [];
    const row = rows[rows.length - 1];
    expect(row?.memory).toEqual(changes);
    expect(row?.runId).toBe("r");
  });

  it("still reads a chat saved before memory models, whose event holds one change", () => {
    const state = reducer(initialState, { type: "thread/event", envelope: { thread_id: "t", run_id: "r", event: { kind: "memory_changed", action: "forgot", scope: "global", name: "old-pref", description: "An old preference" } } } as never);
    const rows = state.messages.t ?? [];
    expect(rows[rows.length - 1]?.memory).toEqual([{ action: "removed", scope: "global", name: "old-pref", description: "An old preference", model: "", model_index: -1 }]);
    const ignored = reducer(initialState, { type: "thread/event", envelope: { thread_id: "t", run_id: "r", event: { kind: "memory_changed" } } } as never);
    expect(ignored.messages.t ?? []).toEqual([]);
  });
});

describe("memory wording", () => {
  it("turns names into titles and dates into short ones", () => {
    expect(memoryTitle("free-model-for-tests")).toBe("Free model for tests");
    expect(memoryDate("2026-11-02", new Date(2026, 10, 2, 15))).toBe("today");
    expect(memoryDate("2026-10-29", new Date(2026, 10, 2))).toMatch(/Oct/);
    expect(memoryDate("2025-10-29", new Date(2026, 10, 2))).toMatch(/2025/);
  });
});
