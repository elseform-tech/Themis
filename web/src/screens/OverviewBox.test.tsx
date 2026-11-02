// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, renderHook, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { initialState } from "../state/reducer";
import type { ChatMessage } from "../state/reducer";
import type { TodoItem, TodoStatus } from "../lib/types";
import { diffTotals, OverviewBox, useOverview } from "./OverviewBox";

const dispatch = vi.fn();
let appState: typeof initialState;
vi.mock("../state/store", async importOriginal => ({
  ...await importOriginal<typeof import("../state/store")>(),
  useApp: () => ({ state: appState, dispatch }),
}));
let streamUpdate: ((update: unknown) => void) | null = null;
vi.mock("../lib/browser", () => ({
  browserStream: vi.fn((_thread: string, onUpdate: (update: unknown) => void) => { streamUpdate = onUpdate; return () => { streamUpdate = null; }; }),
  browserPreview: vi.fn(async () => undefined),
  browserScreencastStop: vi.fn(async () => undefined),
  browserState: vi.fn(async () => ({ open: true, tabs: [], current: 0, url: "http://localhost:5173/notes", title: "Notes", can_go_back: false, can_go_forward: false, controller: "agent", activity: null, console_errors: 0, loading: false, muted: true })),
}));
vi.mock("../lib/tauri", () => ({
  getUsageSummary: vi.fn(async () => ({ available: true, by_chat: [{ key: "t1", totals: { estimated_cost_usd: 0.024 } }] })),
}));

const diff = { available: true, files: [
  { path: "notes/a.md", status: "added" as const, preexisting: false, added: 2, removed: 0, hunks: [{ old_start: 0, old_lines: 0, new_start: 1, new_lines: 2, lines: [{ kind: "add" as const, text: "# A" }, { kind: "add" as const, text: "" }] }] },
  { path: "src/routes.ts", status: "modified" as const, preexisting: false, added: 1, removed: 1, hunks: [{ old_start: 1, old_lines: 2, new_start: 1, new_lines: 2, lines: [{ kind: "context" as const, text: "x" }, { kind: "del" as const, text: "old" }, { kind: "add" as const, text: "new" }] }] },
] };
const media: ChatMessage[] = [{ id: "m1", role: "assistant", text: "Here", final: true, media: [{ path: "/data/shot.png", name: "shot.png", kind: "image" }] }];

beforeEach(() => {
  dispatch.mockClear();
  appState = { ...initialState, diffs: { t1: diff }, queues: { t1: [{ id: "q", text: "later", held: false } as never] } };
  const store = new Map<string, string>();
  vi.stubGlobal("localStorage", { getItem: (k: string) => store.get(k) ?? null, setItem: (k: string, v: string) => store.set(k, v) });
});
afterEach(() => { cleanup(); vi.unstubAllGlobals(); });

