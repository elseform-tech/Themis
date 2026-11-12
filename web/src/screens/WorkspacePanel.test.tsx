// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { initialState } from "../state/reducer";
import type { WorkspaceInfo } from "../lib/types";
import { WorkspacePanel } from "./WorkspacePanel";
import * as bridge from "../lib/tauri";

const dispatch = vi.fn();
let appState: typeof initialState;
const diff = { available: true, files: [{ path: "src/a.ts", status: "modified" as const, preexisting: false, added: 0, removed: 0, hunks: [{ old_start: 1, old_lines: 1, new_start: 1, new_lines: 1, lines: [{ kind: "del" as const, text: "old" }, { kind: "add" as const, text: "new" }] }] }] };
vi.mock("../state/store", async importOriginal => ({
  ...await importOriginal<typeof import("../state/store")>(),
  useApp: () => ({ state: appState, dispatch }),
}));
vi.mock("../lib/tauri", () => ({
  listDiff: vi.fn(async () => diff),
  branchDiff: vi.fn(),
  listProjectDir: vi.fn(async () => ({ path: "", truncated: false, entries: [{ name: "README.md", path: "README.md", is_dir: false, size: 5 }] })),
  readProjectFile: vi.fn(async () => ({ path: "README.md", content: "hello", size: 5, truncated: false, binary: false })),
  readBranchFile: vi.fn(),
  getWorkspace: vi.fn(async () => ({ ...workspace, pull_requests_unavailable: "GitHub CLI (gh) is not installed" })),
}));
const workspace: WorkspaceInfo = { root: "/repo", name: "repo", is_git: true, branch: "main", branches: [{ name: "main", current: true, upstream: null, track: null, updated: "now" }], pull_requests: [], pull_requests_unavailable: null };
beforeEach(() => { appState = { ...initialState, diffs: { t1: diff } }; vi.mocked(bridge.listDiff).mockClear(); const store = new Map<string, string>(); vi.stubGlobal("localStorage", { getItem: (k: string) => store.get(k) ?? null, setItem: (k: string, v: string) => store.set(k, v) }); const session = new Map<string, string>(); vi.stubGlobal("sessionStorage", { getItem: (k: string) => session.get(k) ?? null, setItem: (k: string, v: string) => session.set(k, v) }); });
afterEach(() => { cleanup(); vi.unstubAllGlobals(); });

