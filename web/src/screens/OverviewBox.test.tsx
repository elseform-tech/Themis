// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, renderHook, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { initialState } from "../state/reducer";
import type { ChatMessage } from "../state/reducer";
import { diffTotals, OverviewBox, useOverview } from "./OverviewBox";

const dispatch = vi.fn();
let appState: typeof initialState;
vi.mock("../state/store", async importOriginal => ({
  ...await importOriginal<typeof import("../state/store")>(),
  useApp: () => ({ state: appState, dispatch }),
}));
vi.mock("../lib/browser", () => ({
  browserState: vi.fn(async () => ({ open: true, tabs: [], current: 0, url: "http://localhost:5173/notes", title: "Notes", can_go_back: false, can_go_forward: false, controller: "agent", activity: null, console_errors: 0, loading: false, muted: true })),
}));
vi.mock("../lib/tauri", () => ({
  getUsageSummary: vi.fn(async () => ({ available: true, by_chat: [{ key: "t1", totals: { estimated_cost_usd: 0.024 } }] })),
}));

const diff = { available: true, files: [
  { path: "notes/a.md", status: "added" as const, preexisting: false, hunks: [{ old_start: 0, old_lines: 0, new_start: 1, new_lines: 2, lines: [{ kind: "add" as const, text: "# A" }, { kind: "add" as const, text: "" }] }] },
  { path: "src/routes.ts", status: "modified" as const, preexisting: false, hunks: [{ old_start: 1, old_lines: 2, new_start: 1, new_lines: 2, lines: [{ kind: "context" as const, text: "x" }, { kind: "del" as const, text: "old" }, { kind: "add" as const, text: "new" }] }] },
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
    expect(diffTotals(diff)).toEqual({ files: 2, added: 3, removed: 1 });
    expect(diffTotals({ available: true, files: [] })).toBeNull();
    expect(diffTotals({ available: false, reason: "not git", files: [] })).toBeNull();
  });

  it("shows each part of the chat's workspace in one row that opens the Workspace there", async () => {
    const { result } = renderHook(() => useOverview("t1", media));
    await act(async () => {});
    const overview = result.current;
    expect(overview.visible).toBe(true);
    const openMedia = vi.fn();
    render(<OverviewBox overview={overview} onOpenMedia={openMedia} />);
    expect(screen.getByRole("button", { name: "Browser: Notes, the agent is using it" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Changes: 2 files, 3 added and 1 removed lines" })).toHaveTextContent("2 files+3 −1");
    expect(screen.getByRole("button", { name: "Tasks: 1 queued" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Cost of this chat: $0.02. Opens Settings › Usage" })).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: /^Changes/ }));
    expect(dispatch).toHaveBeenLastCalledWith({ type: "ui/diff-panel", open: true, tab: "changes" });
    fireEvent.click(screen.getByRole("button", { name: /^Browser/ }));
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

  it("folds to its icons and remembers it", async () => {
    const { result, rerender } = renderHook(() => useOverview("t1", media));
    await act(async () => {});
    act(() => result.current.setCollapsed(true));
    rerender();
    expect(result.current.collapsed).toBe(true);
    render(<OverviewBox overview={result.current} onOpenMedia={() => {}} />);
    expect(screen.getByRole("region", { name: "Chat overview" })).toHaveClass("is-collapsed");
    expect(screen.getByRole("button", { name: "Show overview details" })).toHaveAttribute("aria-expanded", "false");
    expect(localStorage.getItem("themis:overviewCollapsed")).toBe("true");
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