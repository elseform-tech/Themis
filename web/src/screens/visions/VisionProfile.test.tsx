// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { useReducer, type ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { appReducer, initialState, type AppAction, type AppState } from "../../state/reducer";
import { useApp } from "../../state/store";
import type { ChannelInfo, VisionInfo } from "../../lib/visions";
import * as visions from "../../lib/visions";
import { ChannelDetails } from "./ChannelDetails";
import { projectChannels } from "./channels";
import { VisionProfile } from "./VisionProfile";
import { VisionsView } from "./VisionsView";

const dispatch = vi.fn();
const live = vi.hoisted(() => ({ value: null as unknown }));
vi.mock("../../state/store", async importOriginal => ({
  ...await importOriginal<typeof import("../../state/store")>(),
  useApp: () => live.value,
}));
vi.mock("../../lib/tauri", async importOriginal => ({
  ...await importOriginal<typeof import("../../lib/tauri")>(),
  listGoModels: vi.fn(async () => [{ id: "glm-5", effort_levels: [] }, { id: "kimi-k3", effort_levels: [] }]),
  listModelProviders: vi.fn(async () => []),
}));
vi.mock("../../lib/visions", async importOriginal => ({
  ...await importOriginal<typeof import("../../lib/visions")>(),
  getVisionsSettings: vi.fn(async () => ({ settings: { enabled: true, autonomy: "most", custom: {}, daily_budget_usd: 2, max_wakes: 8 }, revision: "s1", can_run: true })),
  saveVision: vi.fn(async (draft: object) => ({ ...draft, revision: "r2" })),
  pauseVision: vi.fn(async () => ({})),
  deleteVision: vi.fn(async () => undefined),
  saveChannel: vi.fn(async (draft: object) => ({ ...draft, revision: "c2" })),
  deleteChannel: vi.fn(async () => undefined),
}));

const vision = (id: string, name: string, extra: Partial<VisionInfo> = {}): VisionInfo => ({
  id, name, role: "Builder", shape: "flat", color: "teal", soul: "Careful and brief.", paused: false, revision: "r1",
  over_budget: false, vision_chats: [], needs: 0, sorry: false, ...extra,
});
const release: ChannelInfo = { id: "c1", name: "release", topic: "Ship 0.9", project_root: "/p", full_access: false, members: ["rook"], chief: "rook", revision: "c1r" };
const hq: ChannelInfo = { id: "h1", name: "administration", topic: "", project_root: "/p", full_access: false, members: ["admin"], chief: "admin", revision: "h1r" };

function state(): AppState {
  return {
    ...initialState,
    activeProjectRoot: "/p",
    visions: {
      ...initialState.visions,
      list: [vision("admin", "Atlas", { role: "Sets up channels and Visions", soul: "" }), vision("rook", "Rook"), vision("wren", "Wren", { role: "Reviewer" })],
      channels: { "/p": [hq, release] },
    },
  };
}
/** The app's own reducer behind the mocked store; `dispatch` sees every action. */
function Harness({ initial, view }: { initial: AppState; view: () => ReactNode }) {
  const [current, apply] = useReducer(appReducer, initial);
  live.value = { state: current, dispatch: (action: AppAction) => { dispatch(action); apply(action); } };
  // Built on each render, so the view reads the new state.
  return <>{view()}</>;
}
/** Channel details for the open channel, as the Visions view gives them. */
function OpenChannelDetails() {
  const current = projectChannels(useApp().state).current;
  return current ? <ChannelDetails channel={current} /> : null;
}
async function show(view: () => ReactNode, initial = state()) {
  await act(async () => { render(<Harness initial={initial} view={view} />); });
}
const opened = (channelId: string | null): AppState => ({ ...state(), visions: { ...state().visions, open: { ...initialState.visions.open, channelId } } });

beforeEach(() => {
  dispatch.mockClear();
  vi.mocked(visions.saveVision).mockClear();
  vi.mocked(visions.saveChannel).mockClear();
  vi.mocked(visions.pauseVision).mockClear();
});
afterEach(cleanup);

