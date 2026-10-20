// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { initialState } from "../state/reducer";
import type { WorkspaceInfo } from "../lib/types";
import { WorkspacePanel } from "./WorkspacePanel";
import * as bridge from "../lib/tauri";

const dispatch = vi.fn();
let appState: typeof initialState;
const diff = { available: true, files: [{ path: "src/a.ts", status: "modified" as const, preexisting: false, hunks: [{ old_start: 1, old_lines: 1, new_start: 1, new_lines: 1, lines: [{ kind: "del" as const, text: "old" }, { kind: "add" as const, text: "new" }] }] }] };
vi.mock("../state/store", async importOriginal => ({
  ...await importOriginal<typeof import("../state/store")>(),
  useApp: () => ({ state: appState, dispatch }),
}));
vi.mock("../lib/tauri", () => ({
  listDiff: vi.fn(async () => diff),
  listProjectDir: vi.fn(async () => ({ path: "", truncated: false, entries: [{ name: "README.md", path: "README.md", is_dir: false, size: 5 }] })),
  readProjectFile: vi.fn(async () => ({ path: "README.md", content: "hello", size: 5, truncated: false, binary: false })),
  getWorkspace: vi.fn(async () => ({ ...workspace, pull_requests_unavailable: "GitHub CLI (gh) is not installed" })),
}));
const workspace: WorkspaceInfo = { root: "/repo", name: "repo", is_git: true, branch: "main", branches: [{ name: "main", current: true, upstream: null, track: null, updated: "now" }], pull_requests: [], pull_requests_unavailable: null };
beforeEach(() => { appState = { ...initialState, diffs: { t1: diff } }; vi.mocked(bridge.listDiff).mockClear(); const store = new Map<string, string>(); vi.stubGlobal("localStorage", { getItem: (k: string) => store.get(k) ?? null, setItem: (k: string, v: string) => store.set(k, v) }); });
afterEach(() => { cleanup(); vi.unstubAllGlobals(); });

describe("WorkspacePanel", () => {
  it("monitors changes read-only and shows files, branches and pull request availability", async () => {
    render(<WorkspacePanel projectRoot="/repo" threadId="t1" workspace={workspace} />);
    expect(screen.getByRole("complementary", { name: "Workspace (read-only)" })).toBeInTheDocument();
    expect(screen.queryByRole("checkbox")).toBeNull();
    expect(screen.queryByRole("progressbar")).toBeNull();
    expect(screen.getByText("new")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /^Accept|^Discard|Merge|Comment|Refresh|Unified|Side-by-side/ })).toBeNull();
    const separator = screen.getByRole("separator", { name: "Resize workspace panel" });
    fireEvent.keyDown(separator, { key: "ArrowLeft" });
    expect(separator).toHaveAttribute("aria-valuenow", "600");
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Files" })));
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "README.md" })));
    expect(screen.getByText("hello")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Branches" }));
    expect(screen.getByRole("list", { name: "Branches" })).toHaveTextContent("maincurrent");
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Pull requests" })));
    expect(screen.getByText("GitHub CLI (gh) is not installed")).toBeInTheDocument();
  });
  it("resizes from a narrow 300px panel up to a wide one, and remembers the diff split", () => {
    render(<WorkspacePanel projectRoot="/repo" threadId="t1" workspace={workspace} />);
    const panel = screen.getByRole("separator", { name: "Resize workspace panel" });
    expect(panel).toHaveAttribute("aria-valuemin", "300");
    expect(panel).toHaveAttribute("aria-valuemax", "1600");
    for (let step = 0; step < 10; step += 1) fireEvent.keyDown(panel, { key: "ArrowRight" });
    expect(panel).toHaveAttribute("aria-valuenow", "300");
    const split = screen.getByRole("separator", { name: "Resize file list" });
    expect(split).toHaveAttribute("aria-valuenow", "200");
    fireEvent.keyDown(split, { key: "ArrowRight" });
    expect(split).toHaveAttribute("aria-valuenow", "240");
    expect(localStorage.getItem("themis:workspaceDiffListWidth")).toBe("240");
    for (let step = 0; step < 10; step += 1) fireEvent.keyDown(split, { key: "ArrowLeft" });
    expect(split).toHaveAttribute("aria-valuenow", "120");
    fireEvent.doubleClick(split);
    expect(split).toHaveAttribute("aria-valuenow", "200");
  });
  it("refreshes the diff live: after each completed tool call and every few seconds during a run", async () => {
    vi.useFakeTimers();
    appState = { ...appState, running: { t1: true }, traces: { t1: [] } };
    const view = render(<WorkspacePanel projectRoot="/repo" threadId="t1" workspace={workspace} />);
    await act(async () => {});
    const calls = () => vi.mocked(bridge.listDiff).mock.calls.length;
    const initial = calls();
    expect(initial).toBeGreaterThan(0);
    await act(async () => { vi.advanceTimersByTime(4000); });
    expect(calls()).toBe(initial + 1);
    appState = { ...appState, traces: { t1: [{ id: "x", tool: "write_file", summary: "write", ok: true }] } };
    view.rerender(<WorkspacePanel projectRoot="/repo" threadId="t1" workspace={workspace} />);
    await act(async () => {});
    expect(calls()).toBe(initial + 2);
    vi.useRealTimers();
  });
  it("shows a Tasks tab with a count and opens it when the indicator asks", async () => {
    const feed = { thread_id: "t1", run: { running: true, awaiting_approval: false }, automations: [], recent: [] };
    appState = { ...appState, tasks: { t1: feed }, workspaceTabRequest: { tab: "tasks", at: 1 } };
    render(<WorkspacePanel projectRoot="/repo" threadId="t1" workspace={workspace} />);
    expect(screen.getByRole("button", { name: /^Tasks1$/ })).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByRole("region", { name: "Running" })).toBeInTheDocument();
  });
  it("applies a tab request once so the remembered tab wins when reopened", async () => {
    appState = { ...appState, workspaceTabRequest: { tab: "tasks", at: 1 } };
    const first = render(<WorkspacePanel projectRoot="/repo" threadId="t1" workspace={workspace} />);
    expect(dispatch).toHaveBeenCalledWith({ type: "ui/tab-request-consumed" });
    // The user moves on to Files, closes the panel and reopens it with no request pending.
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Files" })));
    first.unmount();
    appState = { ...appState, workspaceTabRequest: null };
    render(<WorkspacePanel projectRoot="/repo" threadId="t1" workspace={workspace} />);
    expect(screen.getByRole("button", { name: "Files" })).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByRole("button", { name: /^Tasks/ })).toHaveAttribute("aria-pressed", "false");
  });
  it("opens the file an edited-files row asks for", async () => {
    const second = { ...diff.files[0]!, path: "docs/guide.md" };
    appState = { ...appState, diffs: { t1: { ...diff, files: [...diff.files, second] } }, diffFocus: { path: "docs/guide.md", at: 1 } };
    const { container } = render(<WorkspacePanel projectRoot="/repo" threadId="t1" workspace={workspace} />);
    await act(async () => {});
    expect(container.querySelector(".themis-diff-path")).toHaveTextContent("docs/guide.md");
  });
});
