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
  setProvider: vi.fn(), setThreadEffort: vi.fn(), getThread: vi.fn(), getThreadHistory: vi.fn(async () => []), sendMessage: vi.fn(async () => ({ run_id: "test-run" })),
  openProject: vi.fn(), listThreads: vi.fn(), createProject: vi.fn(), createThread: vi.fn(), listGoModels: vi.fn(async () => [{ id: "muse-spark-1.3-contributor", effort_levels: [] }, { id: "gpt-5.6-luna", effort_levels: ["low", "medium", "high"] }]),
}));
const settings = { ...DEFAULT_SETTINGS, projects_directory: "/tmp/Themis/Projects" };
beforeEach(() => {
  vi.clearAllMocks();
  const storage = new Map<string, string>();
  vi.stubGlobal("localStorage", { getItem: (key: string) => storage.get(key) ?? null, setItem: (key: string, value: string) => storage.set(key, value), clear: () => storage.clear() });
  vi.mocked(bridge.getSettings).mockResolvedValue(settings);
  vi.mocked(bridge.getSecretStatus).mockResolvedValue({ go: true });
  vi.mocked(bridge.updateSettings).mockImplementation(async patch => ({ ...settings, ...patch }));
  window.matchMedia = vi.fn().mockReturnValue({ matches: false, addEventListener: vi.fn(), removeEventListener: vi.fn() });
  HTMLElement.prototype.scrollIntoView = vi.fn();
});
afterEach(() => { vi.useRealTimers(); vi.unstubAllGlobals(); });
async function mount() { await act(async () => { render(<App />); }); }

