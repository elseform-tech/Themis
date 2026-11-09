// @vitest-environment jsdom
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { DEFAULT_SETTINGS } from "../state/reducer";
import App from "../App";
import * as bridge from "../lib/tauri";
import type { ThreadInfo } from "../lib/types";

vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
vi.mock("../lib/tauri", async importOriginal => ({
  ...await importOriginal<typeof import("../lib/tauri")>(),
  attachFiles: vi.fn(), pluginAction: vi.fn(async () => []), listPromptSkills: vi.fn(async () => []),
  getDefaultProject: vi.fn(async () => ({ name: "Themis", root: "/tmp/fixed-themis", is_git: true, is_default: true })), getSettings: vi.fn(), getSecretStatus: vi.fn(), updateSettings: vi.fn(),
  onBackendResync: vi.fn(async () => () => {}), getPendingApprovals: vi.fn(async () => []),
  onThreadEvent: vi.fn(async () => () => {}), onApprovalRequest: vi.fn(async () => () => {}), onApprovalExpired: vi.fn(async () => () => {}), onReviewItemAdded: vi.fn(async () => () => {}), onThreadChanged: vi.fn(async () => () => {}), onSettingsChanged: vi.fn(async () => () => {}),
  listSkills: vi.fn(async () => []), listAutomations: vi.fn(async () => []), listReviewItems: vi.fn(async () => []),
  renameThread: vi.fn(), getThread: vi.fn(), getThreadHistory: vi.fn(async () => []), sendMessage: vi.fn(async () => ({ run_id: "test-run" })),
  openProject: vi.fn(), listThreads: vi.fn(), createProject: vi.fn(), renameProject: vi.fn(), createThread: vi.fn(), listGoModels: vi.fn(async () => [{ id: "test", effort_levels: [] }]),
}));

const settings = { ...DEFAULT_SETTINGS, projects_directory: "/tmp/Themis/Projects", recent_roots: ["/tmp/one"] };
const base = { provider: "go" as const, model: "test", running: false, worktree_path: null, branch: null, base_branch: null, recovered: false, skill_ids: [] };
const chat = (id: string, title: string, seq: number, extra: Partial<ThreadInfo> = {}): ThreadInfo => ({ ...base, id, title, last_activity_seq: seq, ...extra });

beforeEach(() => {
  vi.clearAllMocks();
  const storage = new Map<string, string>();
  vi.stubGlobal("localStorage", { getItem: (key: string) => storage.get(key) ?? null, setItem: (key: string, value: string) => storage.set(key, value), clear: () => storage.clear() });
  vi.mocked(bridge.getSettings).mockResolvedValue(settings);
  vi.mocked(bridge.getSecretStatus).mockResolvedValue({ go: true });
  vi.mocked(bridge.updateSettings).mockImplementation(async patch => ({ ...settings, ...patch }));
  vi.mocked(bridge.openProject).mockResolvedValue({ root: "/tmp/one", name: "One", is_git: true });
  window.matchMedia = vi.fn().mockReturnValue({ matches: false, addEventListener: vi.fn(), removeEventListener: vi.fn() });
  HTMLElement.prototype.scrollIntoView = vi.fn();
});
afterEach(() => { vi.unstubAllGlobals(); });

/** Mounts the app with these chats in project One and returns the sidebar and the event feed. */
async function mountWith(threads: ThreadInfo[]) {
  vi.mocked(bridge.listThreads).mockImplementation(async root => root === "/tmp/fixed-themis" ? [] : threads);
  vi.mocked(bridge.getThread).mockImplementation(async id => threads.find(thread => thread.id === id)!);
  await act(async () => { render(<App />); });
  const sidebar = screen.getByRole("complementary", { name: "Workspace navigation" });
  fireEvent.click(within(sidebar).getByRole("button", { name: /^One/ }));
  const receive = vi.mocked(bridge.onThreadEvent).mock.calls[0]![0];
  return { sidebar, receive };
}
const titles = (group: HTMLElement) => [...group.querySelectorAll(".themis-thread-row")].map(row => row.getAttribute("aria-label"));
const row = (sidebar: HTMLElement, title: string) => within(sidebar).getByRole("button", { name: title });

