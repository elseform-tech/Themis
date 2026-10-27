// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { initialState, reducer } from "../state/reducer";
import type { BrowserSettings } from "../lib/types";
import * as bridge from "../lib/tauri";
import { BrowserSettingsSection } from "./BrowserSettings";

const dispatch = vi.fn();
vi.mock("../state/store", async importOriginal => ({
  ...await importOriginal<typeof import("../state/store")>(),
  useApp: () => ({ state: { ...initialState, projects: [{ root: "/p/chess", name: "Chess", is_git: true }], activeThreadId: "t1" }, dispatch }),
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(async () => "/Users/me/Desktop") }));
vi.mock("../lib/browser", () => ({ browserOpenUrl: vi.fn(async () => undefined) }));

const SETTINGS: BrowserSettings = {
  enabled: true,
  developer_mode: true,
  permissions: { browsing: "allow", downloads: "allow", debugging: "block" },
  site_exceptions: [{ site: "paypal.com", browsing: "block" }],
  show_secrets: false,
  open_workspace: true,
};
vi.mock("../lib/tauri", () => ({
  getBrowserSettings: vi.fn(),
  updateBrowserSettings: vi.fn(async (changes: Partial<BrowserSettings>) => ({ settings: { ...SETTINGS, ...changes }, revision: "r2" })),
  browserHistory: vi.fn(async () => ({ entries: [{ url: "http://localhost:5173/", title: "Chess", last_visit: 1_800_000_000, visits: 2, project: "/p/chess" }] })),
  removeBrowserHistory: vi.fn(async () => undefined),
  clearBrowsingData: vi.fn(async () => ({})),
}));

beforeEach(() => {
  vi.mocked(bridge.getBrowserSettings).mockResolvedValue({
    settings: structuredClone(SETTINGS),
    revision: "r1",
    browsers: ["/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"],
    default_downloads_directory: "/Users/me/Downloads",
    starter_sites: ["paypal.com", "stripe.com"],
  });
  vi.mocked(bridge.updateBrowserSettings).mockClear();
  dispatch.mockClear();
});
afterEach(cleanup);

async function renderSection() {
  await act(async () => { render(<BrowserSettingsSection />); });
}
const saved = () => vi.mocked(bridge.updateBrowserSettings).mock.lastCall![0];