describe("A chief's profile", () => {
  it("saves the model and the soul against the profile's revision, and the autonomy on the channel", async () => {
    await show(() => <VisionProfile visionId="rook" />, opened("c1"));
    expect(screen.getByRole("combobox", { name: "Model" })).toHaveDisplayValue("Chat model");
    await act(async () => { fireEvent.change(screen.getByRole("combobox", { name: "Model" }), { target: { value: "glm-5" } }); });
    expect(visions.saveVision).toHaveBeenLastCalledWith(expect.objectContaining({ model: "glm-5" }), "r1");
    // Autonomy and budget are the team's (its channel's): Settings › Visions' until the channel sets its own.
    const autonomy = await screen.findByRole("combobox", { name: "Autonomy" });
    expect(autonomy).toHaveDisplayValue("Most decisions");
    expect(screen.getByText("$2 a day")).toBeInTheDocument();
    await act(async () => { fireEvent.change(autonomy, { target: { value: "small" } }); });
    expect(visions.saveChannel).toHaveBeenLastCalledWith(expect.objectContaining({ id: "c1", autonomy: "small" }), "c1r");
    const soul = screen.getByRole("textbox", { name: "Soul" });
    fireEvent.change(soul, { target: { value: "Bold." } });
    await act(async () => { fireEvent.blur(soul); });
    expect(visions.saveVision).toHaveBeenLastCalledWith(expect.objectContaining({ soul: "Bold." }), "r2");
  });

  it("an unchanged soul is not saved again", async () => {
    await show(() => <VisionProfile visionId="rook" />, opened("c1"));
    const soul = screen.getByRole("textbox", { name: "Soul" });
    fireEvent.focus(soul);
    await act(async () => { fireEvent.blur(soul); });
    expect(visions.saveVision).not.toHaveBeenCalled();
  });

  it("pauses the chief and goes back to its channel; it has no Delete", async () => {
    await show(() => <VisionProfile visionId="rook" />, opened("c1"));
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Pause Rook" })); });
    expect(visions.pauseVision).toHaveBeenCalledWith("rook", true);
    expect(screen.queryByRole("button", { name: "Delete Rook" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "# release" }));
    expect(dispatch).toHaveBeenCalledWith({ type: "visions/panel", panel: null });
  });

  it("keeps the Vision's accessory when it saves", async () => {
    const initial = opened("c1");
    initial.visions.list = initial.visions.list.map(each => each.id === "rook" ? { ...each, accessory: "glasses" } : each);
    await show(() => <VisionProfile visionId="rook" />, initial);
    await act(async () => { fireEvent.change(screen.getByRole("combobox", { name: "Model" }), { target: { value: "glm-5" } }); });
    expect(visions.saveVision).toHaveBeenLastCalledWith(expect.objectContaining({ accessory: "glasses" }), "r1");
  });

  it("shows Atlas only what applies to it: its soul, its model and Pause", async () => {
    await show(() => <VisionProfile visionId="admin" />, opened(null));
    expect(screen.getByText("Admin, # administration")).toBeInTheDocument();
    expect(screen.queryByRole("combobox", { name: "Autonomy" })).toBeNull();
    expect(screen.queryByRole("list", { name: "Team" })).toBeNull();
    expect(screen.getByRole("textbox", { name: "Soul" })).toBeInTheDocument();
    expect(screen.getByRole("combobox", { name: "Model" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Pause Atlas" })).toBeInTheDocument();
  });

  it("gives a worker no profile: it opens its thread with the chief", async () => {
    const initial = opened("c1");
    initial.visions.channels = { "/p": [hq, { ...release, members: ["rook", "wren"] }] };
    initial.mainView = "visions";
    initial.visions.open.panel = { kind: "vision", visionId: "wren" };
    await show(() => <VisionsView />, initial);
    expect(screen.queryByRole("region", { name: "Wren, profile" })).toBeNull();
    expect(screen.getByRole("complementary", { name: "Wren with Rook" })).toBeInTheDocument();
  });
});

describe("Channel details", () => {
  it("saves the topic and the members, and offers only Visions", async () => {
    await show(() => <OpenChannelDetails />, opened("c1"));
    const topic = screen.getByRole("textbox", { name: "Topic" });
    fireEvent.change(topic, { target: { value: "Ship 1.0" } });
    await act(async () => { fireEvent.blur(topic); });
    expect(visions.saveChannel).toHaveBeenLastCalledWith(expect.objectContaining({ id: "c1", topic: "Ship 1.0" }), "c1r");
    expect(screen.queryByRole("combobox", { name: "Approval mode" })).toBeNull();
    const add = screen.getByRole("combobox", { name: "Add a member" });
    expect(within(add).getAllByRole("option").map(option => option.textContent)).toEqual(["Add a member", "Wren"]);
    await act(async () => { fireEvent.change(add, { target: { value: "wren" } }); });
    expect(visions.saveChannel).toHaveBeenLastCalledWith(expect.objectContaining({ members: ["rook", "wren"], chief: "rook" }), "c2");
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Remove Rook" })); });
    expect(visions.saveChannel).toHaveBeenLastCalledWith(expect.objectContaining({ members: ["wren"], chief: "wren" }), "c2");
    expect(screen.getByRole("button", { name: "Delete # release" })).toBeInTheDocument();
  });

  it("saves the channel's daily budget; empty goes back to Settings' budget", async () => {
    await show(() => <OpenChannelDetails />, opened("c1"));
    const budget = screen.getByRole("spinbutton", { name: "Daily budget" });
    fireEvent.change(budget, { target: { value: "1.5" } });
    await act(async () => { fireEvent.blur(budget); });
    expect(visions.saveChannel).toHaveBeenLastCalledWith(expect.objectContaining({ id: "c1", daily_budget_usd: 1.5 }), "c1r");
    fireEvent.change(budget, { target: { value: "" } });
    await act(async () => { fireEvent.blur(budget); });
    expect(visions.saveChannel).toHaveBeenLastCalledWith(expect.objectContaining({ daily_budget_usd: null }), "c2");
    fireEvent.change(budget, { target: { value: "-3" } });
    await act(async () => { fireEvent.blur(budget); });
    expect(screen.getByRole("alert")).toHaveTextContent("Enter an amount");
  });

  it("deletes a member Vision after asking, and refreshes the list; Atlas cannot be deleted", async () => {
    const initial = opened("c1");
    initial.visions.channels = { "/p": [hq, { ...release, members: ["rook", "wren"] }] };
    await show(() => <OpenChannelDetails />, initial);
    fireEvent.click(screen.getByRole("button", { name: "Delete Wren" }));
    const dialog = screen.getByRole("dialog", { name: "Delete Wren?" });
    expect(dialog).toHaveTextContent("Its chats stop and become ordinary chats.");
    expect(visions.deleteVision).not.toHaveBeenCalled();
    await act(async () => { fireEvent.click(within(dialog).getByRole("button", { name: "Delete" })); });
    expect(visions.deleteVision).toHaveBeenCalledWith("wren");
    expect(screen.queryByRole("button", { name: "Wren" })).toBeNull();
    cleanup();
    await show(() => <OpenChannelDetails />, opened(null));
    expect(screen.queryByRole("button", { name: "Delete Atlas" })).toBeNull();
  });

  it("keeps # administration built in: no Delete, and Atlas its only member", async () => {
    await show(() => <OpenChannelDetails />, opened(null));
    expect(screen.queryByRole("button", { name: "Delete # administration" })).toBeNull();
    expect(screen.queryByRole("combobox", { name: "Add a member" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Remove Atlas" })).toBeNull();
    expect(screen.getByRole("button", { name: "Atlas" })).toBeInTheDocument();
    // Atlas starts no chats in # administration, so its approval mode would do nothing.
    expect(screen.queryByRole("combobox", { name: "Approval mode" })).toBeNull();
    expect(screen.getByRole("textbox", { name: "Topic" })).not.toHaveAttribute("placeholder");
  });
});