describe("chat overview box", () => {
  it("adds up the changed lines and shows nothing without changes", () => {
    // From each file's own totals: a very large file's listing holds only its first lines.
    const counted = { ...diff, files: [{ ...diff.files[0]!, added: 25000, removed: 1 }, { ...diff.files[1]!, added: 2, removed: 0 }] };
    expect(diffTotals(counted)).toEqual({ files: 2, added: 25002, removed: 1 });
    expect(diffTotals({ available: true, files: [] })).toBeNull();
    expect(diffTotals({ available: false, reason: "not git", files: [] })).toBeNull();
  });

  it("shows each part of the chat's workspace in one row that opens the Workspace there", async () => {
    const { result } = renderHook(() => useOverview("t1", media));
    await act(async () => {});
    const overview = result.current;
    const openMedia = vi.fn();
    render(<OverviewBox threadId="t1" overview={overview} onOpenMedia={openMedia} />);
    // The browser shows as a live preview: small frames, the page left at its size.
    const { browserPreview } = await import("../lib/browser");
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 300)); });
    expect(browserPreview).toHaveBeenCalledWith("t1", 440);
    const preview = screen.getByRole("button", { name: "Browser: Notes, the agent is using it. Opens the Browser tab" });
    act(() => {
      streamUpdate?.({ kind: "frame", data: "AAAA", width: 440, height: 275 });
      streamUpdate?.({ kind: "state", state: { controller: "agent", activity: "clicking “Save”" } });
    });
    expect(preview.querySelector("img")).toHaveAttribute("src", "data:image/jpeg;base64,AAAA");
    expect(preview).toHaveTextContent("Agent is clicking “Save”");
    expect(screen.getByRole("button", { name: "Changes: 2 files, 3 added and 1 removed lines" })).toHaveTextContent("2 files+3 −1");
    expect(screen.getByRole("button", { name: "Tasks: 1 queued" })).toBeInTheDocument();
    // Cost lives in Settings › Usage only.
    expect(screen.queryByRole("button", { name: /Cost of this chat/ })).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: /^Changes/ }));
    expect(dispatch).toHaveBeenLastCalledWith({ type: "ui/diff-panel", open: true, tab: "changes" });
    fireEvent.click(preview);
    expect(dispatch).toHaveBeenLastCalledWith({ type: "ui/diff-panel", open: true, tab: "browser" });
    fireEvent.click(screen.getByRole("button", { name: /^Tasks/ }));
    expect(dispatch).toHaveBeenLastCalledWith({ type: "ui/diff-panel", open: true, tab: "tasks" });
    // The Workspace icon opens it on the tab it remembers.
    fireEvent.click(screen.getByRole("button", { name: "Open Workspace" }));
    expect(dispatch).toHaveBeenLastCalledWith({ type: "ui/diff-panel", open: true });
    // An artifact opens in the chat's media viewer.
    fireEvent.click(screen.getByRole("button", { name: /^Artifacts: 1/ }));
    expect(openMedia).toHaveBeenCalledWith("/data/shot.png");
  });

  it("asks for the preview again when no frame arrives (Workspace's stop can land after its start)", async () => {
    const { result } = renderHook(() => useOverview("t1", media));
    await act(async () => {});
    const { browserPreview } = await import("../lib/browser");
    vi.mocked(browserPreview).mockClear();
    vi.useFakeTimers();
    try {
      render(<OverviewBox threadId="t1" overview={result.current} onOpenMedia={() => {}} />);
      expect(browserPreview).not.toHaveBeenCalled();
      act(() => { vi.advanceTimersByTime(300); });
      expect(browserPreview).toHaveBeenCalledTimes(1);
      act(() => { vi.advanceTimersByTime(2000); });
      expect(browserPreview).toHaveBeenCalledTimes(2);
    } finally { vi.useRealTimers(); }
  });

  it("folds to its icons and remembers it", async () => {
    const { result, rerender } = renderHook(() => useOverview("t1", media));
    await act(async () => {});
    act(() => result.current.setCollapsed(true));
    rerender();
    expect(result.current.collapsed).toBe(true);
    render(<OverviewBox threadId="t1" overview={result.current} onOpenMedia={() => {}} />);
    expect(screen.getByRole("region", { name: "Chat overview" })).toHaveClass("is-collapsed");
    expect(screen.getByRole("button", { name: "Show overview details" })).toHaveAttribute("aria-expanded", "false");
    expect(localStorage.getItem("themis:overviewCollapsed")).toBe("true");
  });

  it("has nothing to show in a chat with nothing to list", async () => {
    appState = { ...initialState };
    const { browserState } = await import("../lib/browser");
    vi.mocked(browserState).mockResolvedValueOnce({ open: false, tabs: [], current: 0, url: "about:blank", title: "", can_go_back: false, can_go_forward: false } as never);
    const { getUsageSummary } = await import("../lib/tauri");
    vi.mocked(getUsageSummary).mockResolvedValueOnce({ available: true, by_chat: [] } as never);
    const { result } = renderHook(() => useOverview("t2", []));
    await act(async () => {});
    expect(result.current.visible).toBe(false);
  });
});