describe("Sidebar", () => {
  it("keeps a chat that started after the app opened running when it is opened mid-run", async () => {
    const { sidebar, receive } = await mountWith([chat("a", "Alpha", 2), chat("l", "Later", 1)]);
    // "Later" starts running (an automation, the CLI) while its history isn't loaded.
    await act(async () => receive({ thread_id: "l", run_id: "r9", event: { kind: "started", task: "sleep", max_turns: 3 } }));
    vi.mocked(bridge.getThreadHistory).mockResolvedValue([{ kind: "user", text: "sleep", run_id: "r9" }] as never);
    await act(async () => fireEvent.click(row(sidebar, "Later")));
    await waitFor(() => expect(vi.mocked(bridge.getThreadHistory)).toHaveBeenCalledWith("l"));
    // Still running, and nothing claims the run was interrupted.
    expect(titles(within(sidebar).getByRole("region", { name: "Running" }))).toEqual(["Later"]);
    expect(screen.queryByText(/Run interrupted/)).toBeNull();
    // And it shows what was loaded.
    await waitFor(() => expect(screen.getByText("sleep")).toBeTruthy());
  });

  it("shows a Running group only while a chat works or waits, and moves chats between the groups", async () => {
    const { sidebar, receive } = await mountWith([chat("a", "Alpha", 3), chat("b", "Bravo", 2, { running: true }), chat("c", "Charlie", 1)]);
    const running = within(sidebar).getByRole("region", { name: "Running" });
    expect(within(running).getByRole("heading")).toHaveTextContent(/^Running1$/);
    expect(titles(running)).toEqual(["Bravo"]);
    const chats = () => within(sidebar).getByRole("region", { name: "Chats" });
    expect(within(chats()).getByRole("heading")).toHaveTextContent(/^Chats2$/);
    expect(titles(chats())).toEqual(["Alpha", "Charlie"]);
    // Running sits above Chats.
    expect(running.compareDocumentPosition(chats()) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();

    await act(async () => receive({ thread_id: "b", run_id: "r", event: { kind: "finished", result: "Done" } }));
    expect(within(sidebar).queryByRole("region", { name: "Running" })).toBeNull();
    expect(titles(chats())).toEqual(["Alpha", "Bravo", "Charlie"]);

    await act(async () => receive({ thread_id: "c", run_id: "r2", event: { kind: "started", task: "go", max_turns: 3 } }));
    expect(titles(within(sidebar).getByRole("region", { name: "Running" }))).toEqual(["Charlie"]);
    expect(titles(chats())).toEqual(["Alpha", "Bravo"]);
  });

  it("parks a chat waiting for approval under Waiting for you and hides empty groups", async () => {
    const waiting = chat("w", "Waiting chat", 1, { running: true });
    vi.mocked(bridge.getPendingApprovals).mockResolvedValue([{ thread_id: "w", approval_id: "p", tool: "read_file", risk: "read", summary: "Read it" }]);
    const { sidebar } = await mountWith([waiting]);
    const group = within(sidebar).getByRole("region", { name: "Waiting for you" });
    expect(titles(group)).toEqual(["Waiting chat"]);
    expect(within(sidebar).queryByRole("region", { name: "Running" })).toBeNull();
    expect(within(sidebar).queryByRole("region", { name: "Chats" })).toBeNull();
    // The row says why it waits.
    expect(within(row(sidebar, "Waiting chat")).getByText("Approval")).toBeTruthy();
    const mark = within(row(sidebar, "Waiting chat")).getByRole("img", { name: "Waiting for your approval" });
    // A pause sign: left and right columns of the 3×3 grid lit, the middle column faint.
    expect(mark.querySelectorAll("circle")).toHaveLength(9);
    expect([...mark.querySelectorAll("circle")].map(dot => dot.classList.contains("is-lit"))).toEqual([true, false, true, true, false, true, true, false, true]);
    expect(row(sidebar, "Waiting chat").querySelector(".themis-sidebar-icon")).toBeNull();
  });

  it("calls a destructive step Auto or YOLO asks about first a question, like other questions", async () => {
    vi.mocked(bridge.getPendingApprovals).mockResolvedValue([{ thread_id: "q", approval_id: "d", tool: "shell", risk: "destructive", summary: "rm", command: { argv: ["rm", "-rf", "build"] }, ask_first: true }]);
    const { sidebar } = await mountWith([chat("q", "Asking chat", 1, { running: true })]);
    expect(titles(within(sidebar).getByRole("region", { name: "Waiting for you" }))).toEqual(["Asking chat"]);
    expect(within(row(sidebar, "Asking chat")).getByText("Question")).toBeTruthy();
    expect(within(row(sidebar, "Asking chat")).queryByText("Approval")).toBeNull();
    expect(within(row(sidebar, "Asking chat")).getByRole("img", { name: "Waiting for your answer" })).toBeTruthy();
  });

  it("draws status in the icon slot: walking dots, a cross for failure, the bubble otherwise, nothing on the right", async () => {
    const { sidebar, receive } = await mountWith([chat("s", "Status chat", 1, { running: true })]);
    const working = within(row(sidebar, "Status chat")).getByRole("img", { name: "Working" });
    // Eight edge dots of the square, clockwise from the top-left, no centre and no faint grid.
    const dots = [...working.querySelectorAll("circle")];
    expect(dots.map(dot => `${dot.getAttribute("cx")},${dot.getAttribute("cy")}`)).toEqual(["5,5", "12,5", "19,5", "19,12", "19,19", "12,19", "5,19", "5,12"]);
    expect(dots.every(dot => dot.getAttribute("r") === "2.1")).toBe(true);
    // The icon comes first in the row, before the title.
    expect(row(sidebar, "Status chat").firstElementChild).toBe(working);

    await act(async () => receive({ thread_id: "s", run_id: "r", event: { kind: "failed", error: "HTTP 503" } }));
    const failed = within(row(sidebar, "Status chat")).getByRole("img", { name: "Failed" });
    expect([...failed.querySelectorAll("circle")].map(dot => dot.classList.contains("is-lit"))).toEqual([true, false, true, false, true, false, true, false, true]);
    expect(row(sidebar, "Status chat").firstElementChild).toBe(failed);

    await act(async () => receive({ thread_id: "s", run_id: "r2", event: { kind: "failed", error: "Stopped by you." } }));
    expect(within(sidebar).queryByRole("img")).toBeNull();
    expect(row(sidebar, "Status chat").firstElementChild).toHaveClass("themis-sidebar-icon");
    expect(row(sidebar, "Status chat").children).toHaveLength(2);
  });

  it("marks a run that finished out of view with a bold title, never a tick, until the chat is opened", async () => {
    const { sidebar, receive } = await mountWith([chat("front", "Front chat", 2), chat("back", "Background chat", 1, { running: true })]);
    expect(row(sidebar, "Front chat")).toHaveAttribute("aria-current", "true");
    await act(async () => receive({ thread_id: "back", run_id: "r", event: { kind: "finished", result: "Done" } }));
    expect(within(sidebar).queryByRole("img")).toBeNull();
    expect(row(sidebar, "Background chat")).toHaveClass("is-unseen");
    expect(row(sidebar, "Front chat")).not.toHaveClass("is-unseen");
    await act(async () => fireEvent.click(row(sidebar, "Background chat")));
    expect(row(sidebar, "Background chat")).not.toHaveClass("is-unseen");
  });

  it("does not mark the chat on screen as unseen when its run finishes", async () => {
    const { sidebar, receive } = await mountWith([chat("only", "Only chat", 1, { running: true })]);
    expect(row(sidebar, "Only chat")).toHaveAttribute("aria-current", "true");
    await act(async () => receive({ thread_id: "only", run_id: "r", event: { kind: "finished", result: "Done" } }));
    expect(row(sidebar, "Only chat")).not.toHaveClass("is-unseen");
  });
});
