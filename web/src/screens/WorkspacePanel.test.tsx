// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { initialState } from "../state/reducer";
import type { WorkspaceInfo } from "../lib/types";
import { WorkspacePanel } from "./WorkspacePanel";

const dispatch = vi.fn();
const diff = { available: true, files: [{ path: "src/a.ts", status: "modified" as const, preexisting: false, hunks: [{ old_start: 1, old_lines: 1, new_start: 1, new_lines: 1, lines: [{ kind: "del" as const, text: "old" }, { kind: "add" as const, text: "new" }] }] }] };
vi.mock("../state/store", async importOriginal => ({
  ...await importOriginal<typeof import("../state/store")>(),
  useApp: () => ({ state: { ...initialState, diffs: { t1: diff } }, dispatch }),
}));
vi.mock("../lib/tauri", () => ({
  listDiff: vi.fn(async () => diff),
  listProjectDir: vi.fn(async () => ({ path: "", truncated: false, entries: [{ name: "README.md", path: "README.md", is_dir: false, size: 5 }] })),
  readProjectFile: vi.fn(async () => ({ path: "README.md", content: "hello", size: 5, truncated: false, binary: false })),
  getWorkspace: vi.fn(async () => ({ ...workspace, pull_requests_unavailable: "GitHub CLI (gh) is not installed" })),
}));
const workspace: WorkspaceInfo = { root: "/repo", name: "repo", is_git: true, branch: "main", branches: [{ name: "main", current: true, upstream: null, track: null, updated: "now" }], pull_requests: [], pull_requests_unavailable: null };
beforeEach(() => { const store = new Map<string, string>(); vi.stubGlobal("localStorage", { getItem: (k: string) => store.get(k) ?? null, setItem: (k: string, v: string) => store.set(k, v) }); });
afterEach(() => { cleanup(); vi.unstubAllGlobals(); });

describe("WorkspacePanel", () => {
  it("monitors changes read-only and shows files, branches and pull request availability", async () => {
    render(<WorkspacePanel projectRoot="/repo" threadId="t1" workspace={workspace} />);
    expect(screen.getByText("Read-only")).toBeInTheDocument();
    expect(screen.getByText("new")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /^Accept|^Discard|Merge|Comment/ })).toBeNull();
    const separator = screen.getByRole("separator", { name: "Resize workspace panel" });
    fireEvent.keyDown(separator, { key: "ArrowLeft" });
    expect(separator).toHaveAttribute("aria-valuenow", "640");
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Files" })));
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "README.md" })));
    expect(screen.getByText("hello")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Branches" }));
    expect(screen.getByRole("list", { name: "Branches" })).toHaveTextContent("maincurrent");
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Pull requests" })));
    expect(screen.getByText("GitHub CLI (gh) is not installed")).toBeInTheDocument();
  });
});
