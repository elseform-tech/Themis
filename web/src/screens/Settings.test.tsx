// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { initialState, type AppState } from "../state/reducer";
import * as bridge from "../lib/tauri";
import { SettingsScreen } from "./Settings";
import { SettingsNav } from "./SettingsNav";

const dispatch = vi.fn();
const settings = { ...initialState.settings, go_secret: "OPENCODE_KEY" };
let goKeyCheck: AppState["goKeyCheck"] = "works";
/** The section the store holds; the sidebar's SettingsNav changes it, the pane shows it. */
let focus: { section: string } = { section: "Models" };
vi.mock("../state/store", async importOriginal => ({
  ...await importOriginal<typeof import("../state/store")>(),
  useApp: () => ({
    state: { ...initialState, settings, settingsLoaded: true, secretStatus: { go: true }, goKeyCheck, settingsFocus: focus, activeProjectRoot: "/p" },
    dispatch,
  }),
}));
vi.mock("../lib/tauri", () => ({
  checkForUpdates: vi.fn(),
  notificationPermission: vi.fn(async () => "granted"),
  clearSecret: vi.fn(async () => undefined),
  getSecretStatus: vi.fn(async () => ({ go: true })),
  setSecret: vi.fn(async () => undefined),
  updateSettings: vi.fn(async (patch: object) => ({ ...settings, ...patch })),
  listGoModels: vi.fn(async () => [{ id: "muse-spark-1.3-contributor", name: "Muse Spark" }]),
  checkGoKey: vi.fn(async () => ({ state: "works", secret: "OPENCODE_KEY" })),
  listModelProviders: vi.fn(async () => []),
  listKeys: vi.fn(async () => [
    { name: "OPENCODE_KEY", saved: true, environment: false, used_by: ["OpenCode Go"] },
    { name: "WORK_GO_KEY", saved: true, environment: false, used_by: [] },
  ]),
}));

beforeEach(() => {
  dispatch.mockClear(); goKeyCheck = "works"; focus = { section: "Models" };
  vi.mocked(bridge.checkGoKey).mockClear(); vi.mocked(bridge.setSecret).mockClear(); vi.mocked(bridge.listGoModels).mockClear(); vi.mocked(bridge.listModelProviders).mockClear();
});
afterEach(cleanup);

/** A provider's row in Settings › Models (the model picker's option groups share its name). */
function providerRow(name: string): HTMLElement {
  return screen.getAllByRole("group", { name }).find(element => element.tagName === "DIV")!;
}

