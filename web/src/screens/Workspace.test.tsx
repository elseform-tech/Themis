// @vitest-environment jsdom
import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { beforeEach, afterEach, describe, expect, it, vi } from "vitest";
import { DEFAULT_SETTINGS } from "../state/reducer";
import App from "../App";
import * as bridge from "../lib/tauri";

vi.mock("../lib/tauri", async importOriginal => ({
  ...await importOriginal<typeof import("../lib/tauri")>(),
  getSettings: vi.fn(), getSecretStatus: vi.fn(), updateSettings: vi.fn(),
  onThreadEvent: vi.fn(async () => () => {}), onApprovalRequest: vi.fn(async () => () => {}), onReviewItemAdded: vi.fn(async () => () => {}),
  listSkills: vi.fn(async () => []), listAutomations: vi.fn(async () => []), listReviewItems: vi.fn(async () => []),
  setProvider: vi.fn(), getThread: vi.fn(), getThreadHistory: vi.fn(async () => []), sendMessage: vi.fn(async () => ({ run_id: "test-run" })),
  openProject: vi.fn(), listThreads: vi.fn(), createProject: vi.fn(), createThread: vi.fn(), listGoModels: vi.fn(async () => ["minimax-m2.5", "gpt-5.6-luna"]),
}));
const settings = { ...DEFAULT_SETTINGS, projects_directory: "/tmp/Themis/Projects" };
beforeEach(() => {
  vi.clearAllMocks();
  const storage = new Map<string, string>();
  vi.stubGlobal("localStorage", { getItem: (key: string) => storage.get(key) ?? null, setItem: (key: string, value: string) => storage.set(key, value), clear: () => storage.clear() });
  vi.mocked(bridge.getSettings).mockResolvedValue(settings);
  vi.mocked(bridge.getSecretStatus).mockResolvedValue({ go: true, openai: false, anthropic: false });
  vi.mocked(bridge.updateSettings).mockImplementation(async patch => ({ ...settings, ...patch }));
  window.matchMedia = vi.fn().mockReturnValue({ matches: false, addEventListener: vi.fn(), removeEventListener: vi.fn() });
  HTMLElement.prototype.scrollIntoView = vi.fn();
});
afterEach(() => vi.useRealTimers());
async function mount() { await act(async () => { render(<App />); }); }

