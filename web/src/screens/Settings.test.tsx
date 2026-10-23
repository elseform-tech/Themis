// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { initialState } from "../state/reducer";
import * as bridge from "../lib/tauri";
import { SettingsScreen } from "./Settings";

const dispatch = vi.fn();
const settings = { ...initialState.settings, go_secret: "OPENCODE_KEY" };
vi.mock("../state/store", async importOriginal => ({
  ...await importOriginal<typeof import("../state/store")>(),
  useApp: () => ({
    state: { ...initialState, settings, settingsLoaded: true, secretStatus: { go: true }, settingsFocus: { section: "Models" }, activeProjectRoot: "/p" },
    dispatch,
  }),
}));
vi.mock("../lib/tauri", () => ({
  checkForUpdates: vi.fn(),
  clearSecret: vi.fn(async () => undefined),
  getSecretStatus: vi.fn(async () => ({ go: false })),
  setSecret: vi.fn(async () => undefined),
  updateSettings: vi.fn(async (patch: object) => ({ ...settings, ...patch })),
  listGoModels: vi.fn(async () => []),
  checkGoKey: vi.fn(async () => ({ state: "works", secret: "OPENCODE_KEY" })),
  listKeys: vi.fn(async () => [
    { name: "OPENCODE_KEY", saved: true, environment: false, used_by: ["OpenCode Go"] },
    { name: "WORK_GO_KEY", saved: true, environment: false, used_by: [] },
  ]),
}));

beforeEach(() => { dispatch.mockClear(); vi.mocked(bridge.checkGoKey).mockClear(); });
afterEach(cleanup);

describe("Settings › Models › Connections", () => {
  it("chooses the secret OpenCode Go uses and saves it as a setting", async () => {
    await act(async () => { render(<SettingsScreen />); });
    const secret = screen.getByRole("combobox", { name: "OpenCode Go secret" });
    expect(secret).toHaveValue("OPENCODE_KEY");
    expect(within(secret).getAllByRole("option").map(option => option.textContent)).toEqual(["OPENCODE_KEY", "WORK_GO_KEY"]);
    await act(async () => { fireEvent.change(secret, { target: { value: "WORK_GO_KEY" } }); });
    // Saved in app settings (kept across launches), then the connection is checked again.
    expect(bridge.updateSettings).toHaveBeenCalledWith({ go_secret: "WORK_GO_KEY" });
    expect(dispatch).toHaveBeenCalledWith({ type: "settings/patched", settings: expect.objectContaining({ go_secret: "WORK_GO_KEY" }) });
    expect(bridge.getSecretStatus).toHaveBeenCalled();
    expect(dispatch).toHaveBeenCalledWith({ type: "secrets/loaded", status: { go: false } });
  });

  it("checks the key with OpenCode and says so, and confirms a save inline", async () => {
    await act(async () => { render(<SettingsScreen />); });
    expect(screen.getByText("Connected")).toBeInTheDocument();
    expect(bridge.checkGoKey).toHaveBeenCalledTimes(1);

    vi.mocked(bridge.checkGoKey).mockResolvedValueOnce({ state: "rejected", secret: "OPENCODE_KEY" });
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Check" })); });
    expect(screen.getByText("Key rejected")).toBeInTheDocument();

    fireEvent.click(screen.getByText("Set OPENCODE_KEY"));
    fireEvent.change(screen.getByLabelText("Value of OPENCODE_KEY"), { target: { value: "sk-new" } });
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Save" })); });
    expect(bridge.setSecret).toHaveBeenCalledWith("go", "sk-new");
    expect(screen.getByText("Saved OPENCODE_KEY")).toBeInTheDocument();
    // Saving checks the new key right away.
    expect(bridge.checkGoKey).toHaveBeenCalledTimes(3);
    expect(screen.getByText("Connected")).toBeInTheDocument();
  });

  it("names the section Secrets", async () => {
    await act(async () => { render(<SettingsScreen />); });
    expect(screen.getByRole("button", { name: "Secrets" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Keys" })).toBeNull();
  });
});