describe("WorkspacePanel", () => {
  it("monitors changes read-only and shows files and pull request availability", async () => {
    render(<WorkspacePanel projectRoot="/repo" threadId="t1" workspace={workspace} />);
    expect(screen.getByRole("complementary", { name: "Workspace (read-only)" })).toBeInTheDocument();
    expect(screen.queryByRole("checkbox")).toBeNull();
    expect(screen.queryByRole("progressbar")).toBeNull();
    expect(screen.getByText("new")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /^Accept|^Discard|Merge|Comment|Refresh/ })).toBeNull();
    const separator = screen.getByRole("separator", { name: "Resize workspace panel" });
    fireEvent.keyDown(separator, { key: "ArrowLeft" });
    expect(separator).toHaveAttribute("aria-valuenow", "680");
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Files" })));
    // The file view sits beside the tree in the content column, not in the divider's.
    const tree = screen.getByRole("list", { name: "Project files" });
    expect(tree.nextElementSibling).toHaveAttribute("role", "separator");
    expect(tree.nextElementSibling?.nextElementSibling).toHaveTextContent("Pick a file");
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "README.md" })));
    expect(screen.getByText("hello")).toBeInTheDocument();
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Pull requests" })));
    expect(screen.getByText("GitHub CLI (gh) is not installed")).toBeInTheDocument();
  });
  it("falls back to Changes when the remembered tab no longer exists", async () => {
    window.localStorage.setItem("themis:workspaceTab", JSON.stringify("branches"));
    render(<WorkspacePanel projectRoot="/repo" threadId="t1" workspace={workspace} />);
    await act(async () => {});
    expect(screen.getByRole("button", { name: /^Changes/ })).toHaveAttribute("aria-pressed", "true");
  });
  it("keeps a lean header: icon tabs named by tooltips, and Close", () => {
    render(<WorkspacePanel projectRoot="/repo" threadId="t1" workspace={workspace} />);
    const head = document.querySelector(".themis-workspace-head")!;
    const nav = head.querySelector("nav[aria-label='Workspace views']")!;
    // Each tab is an icon whose name is its tooltip; the only text is a change count.
    for (const name of ["Changes 1", "Files", "Pull requests", "Tasks", "Browser"]) {
      const tab = screen.getByRole("button", { name });
      expect(nav.contains(tab)).toBe(true);
      expect(tab.querySelector("svg")).not.toBeNull();
      expect(tab.parentElement).toHaveAttribute("data-tooltip", name.replace(/ \d+$/, ""));
    }
    expect(nav).toHaveTextContent(/^1$/);
    // The composer already shows the project and branch; the header does not repeat them.
    expect(head).not.toHaveTextContent("repo");
    expect(head).not.toHaveTextContent("main");
    const close = screen.getByRole("button", { name: "Close workspace" });
    expect(head.contains(close)).toBe(true);
    // Close sits at the far right of the tab row.
    expect(head.lastElementChild?.contains(close)).toBe(true);
    expect(close).not.toHaveTextContent("✕");
    fireEvent.click(close);
    expect(dispatch).toHaveBeenCalledWith({ type: "ui/diff-panel", open: false });
  });
  it("resizes from a narrow 300px panel up to a wide one, and remembers the diff split", () => {
    render(<WorkspacePanel projectRoot="/repo" threadId="t1" workspace={workspace} />);
    const panel = screen.getByRole("separator", { name: "Resize workspace panel" });
    expect(panel).toHaveAttribute("aria-valuemin", "300");
    expect(panel).toHaveAttribute("aria-valuemax", "1600");
    for (let step = 0; step < 10; step += 1) fireEvent.keyDown(panel, { key: "ArrowRight" });
    expect(panel).toHaveAttribute("aria-valuenow", "300");
    // Changes stacks its file list above the diff (canvas); the Files tab keeps its resizable tree beside the file.
    const changes = document.querySelector(".themis-workspace-changes")!;
    const list = changes.querySelector(".themis-filetree-wrap")!;
    expect(list.compareDocumentPosition(changes.querySelector(".themis-diff")!) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(screen.queryByRole("separator", { name: "Resize file list" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Files" }));
    const split = screen.getByRole("separator", { name: "Resize file list" });
    expect(split).toHaveAttribute("aria-valuenow", "180");
    fireEvent.keyDown(split, { key: "ArrowRight" });
    expect(split).toHaveAttribute("aria-valuenow", "220");
    expect(localStorage.getItem("themis:workspaceFileListWidth")).toBe("220");
    for (let step = 0; step < 10; step += 1) fireEvent.keyDown(split, { key: "ArrowLeft" });
    expect(split).toHaveAttribute("aria-valuenow", "120");
    fireEvent.doubleClick(split);
    expect(split).toHaveAttribute("aria-valuenow", "180");
    // Dragging a divider never starts a text selection (WebKit selects on mousedown).
    expect(fireEvent.mouseDown(panel)).toBe(false);
    expect(fireEvent.mouseDown(split)).toBe(false);
  });
  it("refreshes the diff right after each completed tool call", async () => {
    vi.useFakeTimers();
    appState = { ...appState, running: { t1: true }, traces: { t1: [] } };
    const view = render(<WorkspacePanel projectRoot="/repo" threadId="t1" workspace={workspace} />);
    await act(async () => {});
    const calls = () => vi.mocked(bridge.listDiff).mock.calls.length;
    const initial = calls();
    expect(initial).toBeGreaterThan(0);
    appState = { ...appState, traces: { t1: [{ id: "x", tool: "write_file", summary: "write", ok: true }] } };
    view.rerender(<WorkspacePanel projectRoot="/repo" threadId="t1" workspace={workspace} />);
    await act(async () => {});
    expect(calls()).toBe(initial + 1);
    vi.useRealTimers();
  });
  it("shows what another branch adds in Changes, without switching anything", async () => {
    const info = { ...workspace, is_git: true, branch: "main", branches: [
      { name: "main", current: true, upstream: null, track: null, updated: "now" },
      { name: "feature", current: false, upstream: null, track: null, updated: "now" },
    ] };
    vi.mocked(bridge.branchDiff).mockResolvedValue({ available: true, reason: null, files: [{ path: "new.txt", status: "added", added: 1, removed: 0, hunks: [] }] } as never);
    render(<WorkspacePanel projectRoot="/repo" threadId="t1" workspace={info as never} />);
    await act(async () => {});
    expect(screen.queryByRole("button", { name: "Branches" })).toBeNull();
    const source = screen.getByRole("combobox", { name: "Show changes from" });
    expect([...source.querySelectorAll("option")].map(option => option.textContent)).toEqual(["Uncommitted changes on main", "feature"]);
    await act(async () => { fireEvent.change(source, { target: { value: "feature" } }); });
    expect(bridge.branchDiff).toHaveBeenCalledWith("/repo", "feature");
    expect(screen.getAllByText("new.txt").length).toBeGreaterThan(0);
    expect(screen.queryByText("src/a.ts")).toBeNull();
    expect(screen.queryByRole("button", { name: /switch/i })).toBeNull();
    await act(async () => { fireEvent.change(source, { target: { value: "" } }); });
    expect(screen.queryByText("new.txt")).toBeNull();
  });
  it("drops a branch's comparison when that branch becomes the current one, and rereads the others", async () => {
    const branches = (current: string) => ({ ...workspace, is_git: true, branch: current, branches: ["main", "feature", "other"].map(name => ({ name, current: name === current, upstream: null, track: null, updated: "now" })) });
    vi.mocked(bridge.branchDiff).mockClear();
    vi.mocked(bridge.branchDiff).mockResolvedValue({ available: true, reason: null, files: [{ path: "new.txt", status: "added", added: 1, removed: 0, hunks: [] }] } as never);
    const view = render(<WorkspacePanel projectRoot="/repo" threadId="t1" workspace={branches("main") as never} />);
    await act(async () => {});
    await act(async () => { fireEvent.change(screen.getByRole("combobox", { name: "Show changes from" }), { target: { value: "other" } }); });
    expect(bridge.branchDiff).toHaveBeenCalledTimes(1);
    await act(async () => { view.rerender(<WorkspacePanel projectRoot="/repo" threadId="t1" workspace={branches("feature") as never} />); });
    expect(bridge.branchDiff).toHaveBeenCalledTimes(2);
    await act(async () => { view.rerender(<WorkspacePanel projectRoot="/repo" threadId="t1" workspace={branches("other") as never} />); });
    expect(screen.getByRole("combobox", { name: "Show changes from" })).toHaveValue("");
  });
  it("shows a Tasks tab with a count and opens it when the indicator asks", async () => {
    const feed = { thread_id: "t1", run: { running: true, awaiting_approval: false }, automations: [], recent: [] };
    appState = { ...appState, tasks: { t1: feed }, workspaceTabRequest: { tab: "tasks", at: 1 } };
    render(<WorkspacePanel projectRoot="/repo" threadId="t1" workspace={workspace} />);
    expect(screen.getByRole("button", { name: "Tasks 1" })).toHaveAttribute("aria-pressed", "true");
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
    render(<WorkspacePanel projectRoot="/repo" threadId="t1" workspace={workspace} />);
    await act(async () => {});
    expect(screen.getByRole("region", { name: "Changes in docs/guide.md" })).toBeInTheDocument();
  });
  it("keeps hidden changes out of the count and the opened diff, behind one line", async () => {
    const hidden = { ...diff.files[0]!, path: ".github/workflows/ci.yml", hidden: true };
    const files = [hidden, diff.files[0]!];
    vi.mocked(bridge.listDiff).mockImplementation(async () => ({ ...diff, files }) as never);
    appState = { ...appState, diffs: { t1: { ...diff, files } as never } };
    render(<WorkspacePanel projectRoot="/repo" threadId="t1" workspace={workspace} />);
    await act(async () => {});
    expect(screen.getByRole("button", { name: "Changes 1" })).toBeInTheDocument();
    expect(screen.getByRole("region", { name: "Changes in src/a.ts" })).toBeInTheDocument();
    expect(screen.queryByTitle(".github/workflows/ci.yml")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "1 hidden file changed" }));
    fireEvent.click(screen.getByTitle(".github/workflows/ci.yml"));
    expect(screen.getByRole("region", { name: "Changes in .github/workflows/ci.yml" })).toBeInTheDocument();
    vi.mocked(bridge.listDiff).mockImplementation(async () => diff);
  });
  it("says only hidden files changed instead of opening one", async () => {
    const files = [{ ...diff.files[0]!, path: ".gitignore", hidden: true }];
    vi.mocked(bridge.listDiff).mockImplementation(async () => ({ ...diff, files }) as never);
    appState = { ...appState, diffs: { t1: { ...diff, files } as never } };
    const { container } = render(<WorkspacePanel projectRoot="/repo" threadId="t1" workspace={workspace} />);
    await act(async () => {});
    expect(screen.getByRole("button", { name: "Changes" })).toBeInTheDocument();
    expect(container.querySelector(".themis-diff")).toBeNull();
    expect(screen.getByText("Only hidden files changed.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "1 hidden file changed" })).toBeInTheDocument();
    vi.mocked(bridge.listDiff).mockImplementation(async () => diff);
  });
  it("filters Changes by author once a subagent has write paths", async () => {
    const files = [{ ...diff.files[0], path: "src/a.ts" }, { ...diff.files[0], path: "notes/b.md" }];
    vi.mocked(bridge.listDiff).mockImplementation(async () => ({ ...diff, files }) as never);
    appState = { ...appState, diffs: { t1: { ...diff, files } as never }, messages: { t1: [{ id: "s", role: "system", text: "", subagents: [{ id: "a", name: "Writer", brief: "b", model: "m", confined: true, writePaths: ["notes/"], status: "running", text: "", toolCalls: 0 }] }] } as never };
    render(<WorkspacePanel projectRoot="/repo" threadId="t1" workspace={workspace} />);
    await act(async () => {});
    const select = screen.getByRole("combobox", { name: "Author" });
    expect(screen.getByTitle("notes/b.md")).toBeInTheDocument();
    fireEvent.change(select, { target: { value: "Main" } });
    expect(screen.queryByTitle("notes/b.md")).toBeNull();
    fireEvent.change(select, { target: { value: "Writer" } });
    expect(screen.queryByTitle("src/a.ts")).toBeNull();
    expect(screen.getAllByTitle("notes/b.md").length).toBeGreaterThan(0);
    vi.mocked(bridge.listDiff).mockImplementation(async () => diff);
  });
});

describe("Workspace › Changes accordion", () => {
  const changed = (count: number) => ({ available: true, files: Array.from({ length: count }, (_, i) => ({ ...diff.files[0]!, path: `src/f${i}.ts`, added: i + 1, removed: 1 })) });
  const open = () => screen.queryAllByRole("region").map(region => region.getAttribute("aria-label"));
  const row = (path: string) => screen.getByRole("button", { name: new RegExp(`^${path.replace(".", "\\.")}`) });
  async function show(count: number) {
    const state = changed(count);
    vi.mocked(bridge.listDiff).mockImplementation(async () => state as never);
    appState = { ...appState, diffs: { t1: state as never } };
    const view = render(<WorkspacePanel projectRoot="/repo" threadId="t1" workspace={workspace} />);
    await act(async () => {});
    return view;
  }
  afterEach(() => vi.mocked(bridge.listDiff).mockImplementation(async () => diff));

  it("gives each changed file its own foldable diff: all open for five or fewer, the first for more", async () => {
    await show(5);
    expect(open()).toEqual([0, 1, 2, 3, 4].map(i => `Changes in src/f${i}.ts`));
    cleanup();
    await show(6);
    expect(open()).toEqual(["Changes in src/f0.ts"]);
    expect(row("src/f1.ts")).toHaveAttribute("aria-expanded", "false");
  });

  it("folds and unfolds from the file's row, several at once, and remembers it for the chat", async () => {
    const view = await show(3);
    fireEvent.click(row("src/f1.ts"));
    expect(row("src/f1.ts")).toHaveAttribute("aria-expanded", "false");
    expect(open()).toEqual(["Changes in src/f0.ts", "Changes in src/f2.ts"]);
    view.unmount();
    await show(3);
    expect(open()).toEqual(["Changes in src/f0.ts", "Changes in src/f2.ts"]);
  });

  it("collapses and expands every file from the bar, shown only when there is more than one", async () => {
    await show(3);
    fireEvent.click(screen.getByRole("button", { name: "Collapse all files" }));
    expect(open()).toEqual([]);
    fireEvent.click(screen.getByRole("button", { name: "Expand all files" }));
    expect(open()).toHaveLength(3);
    cleanup();
    await show(1);
    expect(screen.queryByRole("button", { name: "Collapse all files" })).toBeNull();
  });

  it("opens the file an edited-files row asks for, even when it was folded", async () => {
    const state = changed(6);
    appState = { ...appState, diffs: { t1: state as never }, diffFocus: { path: "src/f4.ts", at: 1 } };
    vi.mocked(bridge.listDiff).mockImplementation(async () => state as never);
    render(<WorkspacePanel projectRoot="/repo" threadId="t1" workspace={workspace} />);
    await act(async () => {});
    expect(open()).toContain("Changes in src/f4.ts");
  });

  it("reads the working tree's file for the unchanged lines of uncommitted changes", async () => {
    const long = { available: true, files: [{ ...diff.files[0]!, path: "src/long.ts", hunks: [{ old_start: 30, old_lines: 1, new_start: 30, new_lines: 1, lines: [{ kind: "del" as const, text: "old" }, { kind: "add" as const, text: "new" }] }] }] };
    vi.mocked(bridge.listDiff).mockImplementation(async () => long as never);
    vi.mocked(bridge.readProjectFile).mockResolvedValueOnce({ path: "src/long.ts", content: Array.from({ length: 40 }, (_, i) => `n${i + 1}`).join("\n"), size: 1, truncated: false, binary: false });
    appState = { ...appState, diffs: { t1: long as never } };
    render(<WorkspacePanel projectRoot="/repo" threadId="t1" workspace={workspace} />);
    await act(async () => {});
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "⋯ 29 unchanged lines" })));
    expect(bridge.readProjectFile).toHaveBeenCalledWith("/repo", "src/long.ts");
    expect(screen.getByRole("button", { name: "⋯ 9 unchanged lines" })).toBeInTheDocument();
  });
});