describe("Settings › Browser", () => {
  it("shows the defaults Codex-style and saves a permission change", async () => {
    await renderSection();
    expect(screen.getByRole("combobox", { name: "Browsing" })).toHaveValue("allow");
    expect(screen.getByRole("combobox", { name: "Debugging" })).toHaveValue("block");
    expect(screen.getByRole("combobox", { name: "Browser" })).toHaveTextContent("Automatic (Google Chrome)");
    expect(screen.getByText("/Users/me/Downloads")).toBeInTheDocument();
    await act(async () => { fireEvent.change(screen.getByRole("combobox", { name: "Debugging" }), { target: { value: "ask" } }); });
    expect(saved().permissions!.debugging).toBe("ask");
    // Only the changed field is sent, with the revision the page loaded.
    expect(Object.keys(saved())).toEqual(["permissions"]);
    expect(vi.mocked(bridge.updateBrowserSettings).mock.lastCall![1]).toBe("r1");
    expect(dispatch).toHaveBeenCalledWith({ type: "browser/opens-workspace", value: true });
  });

  it("names the built-in engine as the automatic browser", async () => {
    vi.mocked(bridge.getBrowserSettings).mockResolvedValueOnce({ settings: structuredClone(SETTINGS), revision: "r1", browsers: ["/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"], built_in: true, default_downloads_directory: "/Users/me/Downloads", starter_sites: [] });
    await renderSection();
    const browser = screen.getByRole("combobox", { name: "Browser" });
    expect(within(browser).getAllByRole("option").map(option => option.textContent)).toEqual(["Built-in", "Google Chrome"]);
  });

  it("reloads instead of overwriting when runtime.jsonc changed meanwhile", async () => {
    await renderSection();
    vi.mocked(bridge.updateBrowserSettings).mockRejectedValueOnce(new Error("Browser settings changed in runtime.jsonc since this page loaded; showing the latest"));
    vi.mocked(bridge.getBrowserSettings).mockResolvedValueOnce({ settings: { ...structuredClone(SETTINGS), developer_mode: false }, revision: "r9", browsers: [], default_downloads_directory: "/Users/me/Downloads", starter_sites: [] });
    await act(async () => { fireEvent.change(screen.getByRole("combobox", { name: "Browsing" }), { target: { value: "ask" } }); });
    expect(screen.getByRole("alert")).toHaveTextContent("changed in runtime.jsonc");
    expect(screen.getByRole("checkbox", { name: "Enable full CDP access" })).not.toBeChecked();
    expect(screen.getByRole("combobox", { name: "Browsing" })).toHaveValue("allow");
  });

  it("greys out Debugging when Developer mode is off", async () => {
    await renderSection();
    await act(async () => { fireEvent.click(screen.getByRole("checkbox", { name: "Enable full CDP access" })); });
    expect(saved().developer_mode).toBe(false);
    expect(screen.getByRole("combobox", { name: "Debugging" })).toBeDisabled();
  });

  it("adds, changes, removes and restores site exceptions", async () => {
    await renderSection();
    fireEvent.change(screen.getByRole("textbox", { name: "Add a site" }), { target: { value: "https://Bank.example/login" } });
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Add site" })); });
    expect(saved().site_exceptions!).toContainEqual({ site: "bank.example", browsing: "block" });
    await act(async () => { fireEvent.change(screen.getByRole("combobox", { name: "Debugging on bank.example" }), { target: { value: "allow" } }); });
    expect(saved().site_exceptions!.find(e => e.site === "bank.example")?.debugging).toBe("allow");
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Remove paypal.com" })); });
    expect(saved().site_exceptions!.map(e => e.site)).toEqual(["bank.example"]);
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Restore defaults" })); });
    expect(saved().site_exceptions!.map(e => e.site)).toEqual(["paypal.com", "stripe.com", "bank.example"]);
  });

  it("changes and resets the downloads folder", async () => {
    await renderSection();
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Change…" })); });
    expect(saved().downloads_directory).toBe("/Users/me/Desktop");
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Reset" })); });
    expect(saved()).toHaveProperty("downloads_directory", undefined);
  });

  it("searches history, removes an entry and clears data after confirming", async () => {
    await renderSection();
    fireEvent.click(screen.getByRole("button", { name: "Manage…" }));
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Show history" })); });
    const list = screen.getByRole("list", { name: "Browser history" });
    expect(within(list).getByText("Chess")).toBeInTheDocument();
    await act(async () => { fireEvent.click(within(list).getByRole("button", { name: "Remove http://localhost:5173/" })); });
    expect(bridge.removeBrowserHistory).toHaveBeenCalledWith("/p/chess", "http://localhost:5173/");
    fireEvent.click(screen.getByRole("button", { name: "Clear browsing data…" }));
    expect(screen.getByText(/closes every project's browser/)).toBeInTheDocument();
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Clear" })); });
    expect(bridge.clearBrowsingData).toHaveBeenCalledWith(null, "hour", ["history", "cache"]);
  });
});

describe("Open Workspace when the agent starts browsing", () => {
  it("opens Workspace on the first browser step only when the setting is on", () => {
    const state = { ...initialState, activeThreadId: "t1", mainView: "thread" as const };
    const envelope = { thread_id: "t1", run_id: "r1", event: { kind: "tool_started", tool: "browser_open", summary: "open" } };
    // Off by default: the chat's overview box previews the browsing instead.
    expect(state.browserOpensWorkspace).toBe(false);
    const off = reducer(state, { type: "thread/event", envelope } as never);
    expect(off.diffPanelOpen).toBe(false);
    const on = reducer({ ...state, browserOpensWorkspace: true }, { type: "thread/event", envelope } as never);
    expect(on.diffPanelOpen).toBe(true);
  });
});