describe("Workspace journey", () => {
  it("starts directly in the workspace and creates a project with only a name", async () => {
    vi.useFakeTimers();
    vi.mocked(bridge.createProject).mockResolvedValue({ name: "My project", root: "/tmp/Themis/Projects/My project", is_git: true });
    vi.mocked(bridge.createThread).mockResolvedValue({ id: "thread1", title: "New thread", provider: "go", model: "minimax-m2.5", running: false, worktree_path: null, branch: null, base_branch: null, recovered: false, skill_ids: [] });
    await mount();
    expect(screen.queryByText(/welcome to themis/i)).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "New project" }));
    fireEvent.change(screen.getByLabelText("Project name"), { target: { value: "My project" } });
    await act(async () => fireEvent.submit(screen.getByLabelText("Project name").closest("form")!));
    expect(bridge.createProject).toHaveBeenCalledWith("My project", undefined);
    expect(bridge.createThread).toHaveBeenCalledWith("/tmp/Themis/Projects/My project", "go", "minimax-m2.5");
    expect(screen.getByLabelText("Message")).toBeEnabled();
    expect(screen.getByRole("combobox", { name: "Model" })).toHaveValue("minimax-m2.5");
    expect(screen.getByRole("combobox", { name: "Reasoning effort" })).toBeDisabled();
    expect(screen.queryByLabelText("Diff review panel")).toBeNull();
    expect(screen.queryByText("Review changes")).toBeNull();
    expect(screen.queryByText("Activity")).toBeNull();
    const updated = { id: "thread1", title: "New thread", provider: "go" as const, model: "gpt-5.6-luna", running: false, worktree_path: null, branch: null, base_branch: null, recovered: false, skill_ids: [] };
    vi.mocked(bridge.setProvider).mockResolvedValue(updated);
    vi.mocked(bridge.getThread).mockResolvedValue(updated);
    await act(async () => fireEvent.change(screen.getByRole("combobox", { name: "Model" }), { target: { value: "gpt-5.6-luna" } }));
    expect(bridge.setProvider).toHaveBeenCalledWith("thread1", "go", "gpt-5.6-luna");
    expect(screen.getByRole("combobox", { name: "Reasoning effort" })).toBeEnabled();
    fireEvent.change(screen.getByRole("combobox", { name: "Reasoning effort" }), { target: { value: "low" } });
    fireEvent.change(screen.getByLabelText("Message"), { target: { value: "Read the test file" } });
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Send" })));
    expect(bridge.sendMessage).toHaveBeenCalledWith("thread1", "Read the test file", "low");
    expect(screen.queryByText("Thread options")).toBeNull();
    expect(screen.getByRole("button", { name: "Edit New thread" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Remove New thread" })).toBeInTheDocument();
    const receive = vi.mocked(bridge.onThreadEvent).mock.calls[0]![0];
    await act(async () => receive({ thread_id: "thread1", run_id: "r", event: { kind: "started", task: "read", max_turns: 3 } }));
    expect(screen.getByText("Thinking…")).toBeInTheDocument();
    expect(screen.getByLabelText("Elapsed time")).toHaveTextContent("0:00");
    await act(async () => { vi.advanceTimersByTime(12_000); });
    await act(async () => {
      receive({ thread_id: "thread1", run_id: "r", event: { kind: "assistant_text", text: "Milestone: Inspect current files\nI will read the file." } });
      receive({ thread_id: "thread1", run_id: "r", event: { kind: "tool_started", tool: "read_file", summary: "check.txt" } });
    });
    const actionText = screen.getByText("I will read the file.");
    expect(actionText.closest(".themis-thread-msg")).toHaveClass("themis-thread-msg--action-waiting");
    expect(screen.getByText("read file").closest("summary")).toHaveClass("themis-shimmer");
    await act(async () => {
      receive({ thread_id: "thread1", run_id: "r", event: { kind: "tool_finished", tool: "read_file", ok: true, output: "ORBIT-17" } });
      receive({ thread_id: "thread1", run_id: "r", event: { kind: "finished", result: "Verified." } });
    });
    const activitySummary = screen.getByText("Activities").closest("summary")!;
    expect(activitySummary).toHaveTextContent(/1 tool call · 12s elapsed/);
    expect(activitySummary).not.toHaveTextContent("milestone");
    const activities = activitySummary.parentElement as HTMLDetailsElement;
    expect(activities.open).toBe(false);
    fireEvent.click(activitySummary);
    expect(activities.open).toBe(true);
    expect(screen.getByText("I will read the file.")).toBeInTheDocument();
    expect(screen.queryByText("Inspect current files")).toBeNull();
    expect(screen.getByText("read file")).toBeInTheDocument();
    expect(screen.getByText("ORBIT-17")).toBeInTheDocument();
    expect(screen.queryByText("Task progress")).toBeNull();
    await act(async () => {
      receive({ thread_id: "thread1", run_id: "r2", event: { kind: "started", task: "follow up", max_turns: 3 } });
      receive({ thread_id: "thread1", run_id: "r2", event: { kind: "assistant_text", text: "Milestone: Follow-up\nI will check one more thing." } });
      receive({ thread_id: "thread1", run_id: "r2", event: { kind: "tool_started", tool: "list_files", summary: "workspace" } });
    });
    expect(screen.getByText("Verified.").closest(".themis-thread-msg")).not.toHaveClass("themis-thread-msg--action-waiting");
    expect(screen.getByText("I will check one more thing.").closest(".themis-thread-msg")).toHaveClass("themis-thread-msg--action-waiting");
    vi.mocked(bridge.createThread).mockResolvedValueOnce({ ...updated, id: "thread2" });
    await act(async () => fireEvent.keyDown(document, { key: "O", ctrlKey: true, shiftKey: true }));
    expect(bridge.createThread).toHaveBeenCalledTimes(2);
  });
  it("keeps restored legacy replies static during a follow-up run", async () => {
    const thread = { id: "legacy-thread", title: "Legacy", provider: "go" as const, model: "minimax-m2.5", running: false, worktree_path: null, branch: null, base_branch: null, recovered: false, skill_ids: [] };
    vi.mocked(bridge.getSettings).mockResolvedValue({ ...settings, recent_roots: ["/tmp/legacy"] });
    vi.mocked(bridge.openProject).mockResolvedValue({ root: "/tmp/legacy", name: "Legacy", is_git: true });
    vi.mocked(bridge.listThreads).mockResolvedValue([thread]);
    vi.mocked(bridge.getThreadHistory).mockResolvedValue([{ kind: "legacy", message: { id: "old-answer", role: "assistant", text: "Restored final answer" } }]);
    await mount();
    expect(await screen.findByText("Restored final answer")).toBeInTheDocument();

    fireEvent.change(screen.getByLabelText("Message"), { target: { value: "Follow up" } });
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Send" })));
    const receive = vi.mocked(bridge.onThreadEvent).mock.calls[0]![0];
    await act(async () => {
      receive({ thread_id: thread.id, run_id: "next-run", event: { kind: "started", task: "follow up", max_turns: 3 } });
      receive({ thread_id: thread.id, run_id: "next-run", event: { kind: "assistant_text", text: "Milestone: Follow-up\nI will check one thing." } });
      receive({ thread_id: thread.id, run_id: "next-run", event: { kind: "tool_started", tool: "read_file", summary: "check.txt" } });
    });

    expect(screen.getByText("Restored final answer").closest(".themis-thread-msg")).not.toHaveClass("themis-thread-msg--action-waiting");
    expect(screen.getByText("I will check one thing.").closest(".themis-thread-msg")).toHaveClass("themis-thread-msg--action-waiting");
  });
  it("collapses projects independently without changing the open conversation", async () => {
    const first = { id: "a", title: "First thread", provider: "go" as const, model: "minimax-m2.5", running: false, worktree_path: "/tmp/a", branch: "test", base_branch: "main", recovered: false, skill_ids: [] };
    vi.mocked(bridge.getSettings).mockResolvedValue({ ...settings, recent_roots: ["/tmp/one", "/tmp/two"] });
    vi.mocked(bridge.openProject).mockImplementation(async root => ({ root, name: root.endsWith("one") ? "One" : "Two", is_git: true }));
    vi.mocked(bridge.listThreads).mockImplementation(async root => [root.endsWith("one") ? first : { ...first, id: "b", title: "Second thread" }]);
    const view = await act(async () => render(<App />));
    const sidebar = screen.getByRole("complementary", { name: "Workspace navigation" });
    expect(within(sidebar).getByRole("button", { name: /^Second thread/ })).toBeInTheDocument();
    fireEvent.click(within(sidebar).getByRole("button", { name: "One" }));
    expect(within(sidebar).getByRole("button", { name: "One" })).toHaveAttribute("aria-expanded", "false");
    expect(within(sidebar).queryByRole("button", { name: /^First thread/ })).toBeNull();
    expect(screen.getByLabelText("Message")).toBeEnabled();
    view.unmount();
    await mount();
    expect(screen.getByRole("button", { name: "One" })).toHaveAttribute("aria-expanded", "false");
    fireEvent.click(screen.getByRole("button", { name: "One" }));
    expect(screen.getByRole("button", { name: /^First thread/ })).toBeInTheDocument();
  });
  it("shows restored running, completed, failed, and stopped thread status", async () => {
    const thread = { id: "status", title: "Status thread", provider: "go" as const, model: "minimax-m2.5", running: true, worktree_path: "/tmp/a", branch: "test", base_branch: "main", recovered: false, skill_ids: [] };
    vi.mocked(bridge.getSettings).mockResolvedValue({ ...settings, recent_roots: ["/tmp/one"] });
    vi.mocked(bridge.openProject).mockResolvedValue({ root: "/tmp/one", name: "One", is_git: true });
    vi.mocked(bridge.listThreads).mockResolvedValue([thread]);
    await mount();
    const sidebar = screen.getByRole("complementary", { name: "Workspace navigation" });
    expect(within(sidebar).getByRole("img", { name: "Working" })).toBeInTheDocument();
    expect(within(sidebar).getByRole("button", { name: "Edit Status thread" })).toBeDisabled();
    const receive = vi.mocked(bridge.onThreadEvent).mock.calls[0]![0];
    await act(async () => receive({ thread_id: "status", run_id: "r", event: { kind: "finished", result: "Done" } }));
    expect(within(sidebar).getByRole("img", { name: "Done" })).toBeInTheDocument();
    await act(async () => receive({ thread_id: "status", run_id: "old", event: { kind: "incomplete", result: "I completed the draft. Next, verify it." } }));
    expect(within(sidebar).getByRole("img", { name: "Idle" })).toBeInTheDocument();
    expect(screen.queryByText(/Run activity · Incomplete/)).not.toBeInTheDocument();
    await act(async () => receive({ thread_id: "status", run_id: "r2", event: { kind: "started", task: "retry", max_turns: 3 } }));
    expect(within(sidebar).getByRole("img", { name: "Working" })).toBeInTheDocument();
    await act(async () => receive({ thread_id: "status", run_id: "r2", event: { kind: "failed", error: "HTTP 503" } }));
    expect(within(sidebar).getByRole("img", { name: "Failed" })).toBeInTheDocument();
    await act(async () => receive({ thread_id: "status", run_id: "r3", event: { kind: "failed", error: "Stopped by you." } }));
    expect(within(sidebar).getByRole("img", { name: "Stopped" })).toBeInTheDocument();
  });
  it("fully hides navigation and delays hover reveal and dismissal", async () => {
    vi.useFakeTimers(); await mount();
    fireEvent.click(screen.getByRole("button", { name: "Collapse sidebar" }));
    const sidebar = document.getElementById("themis-sidebar")!;
    expect(sidebar).toHaveAttribute("inert");
    fireEvent.pointerEnter(screen.getByTestId("sidebar-edge"));
    act(() => vi.advanceTimersByTime(299)); expect(sidebar).toHaveAttribute("inert");
    act(() => vi.advanceTimersByTime(1)); expect(sidebar).not.toHaveAttribute("inert");
    fireEvent.pointerEnter(sidebar); fireEvent.pointerLeave(sidebar);
    act(() => vi.advanceTimersByTime(699)); expect(sidebar).not.toHaveAttribute("inert");
    act(() => vi.advanceTimersByTime(1)); expect(sidebar).toHaveAttribute("inert");
    fireEvent.click(screen.getByRole("button", { name: "Show sidebar" }));
    expect(sidebar).not.toHaveAttribute("inert");
  });
  it("saves appearance independently and reports save failure", async () => {
    await mount(); fireEvent.click(screen.getByRole("button", { name: "Settings" }));
    fireEvent.click(screen.getByRole("button", { name: "Appearance" }));
    await act(async () => fireEvent.change(screen.getByLabelText("Text size"), { target: { value: "16" } }));
    expect(bridge.updateSettings).toHaveBeenCalledWith({ text_size: 16 });
    expect(screen.getByText("Saved")).toBeInTheDocument();
    vi.mocked(bridge.updateSettings).mockRejectedValueOnce("Disk is full");
    await act(async () => fireEvent.change(screen.getByLabelText("Theme"), { target: { value: "light" } }));
    expect(screen.getByRole("alert")).toHaveTextContent("Disk is full");
    expect(screen.getByLabelText("Theme")).toHaveValue("system");
  });
});
