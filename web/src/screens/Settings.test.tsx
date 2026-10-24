// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { initialState, type AppState } from "../state/reducer";
import * as bridge from "../lib/tauri";
import { SettingsScreen } from "./Settings";

const dispatch = vi.fn();
const settings = { ...initialState.settings, go_secret: "OPENCODE_KEY" };
let goKeyCheck: AppState["goKeyCheck"] = "works";
vi.mock("../state/store", async importOriginal => ({
  ...await importOriginal<typeof import("../state/store")>(),
  useApp: () => ({
    state: { ...initialState, settings, settingsLoaded: true, secretStatus: { go: true }, goKeyCheck, settingsFocus: { section: "Models" }, activeProjectRoot: "/p" },
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
  listKeys: vi.fn(async () => [
    { name: "OPENCODE_KEY", saved: true, environment: false, used_by: ["OpenCode Go"] },
    { name: "WORK_GO_KEY", saved: true, environment: false, used_by: [] },
  ]),
}));

beforeEach(() => {
  dispatch.mockClear(); goKeyCheck = "works";
  vi.mocked(bridge.checkGoKey).mockClear(); vi.mocked(bridge.setSecret).mockClear(); vi.mocked(bridge.listGoModels).mockClear();
});
afterEach(cleanup);

describe("Settings › Models", () => {
  it("shows the connection first, checks the key with OpenCode, and offers the models", async () => {
    await act(async () => { render(<SettingsScreen />); });
    const groups = [...document.querySelectorAll(".themis-settings-subtitle")].map(node => node.textContent);
    expect(groups).toEqual(["Connection", "Default model"]);
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

  it("names the section Secrets", async () => {
    await act(async () => { render(<SettingsScreen />); });
    expect(screen.getByRole("button", { name: "Secrets" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Keys" })).toBeNull();
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
    await act(async () => { render(<SettingsScreen />); });
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "General" })); });
  }

  it("holds the sound, haptics and goal switches; Appearance no longer has them", async () => {
    await openGeneral();
    const group = screen.getByRole("region", { name: "Notifications" });
    expect(within(group).getAllByRole("checkbox").map(box => box.closest("label")?.textContent)).toEqual(["Sound when a reply finishes", "Haptics", "Notify when a goal finishes or pauses"]);
    expect(within(group).getByLabelText("Notify when a goal finishes or pauses")).toBeChecked();
    // Checking the status on open never asks the system.
    expect(bridge.notificationPermission).toHaveBeenCalledWith(false);
    expect(bridge.notificationPermission).not.toHaveBeenCalledWith(true);
    expect(within(group).queryByText(/System Settings/)).toBeNull();
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Appearance" })); });
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
