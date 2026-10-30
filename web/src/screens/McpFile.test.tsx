// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { initialState } from "../state/reducer";
import * as bridge from "../lib/tauri";
import { McpFileRow } from "./McpFile";

const dispatch = vi.fn();
vi.mock("../state/store", async importOriginal => ({
  ...await importOriginal<typeof import("../state/store")>(),
  useApp: () => ({ state: { ...initialState, settings: { ...initialState.settings, config_editor: "zed" } }, dispatch }),
}));
vi.mock("../lib/tauri", () => ({
  listEditors: vi.fn(async () => ({ editors: [{ id: "", name: "Text editor" }, { id: "zed", name: "Zed" }] })),
  mcpConfigStatus: vi.fn(async () => ({ state: "closed" })),
  openMcpConfig: vi.fn(async () => ({ state: "open", servers: 2 })),
  updateSettings: vi.fn(async (patch: object) => ({ ...initialState.settings, ...patch })),
}));
afterEach(() => { cleanup(); vi.clearAllMocks(); vi.useRealTimers(); });

describe("Integrations › Connections › Edit as mcp.json", () => {
  it("opens the copy with the chosen editor and follows its saves", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const onApplied = vi.fn();
    await act(async () => { render(<McpFileRow onApplied={onApplied} />); });
    expect(screen.getByRole("combobox", { name: "Open with" })).toHaveValue("zed");
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Open" })); });
    expect(bridge.openMcpConfig).toHaveBeenCalledWith("zed");
    expect(screen.getByRole("status")).toHaveTextContent("Open · saves apply");
    vi.mocked(bridge.mcpConfigStatus).mockResolvedValue({ state: "applied", changed: ["personal"] });
    await act(async () => { await vi.advanceTimersByTimeAsync(1600); });
    expect(screen.getByRole("status")).toHaveTextContent("Applied · personal");
    expect(onApplied).toHaveBeenCalled();
    vi.mocked(bridge.mcpConfigStatus).mockResolvedValue({ state: "invalid", line: 3, message: "expected value" });
    await act(async () => { await vi.advanceTimersByTimeAsync(1600); });
    expect(screen.getByRole("status")).toHaveTextContent("Invalid · line 3: expected value");
  });

  it("remembers the editor choice", async () => {
    await act(async () => { render(<McpFileRow onApplied={() => {}} />); });
    await act(async () => { fireEvent.change(screen.getByRole("combobox", { name: "Open with" }), { target: { value: "" } }); });
    expect(bridge.updateSettings).toHaveBeenCalledWith({ config_editor: "" });
  });
});
