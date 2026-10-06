// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { initialState, type AppState } from "../state/reducer";
import { readSession, writeSession } from "../state/session";
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
  it("opens an editable agent draft and preserves an existing chat draft without executing anything", () => {
    writeSession("drafts", { "chat-1": "Existing notes", "other-chat": "Keep me" });
    render(<Automations />);
    fireEvent.click(screen.getByRole("button", { name: "Ask Themis" }));
    const drafts = readSession<Record<string, string>>("drafts", {});
    expect(drafts["chat-1"]).toContain("[[skill:manage-automations]]");
    expect(drafts["chat-1"]).toContain("Existing notes");
    expect(drafts["other-chat"]).toBe("Keep me");
    expect(mocks.dispatch).toHaveBeenCalledWith({ type: "ui/view", view: "thread" });
    expectNoAutomationMutation();
  });

  it("carries instructions and the selected schedule from the simple form into the draft", () => {
    render(<Automations />);
    fireEvent.click(screen.getByRole("button", { name: "+ New automation" }));
    const dialog = screen.getByRole("dialog", { name: "New automation" });
    const instructions = within(dialog).getByRole("textbox", { name: "Instructions" });
    instructions.textContent = "Review important changes";
    fireEvent.input(instructions);
    fireEvent.change(within(dialog).getByRole("combobox", { name: "Repeat" }), { target: { value: "weekly" } });
    fireEvent.change(within(dialog).getByLabelText("Time", { exact: true }), { target: { value: "17:30" } });
    fireEvent.click(within(dialog).getByRole("checkbox", { name: "Enabled" }));
    fireEvent.click(within(dialog).getByRole("button", { name: "Ask Themis" }));
    const draft = readSession<Record<string, string>>("drafts", {})["chat-1"];
    expect(draft).toContain("Review important changes");
    expect(draft).toContain("17:30");
    expect(draft).toContain("Monday");
    expect(draft).toContain("Status: paused");
    expect(draft).toContain("Run in: chat chat-1");
    expect(screen.queryByRole("dialog")).toBeNull();
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

  it("preserves the name, paused status and new-chat model selections when editing through the agent", () => {
    mocks.state.automations[1] = { ...mocks.state.automations[1], model: "chosen-model", reasoning_effort: "high" };
    render(<Automations />);
    fireEvent.click(screen.getAllByRole("button", { name: "Edit" })[1]);
    const dialog = screen.getByRole("dialog", { name: "Edit automation" });
    fireEvent.click(within(dialog).getByRole("button", { name: "Ask Themis" }));
    const draft = readSession<Record<string, string>>("drafts", {})["chat-1"];
    for (const detail of ["id: a2", "Name: Inbox cleanup", "Status: paused", "Every 30 minutes", "a new chat each run", "Model: chosen-model", "Reasoning effort: high"]) expect(draft).toContain(detail);
    expectNoAutomationMutation();
  });

  it("requires a current chat for agent handoff", () => {
    mocks.state = { ...mocks.state, activeThreadId: null };
    render(<Automations />);
    expect(screen.getByRole("button", { name: "Ask Themis" })).toBeDisabled();
    expectNoAutomationMutation();
  });
});
