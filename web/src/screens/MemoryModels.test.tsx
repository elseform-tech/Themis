// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import * as bridge from "../lib/tauri";
import type { GoModel, MemorySettings } from "../lib/types";
import { MemoryModels } from "./MemoryModels";

vi.mock("../lib/tauri", () => ({
  getMemorySettings: vi.fn(),
  updateMemorySettings: vi.fn(),
}));
afterEach(() => { cleanup(); vi.clearAllMocks(); });

const SETTINGS: MemorySettings = { enabled: true, models: ["muse-spark-1.3-contributor", "claude-haiku-5-5", "deepseek-v4.1-flash"], daily_budget_usd: 0.5, consolidate_every_hours: 24 };
const CATALOG = ["muse-spark-1.3-contributor", "claude-haiku-5-5", "deepseek-v4.1-flash", "qwen3.8-flash"].map(id => ({ id }) as GoModel);

function setup(settings: MemorySettings = SETTINGS) {
  vi.mocked(bridge.getMemorySettings).mockResolvedValue({ settings, revision: "s1" });
  vi.mocked(bridge.updateMemorySettings).mockImplementation(async changes => ({ settings: { ...settings, ...changes }, revision: "s2" }));
}

const rows = () => within(screen.getByRole("list", { name: "Memory models" })).getAllByRole("listitem").map(item => item.querySelector(".themis-memory-model-name")!.textContent);

describe("Settings › Models › Memory models", () => {
  it("lists the models in order with the shared daily cap", async () => {
    setup();
    await act(async () => { render(<MemoryModels models={CATALOG} />); });
    expect(rows()).toEqual(["muse-spark-1.3-contributor", "claude-haiku-5-5", "deepseek-v4.1-flash"]);
    expect(screen.getByRole("heading", { name: "Memory models" })).toHaveAttribute("title", expect.stringContaining("up to $0.50 a day together"));
  });

  it("can't drop below three: Remove is off at three", async () => {
    setup();
    await act(async () => { render(<MemoryModels models={CATALOG} />); });
    for (const button of screen.getAllByRole("button", { name: /^Remove / })) expect(button).toBeDisabled();
  });

  it("adds a model at the end and writes only memory.models", async () => {
    setup();
    await act(async () => { render(<MemoryModels models={CATALOG} />); });
    const add = screen.getByRole("combobox", { name: "Add memory model" });
    // Models already listed aren't offered again.
    expect(within(add).getAllByRole("option").map(option => option.textContent)).toEqual(["Add model", "qwen3.8-flash"]);
    await act(async () => { fireEvent.change(add, { target: { value: "qwen3.8-flash" } }); });
    expect(bridge.updateMemorySettings).toHaveBeenCalledWith({ models: [...SETTINGS.models, "qwen3.8-flash"] }, "s1");
    expect(rows()).toHaveLength(4);
    // Four: each may now be removed.
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Remove claude-haiku-5-5" })); });
    expect(bridge.updateMemorySettings).toHaveBeenLastCalledWith({ models: ["muse-spark-1.3-contributor", "deepseek-v4.1-flash", "qwen3.8-flash"] }, "s2");
  });

  it("reorders: Move up and Move down swap neighbours, the ends can't go further", async () => {
    setup();
    await act(async () => { render(<MemoryModels models={CATALOG} />); });
    expect(screen.getByRole("button", { name: "Move muse-spark-1.3-contributor up" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Move deepseek-v4.1-flash down" })).toBeDisabled();
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Move claude-haiku-5-5 up" })); });
    expect(bridge.updateMemorySettings).toHaveBeenCalledWith({ models: ["claude-haiku-5-5", "muse-spark-1.3-contributor", "deepseek-v4.1-flash"] }, "s1");
    expect(rows()[0]).toBe("claude-haiku-5-5");
  });

  it("shows a refused write and the file's list again", async () => {
    setup();
    vi.mocked(bridge.updateMemorySettings).mockRejectedValueOnce(new Error("Memory settings changed in runtime.jsonc since this page loaded; showing the latest"));
    await act(async () => { render(<MemoryModels models={CATALOG} />); });
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Move claude-haiku-5-5 down" })); });
    expect(screen.getByRole("alert")).toHaveTextContent("changed in runtime.jsonc");
    expect(bridge.getMemorySettings).toHaveBeenCalledTimes(2);
  });
});
