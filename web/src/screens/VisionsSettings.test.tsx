// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { initialState } from "../state/reducer";
import type { VisionInfo, VisionsSettings } from "../lib/visions";
import * as visions from "../lib/visions";
import { VisionsSettingsSection } from "./VisionsSettings";

const vision = (id: string, name: string, paused = false): VisionInfo => ({
  id, name, role: "", shape: "flat", color: "teal", soul: "", paused, revision: "r",
  over_budget: false, vision_chats: [], needs: 0, sorry: false,
});
let list: VisionInfo[] = [];
vi.mock("../state/store", async importOriginal => ({
  ...await importOriginal<typeof import("../state/store")>(),
  useApp: () => ({ state: { ...initialState, visions: { ...initialState.visions, list } }, dispatch: vi.fn() }),
}));
vi.mock("../lib/visions", async importOriginal => ({
  ...await importOriginal<typeof import("../lib/visions")>(),
  getVisionsSettings: vi.fn(),
  updateVisionsSettings: vi.fn(),
  pauseVision: vi.fn(async () => ({})),
}));

const base: VisionsSettings = { enabled: true, autonomy: "small", custom: {}, daily_budget_usd: 2, max_wakes: 8 };
function serve(settings: Partial<VisionsSettings>) {
  vi.mocked(visions.getVisionsSettings).mockResolvedValue({ settings: { ...base, ...settings }, revision: "r1" });
  vi.mocked(visions.updateVisionsSettings).mockImplementation(async changes => ({ settings: { ...base, ...settings, ...changes } as VisionsSettings, revision: "r2" }));
}
async function open(settings: Partial<VisionsSettings>) {
  serve(settings);
  await act(async () => { render(<VisionsSettingsSection />); });
}
/** The answer a row shows, by the row's label. */
function row(label: string): HTMLElement {
  return screen.getByText(label).closest(".themis-visions-decision") as HTMLElement;
}

beforeEach(() => {
  list = [vision("rook", "Rook"), vision("wren", "Wren", true)];
  vi.mocked(visions.updateVisionsSettings).mockClear();
  vi.mocked(visions.pauseVision).mockClear();
});
afterEach(cleanup);

describe("Settings › Visions", () => {
  it("presets_are_read_only_and_custom_is_editable", async () => {
    await open({ autonomy: "small" });
    expect(screen.getByRole("slider", { name: "Autonomy" })).toHaveAttribute("aria-valuetext", "Small calls");
    expect(screen.queryAllByRole("radiogroup")).toHaveLength(0);
    expect(row("Wording, names and formatting")).toHaveTextContent("Vision decides");
    expect(row("Choices it can undo, inside the channel’s goal")).toHaveTextContent("Asks you");
    expect(row("A new dependency or a security setting")).toHaveTextContent("Asks you");
    cleanup();

    await open({ autonomy: "custom", custom: { wording: true } });
    expect(screen.getByRole("slider", { name: "Autonomy" })).toHaveAttribute("aria-valuetext", "Custom");
    expect(screen.getAllByRole("radiogroup")).toHaveLength(7);
    const wording = screen.getByRole("radiogroup", { name: "Wording, names and formatting" });
    expect(within(wording).getByRole("radio", { name: "Vision decides" })).toHaveAttribute("aria-checked", "true");
    const deleting = screen.getByRole("radiogroup", { name: "Deleting or replacing work" });
    expect(within(deleting).getByRole("radio", { name: "Asks you" })).toHaveAttribute("aria-checked", "true");
    await act(async () => { fireEvent.click(within(deleting).getByRole("radio", { name: "Vision decides" })); });
    expect(visions.updateVisionsSettings).toHaveBeenCalledWith({ custom: { wording: true, delete: true } }, "r1");
  });

  it("locked_rows_stay_locked_in_custom", async () => {
    await open({ autonomy: "custom", custom: { external: true, spend: true } });
    for (const label of ["Anything that leaves this computer", "Spending past the budget"]) {
      expect(screen.queryByRole("radiogroup", { name: label })).toBeNull();
      expect(row(label)).toHaveTextContent("Always you");
    }
    cleanup();
    await open({ autonomy: "nearly_all" });
    expect(row("Anything that leaves this computer")).toHaveTextContent("Always you");
    expect(row("Spending past the budget")).toHaveTextContent("Always you");
  });

  it("each_field_saves_alone_with_the_page_revision", async () => {
    await open({ autonomy: "small" });
    await act(async () => { fireEvent.change(screen.getByRole("slider", { name: "Autonomy" }), { target: { value: "3" } }); });
    expect(visions.updateVisionsSettings).toHaveBeenCalledWith({ autonomy: "most" }, "r1");
    const budget = screen.getByRole("spinbutton", { name: "Daily budget, all Visions" });
    fireEvent.change(budget, { target: { value: "5" } });
    await act(async () => { fireEvent.blur(budget); });
    // The next write names the revision the last one returned.
    expect(visions.updateVisionsSettings).toHaveBeenLastCalledWith({ daily_budget_usd: 5 }, "r2");
    expect(visions.updateVisionsSettings).toHaveBeenCalledTimes(2);
  });

  it("pauses every Vision with Pause all, and shows it on only when all are paused", async () => {
    await open({});
    const pauseAll = screen.getByRole("checkbox", { name: "Pause all Visions" });
    expect(pauseAll).not.toBeChecked();
    await act(async () => { fireEvent.click(pauseAll); });
    expect(visions.pauseVision).toHaveBeenCalledTimes(1);
    expect(visions.pauseVision).toHaveBeenCalledWith("rook", true);
    expect(visions.updateVisionsSettings).not.toHaveBeenCalled();
    cleanup();
    list = [vision("rook", "Rook", true), vision("wren", "Wren", true)];
    await open({});
    expect(screen.getByRole("checkbox", { name: "Pause all Visions" })).toBeChecked();
  });
});
