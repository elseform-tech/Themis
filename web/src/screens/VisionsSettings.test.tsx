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
/** The file as the server holds it: each write must name its current revision (r1, r2, …). */
let file: { settings: VisionsSettings; revision: number };
function serve(settings: Partial<VisionsSettings>, canRun = true) {
  file = { settings: { ...base, ...settings }, revision: 1 };
  vi.mocked(visions.getVisionsSettings).mockImplementation(async () => ({ settings: file.settings, revision: `r${file.revision}`, can_run: canRun }));
  vi.mocked(visions.updateVisionsSettings).mockImplementation(async (changes, revision) => {
    if (revision !== `r${file.revision}`) throw new Error("Visions settings changed since this page loaded; reload before saving");
    file = { settings: { ...file.settings, ...changes } as VisionsSettings, revision: file.revision + 1 };
    return { settings: file.settings, revision: `r${file.revision}`, can_run: canRun };
  });
}
async function open(settings: Partial<VisionsSettings>, canRun = true) {
  serve(settings, canRun);
  await act(async () => { render(<VisionsSettingsSection />); });
}
/** Drags the slider over `stops` (1–5) and lets go. */
async function slide(...stops: number[]) {
  const slider = screen.getByRole("slider", { name: "Autonomy" });
  await act(async () => {
    for (const stop of stops) fireEvent.change(slider, { target: { value: String(stop) } });
    fireEvent.pointerUp(slider);
  });
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
    expect(screen.getByRole("slider", { name: "Autonomy" })).toHaveAttribute("aria-valuetext", "Routine decisions");
    expect(screen.queryAllByRole("radiogroup")).toHaveLength(0);
    expect(row("Wording, names and formatting")).toHaveTextContent("Decides");
    expect(row("Choices it can undo, inside the channel’s goal")).toHaveTextContent("Asks you");
    expect(row("A new dependency or a security setting")).toHaveTextContent("Asks you");
    cleanup();

    await open({ autonomy: "custom", custom: { wording: true } });
    expect(screen.getByRole("slider", { name: "Autonomy" })).toHaveAttribute("aria-valuetext", "Custom");
    expect(screen.getAllByRole("radiogroup")).toHaveLength(7);
    const wording = screen.getByRole("radiogroup", { name: "Wording, names and formatting" });
    expect(within(wording).getByRole("radio", { name: "Decides" })).toHaveAttribute("aria-checked", "true");
    const deleting = screen.getByRole("radiogroup", { name: "Deleting or replacing work" });
    expect(within(deleting).getByRole("radio", { name: "Asks you" })).toHaveAttribute("aria-checked", "true");
    await act(async () => { fireEvent.click(within(deleting).getByRole("radio", { name: "Decides" })); });
    expect(visions.updateVisionsSettings).toHaveBeenCalledWith({ custom: { wording: true, delete: true } }, "r1");
  });

  it("names the levels in plain professional words and shows no hint text", async () => {
    await open({});
    const stops = [...document.querySelectorAll(".themis-visions-stops span")].map(stop => stop.textContent);
    expect(stops).toEqual(["Always ask", "Routine decisions", "Most decisions", "Nearly all decisions", "Custom"]);
    expect(document.querySelector(".themis-visions-meta")).toBeNull();
    expect(screen.queryByText(/shown for reference|Approvals stay yours|What a Vision decides/)).toBeNull();
  });

  it("says in one line when Visions can't run on this computer, and nothing when they can", async () => {
    await open({});
    expect(screen.queryByRole("status")).toBeNull();
    cleanup();
    await open({}, false);
    expect(screen.getByRole("status").textContent).toBe("Visions can't run on this computer: it has no sandbox that can confine them.");
    // A write keeps it shown.
    await slide(4);
    expect(screen.getByRole("status")).toBeTruthy();
  });

  it("locked_rows_stay_locked_in_custom", async () => {
    await open({ autonomy: "custom", custom: { external: true, spend: true } });
    for (const label of ["Anything that leaves this computer", "Spending past the budget"]) {
      expect(screen.queryByRole("radiogroup", { name: label })).toBeNull();
      expect(row(label)).toHaveTextContent("Always asks you");
    }
    cleanup();
    await open({ autonomy: "nearly_all" });
    expect(row("Anything that leaves this computer")).toHaveTextContent("Always asks you");
    expect(row("Spending past the budget")).toHaveTextContent("Always asks you");
  });

  it("each_field_saves_alone_with_the_page_revision", async () => {
    await open({ autonomy: "small" });
    await slide(3);
    expect(visions.updateVisionsSettings).toHaveBeenCalledWith({ autonomy: "most" }, "r1");
    const budget = screen.getByRole("spinbutton", { name: "Daily budget, all Visions" });
    fireEvent.change(budget, { target: { value: "5" } });
    await act(async () => { fireEvent.blur(budget); });
    // The next write names the revision the last one returned.
    expect(visions.updateVisionsSettings).toHaveBeenLastCalledWith({ daily_budget_usd: 5 }, "r2");
    expect(visions.updateVisionsSettings).toHaveBeenCalledTimes(2);
  });

  it("saves a drag across several stops once, where it is let go", async () => {
    await open({ autonomy: "small" });
    await slide(3, 4);
    expect(visions.updateVisionsSettings).toHaveBeenCalledTimes(1);
    expect(visions.updateVisionsSettings).toHaveBeenCalledWith({ autonomy: "nearly_all" }, "r1");
    expect(screen.getByRole("slider", { name: "Autonomy" })).toHaveAttribute("aria-valuetext", "Nearly all decisions");
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("runs quick writes one at a time, each on the latest revision and rows", async () => {
    await open({ autonomy: "custom", custom: {} });
    await act(async () => {
      fireEvent.click(within(screen.getByRole("radiogroup", { name: "Deleting or replacing work" })).getByRole("radio", { name: "Decides" }));
      fireEvent.click(within(screen.getByRole("radiogroup", { name: "Changing the goal or the scope" })).getByRole("radio", { name: "Decides" }));
    });
    expect(vi.mocked(visions.updateVisionsSettings).mock.calls).toEqual([
      [{ custom: { delete: true } }, "r1"],
      [{ custom: { delete: true, scope: true } }, "r2"],
    ]);
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("starts Custom from the level it came from, so nothing changes until a row is edited", async () => {
    await open({ autonomy: "most", custom: {} });
    await slide(5);
    expect(visions.updateVisionsSettings).toHaveBeenCalledWith({
      autonomy: "custom",
      custom: { wording: true, tests: true, undoable: true, retry: true, scope: false, delete: false, dependency: false },
    }, "r1");
    const retry = screen.getByRole("radiogroup", { name: "Retrying or re-planning after a failure" });
    expect(within(retry).getByRole("radio", { name: "Decides" })).toHaveAttribute("aria-checked", "true");
  });

  it("shows a refused write and reads the file again", async () => {
    await open({ autonomy: "small" });
    // Someone edits runtime.jsonc after the page loaded.
    file = { settings: { ...file.settings, autonomy: "ask" }, revision: 2 };
    await slide(3);
    expect(screen.getByRole("alert")).toHaveTextContent("changed since this page loaded");
    expect(screen.getByRole("slider", { name: "Autonomy" })).toHaveAttribute("aria-valuetext", "Always ask");
  });

  it("keeps the budget within $0.10 to $100, saying so plainly", async () => {
    await open({});
    const budget = screen.getByRole("spinbutton", { name: "Daily budget, all Visions" });
    expect(budget).toHaveAttribute("min", "0.1");
    expect(budget).toHaveAttribute("max", "100");
    for (const value of ["200", "0.05"]) {
      fireEvent.change(budget, { target: { value } });
      await act(async () => { fireEvent.blur(budget); });
      expect(screen.getByRole("alert")).toHaveTextContent(/^Enter an amount from \$0\.10 to \$100$/);
    }
    expect(visions.updateVisionsSettings).not.toHaveBeenCalled();
    expect(budget).toHaveValue(2);
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

  it("leaves Atlas running: Pause all pauses every other Vision and reads only theirs", async () => {
    list = [vision("admin", "Atlas"), vision("rook", "Rook"), vision("wren", "Wren", true)];
    await open({});
    await act(async () => { fireEvent.click(screen.getByRole("checkbox", { name: "Pause all Visions" })); });
    expect(vi.mocked(visions.pauseVision).mock.calls).toEqual([["rook", true]]);
    cleanup();
    list = [vision("admin", "Atlas"), vision("rook", "Rook", true), vision("wren", "Wren", true)];
    await open({});
    const pauseAll = screen.getByRole("checkbox", { name: "Pause all Visions" });
    expect(pauseAll).toBeChecked();
    vi.mocked(visions.pauseVision).mockClear();
    await act(async () => { fireEvent.click(pauseAll); });
    expect(vi.mocked(visions.pauseVision).mock.calls).toEqual([["rook", false], ["wren", false]]);
  });
});
