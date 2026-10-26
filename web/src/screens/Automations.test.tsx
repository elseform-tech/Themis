// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
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
  vi.mocked(api.listGoModels).mockResolvedValue([]);
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

  it("hides search for a short list", () => {
    render(<Automations />);
    expect(screen.queryByRole("searchbox", { name: "Search automations" })).toBeNull();
    expect(screen.getByRole("checkbox", { name: "Morning review enabled" })).toBeChecked();
    expect(screen.getByText(/Every 30 minutes/)).toBeInTheDocument();
  });

  it("filters names, instructions, schedule and paused status with a recoverable empty result", () => {
    const extra = mocks.state.automations[1]!;
    mocks.state = { ...mocks.state, automations: [...mocks.state.automations, ...["Build", "Lint", "Docs"].map(name => ({ ...extra, id: name, name, task: `${name} check`, enabled: true }))] };
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

  it("offers a configured provider's models without an OpenCode Go key", async () => {
    mocks.state = { ...mocks.state, activeThreadId: null, secretStatus: { ...mocks.state.secretStatus, go: false } };
    vi.mocked(api.listGoModels).mockResolvedValue([{ id: "local/qwen3-coder", name: "Qwen3 Coder" } as never]);
    render(<Automations />);
    fireEvent.click(screen.getByRole("button", { name: "+ New automation" }));
    const model = within(screen.getByRole("dialog", { name: "New automation" })).getByRole("combobox", { name: "Model" });
    await waitFor(() => expect(model).toBeEnabled());
    expect(within(model).getByRole("option", { name: "local/qwen3-coder" })).toBeInTheDocument();
  });

  it("says once how to get models when there is no key and no provider", async () => {
    mocks.state = { ...mocks.state, activeThreadId: null, secretStatus: { ...mocks.state.secretStatus, go: false } };
    vi.mocked(api.listGoModels).mockRejectedValue("no API key stored for provider 'go'");
    render(<Automations />);
    fireEvent.click(screen.getByRole("button", { name: "+ New automation" }));
    const dialog = screen.getByRole("dialog", { name: "New automation" });
    await waitFor(() => expect(within(dialog).getByRole("option", { name: "Connect OpenCode Go or add a provider to load models" })).toBeInTheDocument());
    expect(within(dialog).queryByText(/no API key stored/)).toBeNull();
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