describe("Workspace journey", () => {
  it("plays one desktop completion sound for a manually sent response", async () => {
    const root = "/tmp/sound-project";
    const thread = { id: "sound-thread", title: "Sound", provider: "go" as const, model: "muse-spark-1.3-contributor", running: false, worktree_path: null, branch: null, base_branch: null, recovered: false, skill_ids: [] };
    const start = vi.fn();
    const buffer = { duration: 0.5 };
    const source = { buffer: null as object | null, connect: vi.fn(), start };
    vi.stubGlobal("__TAURI_INTERNALS__", {});
    vi.stubGlobal("fetch", vi.fn(async () => ({ ok: true, arrayBuffer: async () => new ArrayBuffer(8) })));
    vi.stubGlobal("AudioContext", vi.fn().mockImplementation(() => ({
      state: "running", destination: {}, resume: vi.fn(),
      decodeAudioData: vi.fn(async () => buffer), createBufferSource: () => source,
    })));
    vi.mocked(bridge.getSettings).mockResolvedValue({ ...settings, recent_roots: [root] });
    vi.mocked(bridge.openProject).mockResolvedValue({ root, name: "Sound project", is_git: true });
    vi.mocked(bridge.listThreads).mockResolvedValue([thread]);
    vi.mocked(bridge.getThread).mockResolvedValue(thread);
    await mount();
    const receive = vi.mocked(bridge.onThreadEvent).mock.calls[0]![0];
    await act(async () => receive({ thread_id: thread.id, run_id: "automation", event: { kind: "finished", result: "Scheduled" } }));
    expect(start).not.toHaveBeenCalled();
    fireEvent.input(screen.getByLabelText("Message"), { target: { textContent: "Hello" } });
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Send" })));
    expect(AudioContext).toHaveBeenCalledTimes(1);
    expect(fetch).toHaveBeenCalledWith("/sounds/completion-woodblock-double.mp3");
    await act(async () => {
      receive({ thread_id: thread.id, run_id: "test-run", event: { kind: "started", task: "Hello", max_turns: 2 } });
      receive({ thread_id: thread.id, run_id: "test-run", event: { kind: "finished", result: "Done" } });
      receive({ thread_id: thread.id, run_id: "test-run", event: { kind: "finished", result: "Done" } });
    });
    expect(start).toHaveBeenCalledTimes(1);
    expect(source.buffer).toBe(buffer);
  });

  it("shows a finished automation thread and pending review without opening it", async () => {
    const root = "/tmp/automation-project";
    const thread = { id: "scheduled-thread", title: "Scheduled check", provider: "go" as const, model: "muse-spark-1.3-contributor", running: false, worktree_path: null, branch: null, base_branch: null, recovered: false, skill_ids: [] };
    const automation = { id: "automation-1", name: "Scheduled check", project_root: root, provider: "go" as const, model: thread.model, skill_ids: [], interval_mins: 60, task: "Check", enabled: false, last_run_at: null, next_run_at: "2026-09-28T22:00:00Z", run_count: 1 };
    vi.mocked(bridge.getSettings).mockResolvedValue({ ...settings, recent_roots: [root] });
    vi.mocked(bridge.openProject).mockResolvedValue({ root, name: "Automation project", is_git: true });
    vi.mocked(bridge.listThreads).mockResolvedValue([]);
    vi.mocked(bridge.listAutomations).mockResolvedValue([automation]);
    vi.mocked(bridge.getThread).mockResolvedValue(thread);
    await mount();
    const review = { id: "review-1", automation_id: automation.id, thread_id: thread.id, created_at: "2026-09-28T21:00:00Z", title: "Scheduled check", summary: "Completed", status: "pending" as const };
    const onReview = vi.mocked(bridge.onReviewItemAdded).mock.calls[0]![0];
    const onThread = vi.mocked(bridge.onThreadEvent).mock.calls[0]![0];
    await act(async () => {
      onReview(review);
      onThread({ thread_id: thread.id, run_id: "run-1", event: { kind: "finished", result: "Completed" } });
    });
    const sidebar = screen.getByRole("complementary", { name: "Workspace navigation" });
    expect(within(within(sidebar).getByRole("button", { name: /^Scheduled check/ })).getByRole("img", { name: "Done" })).toBeInTheDocument();
    const queueButton = within(sidebar).getByRole("button", { name: /Review queue\s*1/ });
    fireEvent.click(queueButton);
    expect(screen.getByRole("tab", { name: "Pending (1)" })).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "Reviewed (0)" })).toBeInTheDocument();
  });

  it("starts directly in the workspace and creates a project with only a name", async () => {
    vi.useFakeTimers();
    vi.mocked(bridge.createProject).mockResolvedValue({ name: "My project", root: "/tmp/Themis/Projects/My project", is_git: true });
    vi.mocked(bridge.createThread).mockResolvedValue({ id: "thread1", title: "New thread", provider: "go", model: "muse-spark-1.3-contributor", running: false, worktree_path: null, branch: null, base_branch: null, recovered: false, skill_ids: [] });
    await mount();
    expect(screen.queryByText(/welcome to themis/i)).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "New project" }));
    fireEvent.change(screen.getByLabelText("Project name"), { target: { value: "My project" } });
    await act(async () => fireEvent.submit(screen.getByLabelText("Project name").closest("form")!));
    expect(bridge.createProject).toHaveBeenCalledWith("My project", undefined);
    expect(bridge.createThread).toHaveBeenCalledWith("/tmp/Themis/Projects/My project", "go", "muse-spark-1.3-contributor");
    expect(screen.getByLabelText("Message")).toBeEnabled();
    expect(screen.getByRole("combobox", { name: "Model" })).toHaveValue("muse-spark-1.3-contributor");
    expect(screen.getByRole("combobox", { name: "Model" }).closest(".themis-composer-model-picker")).toHaveTextContent("Go");
    fireEvent.click(screen.getByTitle("Reasoning effort"));
    expect(screen.getByRole("slider", { name: "Reasoning effort" })).toBeDisabled();
    expect(screen.queryByLabelText("Diff review panel")).toBeNull();
    expect(screen.queryByText("Review changes")).toBeNull();
    expect(screen.queryByText("Activity")).toBeNull();
    const updated = { id: "thread1", title: "New thread", provider: "go" as const, model: "gpt-5.6-luna", running: false, worktree_path: null, branch: null, base_branch: null, recovered: false, skill_ids: [] };
    vi.mocked(bridge.setProvider).mockResolvedValue(updated);
    vi.mocked(bridge.getThread).mockResolvedValue(updated);
    await act(async () => fireEvent.change(screen.getByRole("combobox", { name: "Model" }), { target: { value: "gpt-5.6-luna" } }));
    expect(bridge.setProvider).toHaveBeenCalledWith("thread1", "go", "gpt-5.6-luna");
    expect(screen.getByRole("slider", { name: "Reasoning effort" })).toBeEnabled();
    vi.mocked(bridge.setThreadEffort).mockResolvedValue({ ...updated, reasoning_effort: "low" });
    await act(async () => fireEvent.change(screen.getByRole("slider", { name: "Reasoning effort" }), { target: { value: "1" } }));
    expect(bridge.setThreadEffort).toHaveBeenCalledWith("thread1", "low");
    fireEvent.input(screen.getByLabelText("Message"), { target: { textContent: "Read the test file" } });
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Send" })));
    expect(bridge.sendMessage).toHaveBeenCalledWith("thread1", "Read the test file", "low");
    expect(screen.queryByText("Thread options")).toBeNull();
    expect(screen.getByRole("button", { name: "Edit New thread" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Remove New thread" })).toBeInTheDocument();
    const receive = vi.mocked(bridge.onThreadEvent).mock.calls[0]![0];
    await act(async () => receive({ thread_id: "thread1", run_id: "r", event: { kind: "started", task: "read", max_turns: 3 } }));
    expect(screen.getByText("Thinking…")).toBeInTheDocument();
    expect(screen.getByText("Thinking…")).toHaveClass("themis-shimmer");
    expect(screen.getByLabelText("Elapsed time")).toHaveTextContent("0:00");
    await act(async () => { vi.advanceTimersByTime(12_000); });
    await act(async () => {
      receive({ thread_id: "thread1", run_id: "r", event: { kind: "assistant_text", text: "Milestone: Inspect current files\nI will read the file." } });
      receive({ thread_id: "thread1", run_id: "r", event: { kind: "tool_started", tool: "read_file", summary: "check.txt" } });
    });
    const actionText = screen.getByText("I will read the file.");
    expect(actionText.closest(".themis-thread-msg")).toHaveClass("themis-thread-msg--action-waiting");
    expect(screen.getByText("Using read file…")).toHaveClass("themis-shimmer");
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
    const thread = { id: "legacy-thread", title: "Legacy", provider: "go" as const, model: "muse-spark-1.3-contributor", running: false, worktree_path: null, branch: null, base_branch: null, recovered: false, skill_ids: [] };
    vi.mocked(bridge.getSettings).mockResolvedValue({ ...settings, recent_roots: ["/tmp/legacy"] });
    vi.mocked(bridge.openProject).mockResolvedValue({ root: "/tmp/legacy", name: "Legacy", is_git: true });
    vi.mocked(bridge.listThreads).mockResolvedValue([thread]);
    vi.mocked(bridge.getThreadHistory).mockResolvedValue([{ kind: "legacy", message: { id: "old-answer", role: "assistant", text: "Restored final answer" } }]);
    await mount();
    expect(await screen.findByText("Restored final answer")).toBeInTheDocument();

    fireEvent.input(screen.getByLabelText("Message"), { target: { textContent: "Follow up" } });
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Send" })));
    const receive = vi.mocked(bridge.onThreadEvent).mock.calls[0]![0];
    await act(async () => {
      receive({ thread_id: thread.id, run_id: "next-run", event: { kind: "started", task: "follow up", max_turns: 3 } });
    });
    expect(screen.getByText("Restored final answer").closest(".themis-thread-msg")).not.toHaveClass("themis-thread-msg--action-waiting");

    await act(async () => {
      receive({ thread_id: thread.id, run_id: "next-run", event: { kind: "assistant_text", text: "Milestone: Follow-up\nI will check one thing." } });
      receive({ thread_id: thread.id, run_id: "next-run", event: { kind: "tool_started", tool: "read_file", summary: "check.txt" } });
    });

    const oldReply = screen.getByText("Restored final answer").closest(".themis-thread-msg")!;
    expect(oldReply).toHaveClass("themis-thread-msg--completed");
    expect(oldReply).not.toHaveClass("themis-thread-msg--action-waiting");
    expect(oldReply.querySelector(".themis-shimmer")).toBeNull();
    expect(screen.getByText("I will check one thing.").closest(".themis-thread-msg")).toHaveClass("themis-thread-msg--action-waiting");
  });
  it("collapses projects independently without changing the open conversation", async () => {
    const first = { id: "a", title: "First thread", provider: "go" as const, model: "muse-spark-1.3-contributor", running: false, worktree_path: "/tmp/a", branch: "test", base_branch: "main", recovered: false, skill_ids: [] };
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
    const thread = { id: "status", title: "Status thread", provider: "go" as const, model: "muse-spark-1.3-contributor", running: true, worktree_path: "/tmp/a", branch: "test", base_branch: "main", recovered: false, skill_ids: [] };
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
  it("keeps compact navigation while delaying the full sidebar hover reveal", async () => {
    vi.useFakeTimers(); await mount();
    fireEvent.click(screen.getByRole("button", { name: "Collapse sidebar" }));
    const sidebar = document.getElementById("themis-sidebar")!;
    expect(sidebar).toHaveAttribute("inert");
    const rail = screen.getByRole("navigation", { name: "Compact workspace navigation" });
    expect(within(rail).getByRole("img", { name: "ThemisCode" })).toBeInTheDocument();
    expect(within(rail).getByRole("button", { name: "Settings" })).toBeEnabled();
    expect(document.getElementById("sidebar-toggle")?.closest("header")).toBeInTheDocument();
    fireEvent.pointerEnter(screen.getByTestId("sidebar-edge"));
    act(() => vi.advanceTimersByTime(299)); expect(sidebar).toHaveAttribute("inert");
    act(() => vi.advanceTimersByTime(1)); expect(sidebar).not.toHaveAttribute("inert");
    fireEvent.pointerLeave(screen.getByTestId("sidebar-edge"));
    act(() => vi.advanceTimersByTime(699)); expect(sidebar).not.toHaveAttribute("inert");
    act(() => vi.advanceTimersByTime(1)); expect(sidebar).toHaveAttribute("inert");
    fireEvent.pointerEnter(screen.getByTestId("sidebar-edge"));
    act(() => vi.advanceTimersByTime(300));
    fireEvent.pointerEnter(sidebar); fireEvent.pointerLeave(sidebar);
    act(() => vi.advanceTimersByTime(699)); expect(sidebar).not.toHaveAttribute("inert");
    act(() => vi.advanceTimersByTime(1)); expect(sidebar).toHaveAttribute("inert");
    fireEvent.click(screen.getByRole("button", { name: "Show sidebar" }));
    expect(sidebar).not.toHaveAttribute("inert");
  });
  it("saves appearance independently and reports save failure", async () => {
    await mount(); fireEvent.click(screen.getByRole("button", { name: "Settings" }));
    fireEvent.click(screen.getByRole("button", { name: "Appearance" }));
    const sound = screen.getByRole("checkbox", { name: "Play a click when my response finishes" });
    expect(sound).toBeChecked();
    await act(async () => fireEvent.click(sound));
    expect(bridge.updateSettings).toHaveBeenCalledWith({ completion_sound: false });
    await act(async () => fireEvent.change(screen.getByLabelText("Text size"), { target: { value: "16" } }));
    expect(bridge.updateSettings).toHaveBeenCalledWith({ text_size: 16 });
    expect(screen.getByText("Saved")).toBeInTheDocument();
    vi.mocked(bridge.updateSettings).mockRejectedValueOnce("Disk is full");
    await act(async () => fireEvent.change(screen.getByLabelText("Theme"), { target: { value: "light" } }));
    expect(screen.getByRole("alert")).toHaveTextContent("Disk is full");
    expect(screen.getByLabelText("Theme")).toHaveValue("system");
  });
  it("saves and applies palette, font and larger text without replacing composer controls", async () => {
    await mount();
    fireEvent.click(screen.getByRole("button", { name: "Settings" }));
    fireEvent.click(screen.getByRole("button", { name: "Appearance" }));
    await act(async () => fireEvent.change(screen.getByLabelText("Color palette"), { target: { value: "ocean" } }));
    expect(bridge.updateSettings).toHaveBeenCalledWith({ theme_palette: "ocean" });
    expect(document.documentElement.dataset.palette).toBe("ocean");
    await act(async () => fireEvent.change(screen.getByLabelText("Font"), { target: { value: "mono" } }));
    expect(bridge.updateSettings).toHaveBeenCalledWith({ font_family: "mono" });
    expect(document.documentElement.dataset.font).toBe("mono");
    await act(async () => fireEvent.change(screen.getByLabelText("Text size"), { target: { value: "22" } }));
    expect(bridge.updateSettings).toHaveBeenCalledWith({ text_size: 22 });
  });
  it("selects the default model from the Go catalog", async () => {
    await mount();
    fireEvent.click(screen.getByRole("button", { name: "Settings" }));
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Models & connections" })));
    expect(bridge.listGoModels).toHaveBeenCalled();
    expect(screen.queryByRole("textbox", { name: "Default model" })).toBeNull();
    const select = screen.getByRole("combobox", { name: "Default model" });
    expect(select).toHaveValue("muse-spark-1.3-contributor");
    await act(async () => fireEvent.change(select, { target: { value: "gpt-5.6-luna" } }));
    expect(bridge.updateSettings).toHaveBeenCalledWith({ default_model: "gpt-5.6-luna" });
  });
});
