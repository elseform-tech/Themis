// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import * as bridge from "../lib/tauri";
import type { MemoryList } from "../lib/types";
import { initialState, reducer } from "../state/reducer";
import { MemoryLine, MemorySettingsSection, memoryDate, memoryTitle } from "./MemorySettings";

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

function setup(enabled = true) {
  vi.mocked(bridge.getMemorySettings).mockResolvedValue({ settings: { enabled, auto: false }, revision: "s1" });
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
    vi.mocked(bridge.updateMemorySettings).mockResolvedValue({ settings: { enabled: false, auto: false }, revision: "s2" });
    const saved = vi.fn();
    await act(async () => { render(<MemorySettingsSection onSaved={saved} />); });
    await act(async () => { fireEvent.click(screen.getByRole("checkbox", { name: /Remember things across chats/ })); });
    expect(bridge.updateMemorySettings).toHaveBeenCalledWith({ enabled: false }, "s1");
    expect(saved).toHaveBeenCalled();
    expect(screen.getByText(/Memory is off/)).toBeVisible();
    expect(screen.queryByText("2 memories")).toBeNull();
    expect(screen.queryByRole("button", { name: "Everywhere" })).toBeNull();
    expect(screen.queryByRole("checkbox", { name: /Notice what's worth remembering/ })).toBeNull();
  });

  it("turns noticing what to remember on in place, off by default", async () => {
    setup();
    vi.mocked(bridge.updateMemorySettings).mockResolvedValue({ settings: { enabled: true, auto: true }, revision: "s2" });
    await act(async () => { render(<MemorySettingsSection />); });
    const notice = screen.getByRole("checkbox", { name: /Notice what's worth remembering/ });
    expect(notice).not.toBeChecked();
    await act(async () => { fireEvent.click(notice); });
    expect(bridge.updateMemorySettings).toHaveBeenCalledWith({ auto: true }, "s1");
    expect(screen.getByRole("checkbox", { name: /Notice what's worth remembering/ })).toBeChecked();
  });
});

describe("memory in the chat", () => {
  it("is a slim Remembered line that opens the memory in Settings", () => {
    render(<MemoryLine note={{ action: "saved", scope: "global", name: "free-model-for-tests", description: "Use the free model" }} />);
    expect(screen.getByRole("note")).toHaveTextContent("RememberedFree model for tests");
    fireEvent.click(screen.getByRole("button", { name: /Free model for tests/ }));
    expect(dispatch).toHaveBeenCalledWith({ type: "ui/view", view: "settings", settings: { section: "Memory", key: "global:free-model-for-tests" } });
  });

  it("says Forgot for a deleted memory, with nothing to open", () => {
    render(<MemoryLine note={{ action: "forgot", scope: "project", name: "old-fact", description: "Gone" }} />);
    expect(screen.getByRole("note")).toHaveTextContent("ForgotOld fact");
    expect(screen.queryByRole("button")).toBeNull();
  });

  it("comes from the memory_changed event and stays in the run", () => {
    const state = reducer(initialState, { type: "thread/event", envelope: { thread_id: "t", run_id: "r", event: { kind: "memory_changed", action: "updated", scope: "project", name: "a-fact", description: "A fact", project_root: "/p" } } } as never);
    const rows = state.messages.t ?? [];
    const row = rows[rows.length - 1];
    expect(row?.memory).toEqual({ action: "updated", scope: "project", name: "a-fact", description: "A fact", project_root: "/p" });
    expect(row?.runId).toBe("r");
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