describe("Settings › Models", () => {
  it("shows the connection first, checks the key with OpenCode, and offers the models", async () => {
    await act(async () => { render(<SettingsScreen />); });
    const groups = [...document.querySelectorAll(".themis-settings-subtitle")].map(node => node.textContent);
    expect(groups).toEqual(["Providers", "Default model"]);
    expect(screen.getByText("Connected")).toBeInTheDocument();
    expect(bridge.checkGoKey).toHaveBeenCalledTimes(1);
    expect(dispatch).toHaveBeenCalledWith({ type: "provider/checked", state: "works" });
    expect(screen.getByRole("combobox", { name: "Model" })).toBeEnabled();
    // A working connection keeps the secret and its value tucked away until asked for.
    expect(screen.queryByRole("combobox", { name: "OpenCode Go secret" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Manage" }));
    expect(screen.getByRole("combobox", { name: "OpenCode Go secret" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Hide" }));
    expect(screen.queryByRole("combobox", { name: "OpenCode Go secret" })).toBeNull();
  });

  it("blocks choosing a model while OpenCode rejects the key", async () => {
    goKeyCheck = "rejected";
    vi.mocked(bridge.checkGoKey).mockResolvedValueOnce({ state: "rejected", secret: "OPENCODE_KEY" });
    await act(async () => { render(<SettingsScreen />); });
    expect(screen.getByText("Key rejected")).toBeInTheDocument();
    const model = screen.getByRole("combobox", { name: "Model" });
    expect(model).toBeDisabled();
    expect(model).toHaveTextContent("Fix the connection first");
    expect(bridge.listGoModels).not.toHaveBeenCalled();
    // Shown without asking, since the key needs fixing.
    expect(screen.getByRole("combobox", { name: "OpenCode Go secret" })).toBeInTheDocument();
  });

  it("replaces the value inline, confirms the save and checks it again", async () => {
    await act(async () => { render(<SettingsScreen />); });
    fireEvent.click(screen.getByRole("button", { name: "Manage" }));
    fireEvent.click(screen.getByRole("button", { name: "Replace" }));
    fireEvent.change(screen.getByLabelText("Value of OPENCODE_KEY"), { target: { value: "sk-new" } });
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Save" })); });
    expect(bridge.setSecret).toHaveBeenCalledWith("go", "sk-new");
    expect(screen.getByText("Saved OPENCODE_KEY")).toBeInTheDocument();
    expect(bridge.checkGoKey).toHaveBeenCalledTimes(2);
    expect(screen.queryByLabelText("Value of OPENCODE_KEY")).toBeNull();
  });

  it("chooses the secret OpenCode Go uses and saves it as a setting", async () => {
    await act(async () => { render(<SettingsScreen />); });
    fireEvent.click(screen.getByRole("button", { name: "Manage" }));
    const secret = screen.getByRole("combobox", { name: "OpenCode Go secret" });
    expect(within(secret).getAllByRole("option").map(option => option.textContent)).toEqual(["OPENCODE_KEY", "WORK_GO_KEY"]);
    await act(async () => { fireEvent.change(secret, { target: { value: "WORK_GO_KEY" } }); });
    // Kept in app settings, so it applies every time the app opens.
    expect(bridge.updateSettings).toHaveBeenCalledWith({ go_secret: "WORK_GO_KEY" });
    expect(dispatch).toHaveBeenCalledWith({ type: "settings/patched", settings: expect.objectContaining({ go_secret: "WORK_GO_KEY" }) });
  });

  it("lists the providers from runtime.jsonc with their secret and status, and their models by provider", async () => {
    vi.mocked(bridge.listModelProviders).mockResolvedValueOnce([
      { id: "local", name: "Local server", secret: "LOCAL_KEY", status: "works", models: 1 },
      { id: "spare", name: "Spare", secret: "SPARE_KEY", status: "missing", models: 0 },
    ]);
    vi.mocked(bridge.listGoModels).mockResolvedValueOnce([
      { id: "muse-spark-1.3-contributor", effort_levels: [] },
      { id: "local/qwen/coder", effort_levels: [], provider: "local" },
    ]);
    await act(async () => { render(<SettingsScreen />); });
    const local = providerRow("Local server");
    expect(local).toHaveTextContent("LOCAL_KEY");
    expect(within(local).getByRole("status")).toHaveTextContent("Connected");
    expect(within(providerRow("Spare")).getByRole("status")).toHaveTextContent("Not set");
    const picker = screen.getByRole("combobox", { name: "Model" });
    const groups = [...picker.querySelectorAll("optgroup")].map(group => [group.label, [...group.querySelectorAll("option")].map(option => option.textContent)]);
    expect(groups).toEqual([["OpenCode Go", ["muse-spark-1.3-contributor"]], ["Local server", ["qwen/coder"]]]);
    await act(async () => { fireEvent.change(picker, { target: { value: "local/qwen/coder" } }); });
    expect(bridge.updateSettings).toHaveBeenCalledWith({ default_model: "local/qwen/coder" });
  });

  it("offers a connected provider's models while OpenCode rejects its key", async () => {
    goKeyCheck = "rejected";
    vi.mocked(bridge.checkGoKey).mockResolvedValueOnce({ state: "rejected", secret: "OPENCODE_KEY" });
    vi.mocked(bridge.listModelProviders).mockResolvedValueOnce([{ id: "local", name: "Local server", secret: "LOCAL_KEY", status: "unchecked", models: 1 }]);
    vi.mocked(bridge.listGoModels).mockResolvedValueOnce([{ id: "local/m", effort_levels: [], provider: "local" }]);
    await act(async () => { render(<SettingsScreen />); });
    expect(within(providerRow("Local server")).getByRole("status")).toHaveTextContent("Key set");
    expect(screen.getByRole("combobox", { name: "Model" })).toBeEnabled();
    expect(bridge.listGoModels).toHaveBeenCalled();
  });

  it("names the section Secrets", async () => {
    await act(async () => { render(<SettingsNav />); });
    expect(screen.getByRole("button", { name: "Secrets" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Keys" })).toBeNull();
  });
});

describe("Settings section list", () => {
  it("is not part of the pane: the pane shows only the chosen section's name as its heading", async () => {
    await act(async () => { render(<SettingsScreen />); });
    expect(screen.queryByRole("navigation", { name: "Settings sections" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Back to app" })).toBeNull();
    expect(screen.getByRole("heading", { level: 2 })).toHaveTextContent("Models");
  });

  it("lists Back to app once, then every section, and marks the chosen one", async () => {
    await act(async () => { render(<SettingsNav />); });
    const nav = screen.getByRole("navigation", { name: "Settings sections" });
    expect(within(nav).getAllByRole("button").map(button => button.getAttribute("aria-label"))).toEqual(["Back to app", "General", "Appearance", "Models", "Usage", "Secrets", "Permissions", "Browser", "Web", "Advanced"]);
    expect(within(nav).getByRole("button", { name: "Models" })).toHaveAttribute("aria-current", "page");
    expect(within(nav).getByRole("button", { name: "General" })).not.toHaveAttribute("aria-current");
  });

  it("switches the section through the store and goes back to the chat", async () => {
    await act(async () => { render(<SettingsNav />); });
    fireEvent.click(screen.getByRole("button", { name: "Browser" }));
    expect(dispatch).toHaveBeenCalledWith({ type: "ui/settings-section", section: "Browser" });
    fireEvent.click(screen.getByRole("button", { name: "Back to app" }));
    expect(dispatch).toHaveBeenCalledWith({ type: "ui/view", view: "thread" });
  });
});

describe("Settings › General › Notifications", () => {
  let platform: ReturnType<typeof vi.spyOn>;
  beforeEach(() => {
    platform = vi.spyOn(navigator, "platform", "get").mockReturnValue("MacIntel");
    vi.mocked(bridge.updateSettings).mockClear();
    vi.mocked(bridge.notificationPermission).mockClear().mockResolvedValue("granted");
  });
  afterEach(() => { platform.mockRestore(); settings.goal_notifications = true; });

  async function openGeneral() {
    focus = { section: "General" };
    return await act(async () => render(<SettingsScreen />));
  }

  it("holds the sound, haptics and goal switches; Appearance no longer has them", async () => {
    const { rerender } = await openGeneral();
    const group = screen.getByRole("region", { name: "Notifications" });
    expect(within(group).getAllByRole("checkbox").map(box => box.closest("label")?.textContent)).toEqual(["Sound when a reply finishes", "Haptics", "Notify when a goal finishes or pauses"]);
    expect(within(group).getByLabelText("Notify when a goal finishes or pauses")).toBeChecked();
    // Checking the status on open never asks the system.
    expect(bridge.notificationPermission).toHaveBeenCalledWith(false);
    expect(bridge.notificationPermission).not.toHaveBeenCalledWith(true);
    expect(within(group).queryByText(/System Settings/)).toBeNull();
    focus = { section: "Appearance" };
    await act(async () => { rerender(<SettingsScreen />); });
    expect(screen.queryByRole("region", { name: "Notifications" })).toBeNull();
    expect(screen.queryByRole("region", { name: "Feedback" })).toBeNull();
    expect(screen.queryByText("Sound when a reply finishes")).toBeNull();
  });

  it("saves the switch off without asking for permission", async () => {
    await openGeneral();
    vi.mocked(bridge.notificationPermission).mockClear();
    await act(async () => { fireEvent.click(screen.getByLabelText("Notify when a goal finishes or pauses")); });
    expect(bridge.updateSettings).toHaveBeenCalledWith({ goal_notifications: false });
    expect(bridge.notificationPermission).not.toHaveBeenCalledWith(true);
  });

  it("asks for permission when turned on and hints once when it is refused", async () => {
    settings.goal_notifications = false;
    vi.mocked(bridge.notificationPermission).mockResolvedValue("denied");
    await openGeneral();
    expect(bridge.notificationPermission).not.toHaveBeenCalled();
    await act(async () => { fireEvent.click(screen.getByLabelText("Notify when a goal finishes or pauses")); });
    expect(bridge.updateSettings).toHaveBeenCalledWith({ goal_notifications: true });
    expect(bridge.notificationPermission).toHaveBeenCalledWith(true);
    // The stored setting is still off in this mock, so the hint shows only for an on switch.
    cleanup();
    settings.goal_notifications = true;
    await openGeneral();
    expect(screen.getByText("Notifications are off for Themis in System Settings › Notifications.")).toBeInTheDocument();
  });

  it("shows only the sound switch outside macOS", async () => {
    platform.mockReturnValue("Win32");
    await openGeneral();
    const group = screen.getByRole("region", { name: "Notifications" });
    expect(within(group).getAllByRole("checkbox")).toHaveLength(1);
    expect(bridge.notificationPermission).not.toHaveBeenCalled();
  });
});

describe("Settings › Advanced › Working memory", () => {
  afterEach(() => { settings.context_token_cap = 0; vi.mocked(bridge.updateSettings).mockClear(); });

  async function openAdvanced() {
    focus = { section: "Advanced" };
    await act(async () => { render(<SettingsScreen />); });
  }

  it("is Auto by default, with no number and no recent-messages control", async () => {
    await openAdvanced();
    expect(screen.getByRole("combobox", { name: "Working memory" })).toHaveValue("auto");
    expect(screen.queryByLabelText("Working memory tokens")).toBeNull();
    expect(screen.queryByLabelText("Recent messages")).toBeNull();
    await act(async () => { fireEvent.change(screen.getByRole("combobox", { name: "Working memory" }), { target: { value: "custom" } }); });
    expect(bridge.updateSettings).toHaveBeenLastCalledWith({ context_token_cap: 200000 });
  });

  it("shows a custom cap, saves an edited number, and goes back to Auto", async () => {
    settings.context_token_cap = 300000;
    await openAdvanced();
    const cap = screen.getByLabelText("Working memory tokens");
    expect(cap).toHaveValue(300000);
    expect(cap).toHaveAttribute("min", "200000");
    expect(cap).toHaveAttribute("max", "1000000");
    fireEvent.change(cap, { target: { value: "500000" } });
    await act(async () => { fireEvent.blur(cap); });
    expect(bridge.updateSettings).toHaveBeenLastCalledWith({ context_token_cap: 500000 });
    await act(async () => { fireEvent.change(screen.getByRole("combobox", { name: "Working memory" }), { target: { value: "auto" } }); });
    expect(bridge.updateSettings).toHaveBeenLastCalledWith({ context_token_cap: 0 });
  });

  it("names the field in a rejected cap's error", async () => {
    settings.context_token_cap = 300000;
    vi.mocked(bridge.updateSettings).mockRejectedValueOnce("context_token_cap must be Auto or between 200000 and 1000000");
    await openAdvanced();
    const cap = screen.getByLabelText("Working memory tokens");
    fireEvent.change(cap, { target: { value: "10" } });
    await act(async () => { fireEvent.blur(cap); });
    expect(screen.getByRole("alert")).toHaveTextContent("Working memory must be Auto or between 200000 and 1000000");
  });
});
