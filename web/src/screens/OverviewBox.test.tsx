// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, renderHook, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { initialState } from "../state/reducer";
import type { ChatMessage } from "../state/reducer";
import { diffTotals, OverviewBox, useOverview, useOverviewRoom } from "./OverviewBox";

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
    expect(overview.visible).toBe(true);
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
    expect(screen.getByRole("button", { name: "Cost of this chat: $0.02. Opens Settings › Usage" })).toBeInTheDocument();

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

  it("keeps the chat's room once the box has shown, so the conversation does not jump when a run ends", () => {
    const { result, rerender } = renderHook(({ thread, visible }) => useOverviewRoom(thread, visible), { initialProps: { thread: "t1", visible: false } });
    expect(result.current).toBe(false);
    rerender({ thread: "t1", visible: true });
    expect(result.current).toBe(true);
    rerender({ thread: "t1", visible: false });
    expect(result.current).toBe(true);
    // Another chat starts without it.
    rerender({ thread: "t2", visible: false });
    expect(result.current).toBe(false);
  });

  it("is hidden when the chat has nothing to show", async () => {
    appState = { ...initialState };
    const { browserState } = await import("../lib/browser");
    vi.mocked(browserState).mockResolvedValueOnce({ open: false, tabs: [], current: 0, url: "about:blank", title: "", can_go_back: false, can_go_forward: false, controller: "idle", activity: null, console_errors: 0, loading: false, muted: true });
    const { getUsageSummary } = await import("../lib/tauri");
    vi.mocked(getUsageSummary).mockResolvedValueOnce({ available: true, by_chat: [] } as never);
    const { result } = renderHook(() => useOverview("t2", []));
    await act(async () => {});
    expect(result.current.visible).toBe(false);
  });
});