describe("the to-do list in the overview box", () => {
  beforeEach(() => { dispatch.mockClear(); });
  const overview = { changes: null, tasks: null, subagents: null, todo: null, browser: null, media: [], visible: true, collapsed: false, setCollapsed: vi.fn() };
  const list = (statuses: TodoStatus[]): TodoItem[] => statuses.map((status, index) => ({ id: `i${index}`, title: `Item ${index + 1}`, status }));
  const rows = () => [...document.querySelectorAll(".themis-overview-todo .themis-todo-item")].map(row => row.textContent);
  const folds = () => [...document.querySelectorAll(".themis-overview-todo .themis-todo-more")].map(row => row.textContent);

  it("comes from the chat's list, hidden while the list is empty", async () => {
    appState = { ...initialState, todos: { t1: { revision: 1, items: list(["completed", "in_progress"]) } } };
    const { result } = renderHook(() => useOverview("t1", []));
    await act(async () => {});
    expect(result.current.todo?.items).toHaveLength(2);
    expect(result.current.visible).toBe(true);
    appState = { ...initialState, todos: { t1: { revision: 2, items: [] } } };
    const { browserState } = await import("../lib/browser");
    vi.mocked(browserState).mockResolvedValueOnce({ open: false, url: "about:blank" } as never);
    const empty = renderHook(() => useOverview("t1", []));
    await act(async () => {});
    expect(empty.result.current.todo).toBeNull();
    expect(empty.result.current.visible).toBe(false);
  });

  it("shows the current item with two before it, folds earlier finished items and what comes after", () => {
    appState = initialState;
    const items = list(["completed", "completed", "completed", "completed", "in_progress", "pending", "pending", "pending", "pending"]);
    render(<OverviewBox threadId="t1" overview={{ ...overview, todo: { items, broken: false } }} onOpenMedia={vi.fn()} />);
    expect(document.querySelector(".themis-overview-todo-head")).toHaveTextContent("To-do4 of 9");
    expect(folds()).toEqual(["2 done", "2 more"]);
    expect(rows().map(text => text?.replace(/^(Done|In progress): /, ""))).toEqual(["Item 3", "Item 4", "Item 5", "Item 6", "Item 7"]);
    // Only the current item spins; finished ones are crossed off with no tick.
    const shown = [...document.querySelectorAll(".themis-overview-todo .themis-todo-item")];
    expect(shown.map(row => row.querySelector(".themis-dot-spinner") !== null)).toEqual([false, false, true, false, false]);
    expect(shown[0]).toHaveClass("is-done");
    expect(shown[0]!.querySelector("svg")).toBeNull();
    // The whole section opens the To-do tab; it names the current item for a screen reader.
    fireEvent.click(screen.getByRole("button", { name: "To-do: 4 of 9 done, now Item 5. Opens the To-do tab" }));
    expect(dispatch).toHaveBeenLastCalledWith({ type: "ui/diff-panel", open: true, tab: "todo" });
  });

  it("starts at the first pending item when none is in progress, and needs no folds for a short list", () => {
    appState = initialState;
    render(<OverviewBox threadId="t1" overview={{ ...overview, todo: { items: list(["completed", "pending", "pending"]), broken: false } }} onOpenMedia={vi.fn()} />);
    expect(folds()).toEqual([]);
    expect(rows()).toHaveLength(3);
    expect(document.querySelector(".themis-overview-todo .themis-dot-spinner")).toBeNull();
  });

  it("collapses a finished list to one quiet line", () => {
    appState = initialState;
    render(<OverviewBox threadId="t1" overview={{ ...overview, todo: { items: list(Array(8).fill("completed")), broken: false } }} onOpenMedia={vi.fn()} />);
    expect(rows()).toEqual([]);
    expect(folds()).toEqual(["8 of 8 done"]);
    // The line says it; the header doesn't repeat the count.
    expect(document.querySelector(".themis-overview-todo-count")).toBeNull();
    expect(screen.getByRole("button", { name: "To-do: 8 of 8 done. Opens the To-do tab" })).toBeInTheDocument();
  });

  it("counts open items before the current one instead of dropping them", () => {
    appState = initialState;
    // The agent put an item in progress below items it hasn't started.
    const items = list(["pending", "pending", "completed", "pending", "pending", "pending", "pending", "in_progress", "pending", "pending"]);
    render(<OverviewBox threadId="t1" overview={{ ...overview, todo: { items, broken: false } }} onOpenMedia={vi.fn()} />);
    expect(folds()).toEqual(["1 done · 4 to do"]);
    expect(rows().map(text => text?.replace(/^(Done|In progress): /, ""))).toEqual(["Item 6", "Item 7", "Item 8", "Item 9", "Item 10"]);
  });

  it("never shows more than five items", () => {
    appState = initialState;
    render(<OverviewBox threadId="t1" overview={{ ...overview, todo: { items: list(Array(12).fill("pending")), broken: false } }} onOpenMedia={vi.fn()} />);
    expect(rows()).toHaveLength(5);
    expect(folds()).toEqual(["7 more"]);
  });

  it("folds to one icon row that spins while an item is in progress, with a dot for a broken rule", () => {
    appState = initialState;
    render(<OverviewBox threadId="t1" overview={{ ...overview, collapsed: true, todo: { items: list(["completed", "in_progress"]), broken: true } }} onOpenMedia={vi.fn()} />);
    expect(document.querySelector(".themis-overview-todo")).toBeNull();
    const row = screen.getByRole("button", { name: "To-do: 1 of 2 done, a rule wasn't followed" });
    expect(row.querySelector(".themis-dot-spinner")).not.toBeNull();
    expect(row.querySelector(".themis-overview-dot.is-attention")).not.toBeNull();
    fireEvent.click(row);
    expect(dispatch).toHaveBeenLastCalledWith({ type: "ui/diff-panel", open: true, tab: "todo" });
  });
});
