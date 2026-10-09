// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { initialState, type AppState } from "../state/reducer";
import { Automations } from "./Automations";
import * as api from "../lib/tauri";

const mocks = vi.hoisted(() => ({ dispatch: vi.fn(), state: null as unknown as AppState }));
vi.mock("../state/store", async original => ({ ...await original<typeof import("../state/store")>(), useApp: () => ({ state: mocks.state, dispatch: mocks.dispatch }) }));
vi.mock("../lib/usePromptSkills", () => ({ usePromptSkills: () => ({ skills: [], error: "" }) }));
vi.mock("../lib/tauri", () => ({ createAutomation: vi.fn(), createProject: vi.fn(), deleteAutomation: vi.fn(), listGoModels: vi.fn(), runAutomationNow: vi.fn(), setAutomationEnabled: vi.fn(), updateAutomation: vi.fn() }));

beforeEach(() => {
  vi.clearAllMocks();
  const storage = new Map<string, string>();
  vi.stubGlobal("localStorage", { getItem: (key: string) => storage.get(key) ?? null, setItem: (key: string, value: string) => storage.set(key, value) });
  mocks.state = { ...initialState, activeThreadId: "chat-1", activeProjectRoot: "/repo", automations: [
    { id: "a1", name: "Morning review", project_root: "/repo", target_thread_id: "chat-1", provider: "go", model: "", skill_ids: [], interval_mins: 60, task: "Review pull requests", enabled: true, last_run_at: null, next_run_at: "2026-10-08T07:00:00Z", run_count: 0, schedule: { repeat: "weekdays", time: "09:00", timezone: "Europe/Berlin", weekday: 0 } },
    { id: "a2", name: "Inbox cleanup", project_root: "/repo", provider: "go", model: "", skill_ids: [], interval_mins: 30, task: "Triage unread mail", enabled: false, last_run_at: null, next_run_at: "2026-10-08T07:00:00Z", run_count: 0 },
  ] };
});
afterEach(() => { cleanup(); vi.unstubAllGlobals(); });

function expectNoAutomationMutation() {
  for (const action of [api.createAutomation, api.updateAutomation, api.deleteAutomation, api.runAutomationNow, api.setAutomationEnabled]) expect(action).not.toHaveBeenCalled();
}

describe("Automation navigation", () => {
  it("opens the direct form without agent help or mutations", () => {
    render(<Automations />);
    expect(screen.queryByRole("button", { name: "Ask Themis" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "+ New automation" }));
    const dialog = screen.getByRole("dialog", { name: "New automation" });
    expect(within(dialog).getByRole("textbox", { name: "Instructions" })).toBeInTheDocument();
    expect(within(dialog).queryByRole("button", { name: "Ask Themis" })).toBeNull();
    expectNoAutomationMutation();
  });

  it("filters names, instructions, schedule and paused status with a recoverable empty result", () => {
    render(<Automations />);
    const search = screen.getByRole("searchbox", { name: "Search automations" });
    fireEvent.change(search, { target: { value: " unread " } });
    expect(screen.queryByText("Morning review")).toBeNull();
    expect(screen.getByText("Inbox cleanup")).toBeInTheDocument();
    fireEvent.change(search, { target: { value: "Weekdays" } });
    expect(screen.getByText("Morning review")).toBeInTheDocument();
    expect(screen.queryByText("Inbox cleanup")).toBeNull();
    fireEvent.change(search, { target: { value: "Paused" } });
    expect(screen.getByText("Inbox cleanup")).toBeInTheDocument();
    fireEvent.change(search, { target: { value: "missing" } });
    expect(screen.getByText("No matching automations")).toBeInTheDocument();
    fireEvent.change(search, { target: { value: "" } });
    expect(screen.getByText("Morning review")).toBeInTheDocument();
    expect(screen.getByText("Inbox cleanup")).toBeInTheDocument();
  });

  it("keeps direct editing available without a selected chat", () => {
    mocks.state = { ...mocks.state, activeThreadId: null };
    render(<Automations />);
    fireEvent.click(screen.getAllByRole("button", { name: "Edit" })[1]);
    const dialog = screen.getByRole("dialog", { name: "Edit automation" });
    expect(within(dialog).getByRole("checkbox", { name: "Enabled" })).not.toBeChecked();
    expect(within(dialog).queryByRole("button", { name: "Ask Themis" })).toBeNull();
    expectNoAutomationMutation();
  });
});
