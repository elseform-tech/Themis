// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { initialState } from "../state/reducer";
import type { VisionInfo, VisionsSettings } from "../lib/visions";
import * as visions from "../lib/visions";
import { VisionsSettingsSection } from "./VisionsSettings";

const vision = (id: string, name: string, paused = false): VisionInfo => ({
  id, name, role: "", color: "teal", soul: "", paused, revision: "r",
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

const base: VisionsSettings = { enabled: true, level: "auto_review", daily_budget_usd: 2, max_wakes: 8,
  requests_per_day: 10, routines_per_chief: 10, quiet_hours: null };
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
/** Picks a level on Settings' switch. */
async function pick(level: "Auto review" | "Full access") {
  const group = screen.getByRole("radiogroup", { name: "Level for new channels" });
  await act(async () => { fireEvent.click(within(group).getByRole("radio", { name: level })); });
}
/** Which level the switch shows. */
function shownLevel(): string | null {
  const group = screen.getByRole("radiogroup", { name: "Level for new channels" });
  return within(group).getAllByRole("radio").find(radio => radio.getAttribute("aria-checked") === "true")?.textContent ?? null;
}

beforeEach(() => {
  list = [vision("rook", "Rook"), vision("wren", "Wren", true)];
  vi.mocked(visions.updateVisionsSettings).mockClear();
  vi.mocked(visions.pauseVision).mockClear();
});
afterEach(cleanup);

describe("Settings › Visions", () => {
  it("team_limits_and_quiet_hours_save_field_by_field", async () => {
    await open({});
    const requests = screen.getByRole("spinbutton", { name: "Requests between channels, a day" });
    await act(async () => { fireEvent.change(requests, { target: { value: "4" } }); fireEvent.blur(requests); });
    expect(visions.updateVisionsSettings).toHaveBeenLastCalledWith({ requests_per_day: 4 }, "r1");
    const routines = screen.getByRole("spinbutton", { name: "Routines per chief" });
    await act(async () => { fireEvent.change(routines, { target: { value: "99" } }); fireEvent.blur(routines); });
    expect(screen.getByRole("alert")).toHaveTextContent("Enter a number from 1 to 50");
    expect(visions.updateVisionsSettings).toHaveBeenCalledTimes(1);
    expect(screen.queryByLabelText("Quiet from")).toBeNull();
    await act(async () => { fireEvent.click(screen.getByRole("checkbox", { name: "Quiet hours" })); });
    expect(visions.updateVisionsSettings).toHaveBeenLastCalledWith({ quiet_hours: { start: "22:00", end: "07:00" } }, "r2");
    await act(async () => { fireEvent.change(screen.getByLabelText("Quiet until"), { target: { value: "22:00" } }); });
    expect(screen.getByRole("alert")).toHaveTextContent("two different times");
    await act(async () => { fireEvent.change(screen.getByLabelText("Quiet until"), { target: { value: "06:30" } }); });
    expect(visions.updateVisionsSettings).toHaveBeenLastCalledWith({ quiet_hours: { start: "22:00", end: "06:30" } }, "r3");
  });

  it("level_switch_has_two_options_and_saves_level", async () => {
    await open({ level: "auto_review" });
    const group = screen.getByRole("radiogroup", { name: "Level for new channels" });
    expect(within(group).getAllByRole("radio").map(radio => radio.textContent)).toEqual(["Auto review", "Full access"]);
    expect(shownLevel()).toBe("Auto review");
    // Choosing the level it already has writes nothing.
    await pick("Auto review");
    expect(visions.updateVisionsSettings).not.toHaveBeenCalled();
    await pick("Full access");
    expect(visions.updateVisionsSettings).toHaveBeenCalledWith({ level: "full_access" }, "r1");
    expect(shownLevel()).toBe("Full access");
    // Full access on is drawn as the risky one.
    expect(within(group).getByRole("radio", { name: "Full access" })).toHaveClass("is-danger");
  });

  it("locked_rows_always_ask_you_at_either_level", async () => {
    for (const level of ["auto_review", "full_access"] as const) {
      await open({ level });
      const rows = [...document.querySelectorAll(".themis-visions-decision")].map(row => row.textContent);
      expect(rows).toEqual([
        "Anything that leaves this computerAlways asks you",
        "Deleting or replacing workAlways asks you",
        "Spending past the budgetAlways asks you",
      ]);
      // Nothing to edit there: the only choice is the level.
      expect(screen.getAllByRole("radiogroup")).toHaveLength(1);
      cleanup();
    }
  });

  it("shows no autonomy ladder and no hint text", async () => {
    await open({});
    expect(screen.queryByRole("slider")).toBeNull();
    expect(screen.queryByText(/Autonomy|Routine decisions|Custom/)).toBeNull();
    expect(screen.queryByText(/shown for reference|Approvals stay yours|What a Vision decides/)).toBeNull();
  });

  it("says in one line when Visions can't run on this computer, and nothing when they can", async () => {
    await open({});
    expect(screen.queryByRole("status")).toBeNull();
    cleanup();
    await open({}, false);
    expect(screen.getByRole("status").textContent).toBe("Visions can't run on this computer: it has no sandbox that can confine them.");
    // A write keeps it shown.
    await pick("Full access");
    expect(screen.getByRole("status")).toBeTruthy();
  });

  it("each_field_saves_alone_with_the_page_revision", async () => {
    await open({ level: "auto_review" });
    await pick("Full access");
    expect(visions.updateVisionsSettings).toHaveBeenCalledWith({ level: "full_access" }, "r1");
    const budget = screen.getByRole("spinbutton", { name: "Daily budget, all Visions" });
    fireEvent.change(budget, { target: { value: "5" } });
    await act(async () => { fireEvent.blur(budget); });
    // The next write names the revision the last one returned.
    expect(visions.updateVisionsSettings).toHaveBeenLastCalledWith({ daily_budget_usd: 5 }, "r2");
    expect(visions.updateVisionsSettings).toHaveBeenCalledTimes(2);
  });

  it("runs quick writes one at a time, each on the latest revision and level", async () => {
    await open({ level: "auto_review" });
    const group = screen.getByRole("radiogroup", { name: "Level for new channels" });
    await act(async () => {
      fireEvent.click(within(group).getByRole("radio", { name: "Full access" }));
      fireEvent.click(within(group).getByRole("radio", { name: "Auto review" }));
    });
    expect(vi.mocked(visions.updateVisionsSettings).mock.calls).toEqual([
      [{ level: "full_access" }, "r1"],
      [{ level: "auto_review" }, "r2"],
    ]);
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("shows a refused write and reads the file again", async () => {
    await open({ level: "auto_review" });
    // Someone edits runtime.jsonc after the page loaded.
    file = { settings: { ...file.settings, level: "full_access" }, revision: 2 };
    await pick("Full access");
    expect(screen.getByRole("alert")).toHaveTextContent("changed since this page loaded");
    expect(shownLevel()).toBe("Full access");
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
