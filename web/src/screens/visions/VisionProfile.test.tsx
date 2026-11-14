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

const dispatch = vi.fn();
const live = vi.hoisted(() => ({ value: null as unknown }));
vi.mock("../../state/store", async importOriginal => ({
  ...await importOriginal<typeof import("../../state/store")>(),
  useApp: () => live.value,
}));
vi.mock("../../lib/tauri", async importOriginal => ({
  ...await importOriginal<typeof import("../../lib/tauri")>(),
  listGoModels: vi.fn(async () => [{ id: "glm-5", effort_levels: [] }, { id: "kimi-k3", effort_levels: [] }]),
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
const release: ChannelInfo = { id: "c1", name: "release", topic: "Ship 0.9", project_root: "/p", approval_mode: "custom", members: ["rook"], chief: "rook", revision: "c1r" };
const hq: ChannelInfo = { id: "h1", name: "administration", topic: "", project_root: "/p", approval_mode: "custom", members: ["admin"], chief: "admin", revision: "h1r" };

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

describe("A Vision's profile", () => {
  it("saves the model and the soul, each against the profile's revision, and offers no autonomy or budget of its own", async () => {
    await show(() => <VisionProfile visionId="rook" />);
    expect(screen.getByRole("combobox", { name: "Model" })).toHaveDisplayValue("Default model");
    await act(async () => { fireEvent.change(screen.getByRole("combobox", { name: "Model" }), { target: { value: "glm-5" } }); });
    expect(visions.saveVision).toHaveBeenLastCalledWith(expect.objectContaining({ model: "glm-5" }), "r1");
    // Autonomy and budget are the team's (its channel's), not a Vision's.
    expect(screen.queryByRole("combobox", { name: "Autonomy" })).toBeNull();
    expect(screen.queryByRole("spinbutton", { name: "Daily budget" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Edit soul" }));
    fireEvent.change(screen.getByRole("textbox", { name: "Soul" }), { target: { value: "Bold." } });
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Save" })); });
    expect(visions.saveVision).toHaveBeenLastCalledWith(expect.objectContaining({ soul: "Bold." }), "r2");
  });

  it("pauses the Vision and lists its channels", async () => {
    await show(() => <VisionProfile visionId="rook" />);
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Pause Rook" })); });
    expect(visions.pauseVision).toHaveBeenCalledWith("rook", true);
    const channels = screen.getByRole("group", { name: "Channels" });
    fireEvent.click(within(channels).getByRole("button", { name: "# release" }));
    expect(dispatch).toHaveBeenCalledWith({ type: "visions/open", channelId: "c1" });
    expect(screen.getByRole("button", { name: "Delete Rook" })).toBeInTheDocument();
  });

  it("shows an empty soul as its heading and edit button only", async () => {
    const initial = state();
    initial.visions.list = initial.visions.list.map(each => each.id === "rook" ? { ...each, soul: "" } : each);
    await show(() => <VisionProfile visionId="rook" />, initial);
    expect(screen.getByRole("button", { name: "Edit soul" })).toBeInTheDocument();
    expect(document.querySelector(".themis-visions-soul")).toBeNull();
    expect(screen.queryByText(/in your words/)).toBeNull();
  });

  it("gives a worker no soul: it has a role", async () => {
    await show(() => <VisionProfile visionId="wren" />);
    expect(screen.queryByText("Soul")).toBeNull();
    expect(screen.queryByRole("button", { name: "Edit soul" })).toBeNull();
  });

  it("keeps the Vision's accessory when it saves", async () => {
    const initial = state();
    initial.visions.list = initial.visions.list.map(each => each.id === "rook" ? { ...each, accessory: "glasses" } : each);
    await show(() => <VisionProfile visionId="rook" />, initial);
    await act(async () => { fireEvent.change(screen.getByRole("combobox", { name: "Model" }), { target: { value: "glm-5" } }); });
    expect(visions.saveVision).toHaveBeenLastCalledWith(expect.objectContaining({ accessory: "glasses" }), "r1");
  });

  it("shows Atlas only what applies to it: its soul, no Delete, autonomy or channels", async () => {
    await show(() => <VisionProfile visionId="admin" />);
    expect(screen.queryByRole("button", { name: "Delete Atlas" })).toBeNull();
    expect(screen.queryByRole("combobox", { name: "Autonomy" })).toBeNull();
    expect(screen.getByText("Soul")).toBeInTheDocument();
    expect(screen.queryByRole("group", { name: "Channels" })).toBeNull();
    expect(screen.getByRole("combobox", { name: "Model" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Pause Atlas" })).toBeInTheDocument();
  });
});

describe("Channel details", () => {
  it("saves the topic, the approval mode and the members, and offers only Visions", async () => {
    await show(() => <OpenChannelDetails />, opened("c1"));
    const topic = screen.getByRole("textbox", { name: "Topic" });
    fireEvent.change(topic, { target: { value: "Ship 1.0" } });
    await act(async () => { fireEvent.blur(topic); });
    expect(visions.saveChannel).toHaveBeenLastCalledWith(expect.objectContaining({ id: "c1", topic: "Ship 1.0" }), "c1r");
    await act(async () => { fireEvent.change(screen.getByRole("combobox", { name: "Approval mode" }), { target: { value: "auto" } }); });
    expect(visions.saveChannel).toHaveBeenLastCalledWith(expect.objectContaining({ approval_mode: "auto" }), "c2");
    const add = screen.getByRole("combobox", { name: "Add a member" });
    expect(within(add).getAllByRole("option").map(option => option.textContent)).toEqual(["Add a member", "Wren"]);
    await act(async () => { fireEvent.change(add, { target: { value: "wren" } }); });
    expect(visions.saveChannel).toHaveBeenLastCalledWith(expect.objectContaining({ members: ["rook", "wren"], chief: "rook" }), "c2");
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Remove Rook" })); });
    expect(visions.saveChannel).toHaveBeenLastCalledWith(expect.objectContaining({ members: ["wren"], chief: "wren" }), "c2");
    expect(screen.getByRole("button", { name: "Delete # release" })).toBeInTheDocument();
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
