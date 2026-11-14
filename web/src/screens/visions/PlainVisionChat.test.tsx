// @vitest-environment jsdom
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { useReducer } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { appReducer, DEFAULT_SETTINGS, initialState, type AppAction, type AppState } from "../../state/reducer";
import type { ThreadInfo } from "../../lib/types";
import type { ChannelInfo, OwnedChat, VisionInfo } from "../../lib/visions";
import * as visionsBridge from "../../lib/visions";
import { SidebarWithVisions } from "./SidebarWithVisions";
import { VisionsView } from "./VisionsView";
import { PlainVisionBar } from "./HandBack";

const live = vi.hoisted(() => ({ value: null as unknown }));
vi.mock("../../state/store", async importOriginal => ({
  ...await importOriginal<typeof import("../../state/store")>(),
  useApp: () => live.value,
}));
vi.mock("../ThreadView", () => ({ ThreadView: () => <div data-testid="thread-view" /> }));
vi.mock("../../lib/tauri", async importOriginal => ({
  ...await importOriginal<typeof import("../../lib/tauri")>(),
  pluginAction: vi.fn(async () => []), stopThread: vi.fn(async () => undefined),
}));
vi.mock("../../lib/visions", async importOriginal => ({
  ...await importOriginal<typeof import("../../lib/visions")>(),
  listQuestions: vi.fn(async () => []), handBackThread: vi.fn(),
}));

const base = { provider: "go" as const, model: "m", running: false, worktree_path: null, branch: null, base_branch: null, recovered: false, skill_ids: [] };
const rookMark = { id: "rook", name: "Rook", shape: "flat", color: "amber" };
const plain: ThreadInfo = { ...base, id: "plain", title: "Plain chat", last_activity_seq: 1, vision: null };
const work: ThreadInfo = { ...base, id: "w1", title: "Fix the date test", last_activity_seq: 2, vision: rookMark };
const rook: VisionInfo = { ...rookMark, role: "", soul: "", paused: false, revision: "r", over_budget: false, vision_chats: [], needs: 0, sorry: false };
const channel: ChannelInfo = { id: "c1", name: "release", topic: "", project_root: "/p", approval_mode: "custom", members: ["rook"], revision: "r" };
const owned = (hold: OwnedChat["owner"]["hold"]): OwnedChat => ({ ...work, owner: { thread_id: "w1", vision_id: "rook", channel_id: "c1", kind: "work", hold } });

function fixture(extra: Partial<AppState> = {}): AppState {
  return {
    ...initialState,
    settings: DEFAULT_SETTINGS,
    settingsLoaded: true,
    mainView: "thread",
    projects: [{ root: "/p", name: "Ledger", is_git: true }],
    activeProjectRoot: "/p",
    activeThreadId: "plain",
    threadsByProject: { "/p": [plain, work] },
    visions: { ...initialState.visions, list: [rook], channels: { "/p": [channel] }, lines: { c1: [] }, nextBefore: { c1: null }, chats: { c1: [owned("none")] } },
    ...extra,
  };
}

function Harness({ initial }: { initial: AppState }) {
  const [state, dispatch] = useReducer(appReducer, initial);
  live.value = { state, dispatch: dispatch as (action: AppAction) => void, loadOlderLines: vi.fn() };
  return <><SidebarWithVisions collapsed={false} /><main>{state.mainView === "thread" && <PlainVisionBar />}<VisionsView /></main></>;
}

const dispatchLive = (action: AppAction) => act(() => { (live.value as { dispatch: (action: AppAction) => void }).dispatch(action); });

beforeEach(() => {
  vi.clearAllMocks();
  window.matchMedia = vi.fn().mockReturnValue({ matches: false, addEventListener: vi.fn(), removeEventListener: vi.fn() });
  HTMLElement.prototype.scrollIntoView = vi.fn();
});

/** The cloud inside `element`: its colour and whether it moves. */
function cloudOf(element: Element) {
  const cloud = element.querySelector("svg.themis-cloud");
  expect(cloud).not.toBeNull();
  return { amber: cloud!.classList.contains("themis-cloud--amber"), live: cloud!.classList.contains("is-live") };
}

describe("Vision chats in plain chat", () => {
  it("an_owned_chat_shows_its_vision_cloud_in_the_sidebar", () => {
    render(<Harness initial={fixture()} />);
    const sidebar = screen.getByRole("complementary", { name: "Workspace navigation" });
    // Assistive tech hears who runs it in the row's name; the cloud itself is decoration.
    const row = within(sidebar).getByRole("button", { name: "Fix the date test, run by Rook" });
    const mark = row.querySelector(".themis-vision-mark");
    expect(mark).toHaveAttribute("aria-hidden", "true");
    expect(cloudOf(mark!)).toEqual({ amber: true, live: false });
    // The person's own chat has no mark.
    expect(within(sidebar).getByRole("button", { name: "Plain chat" }).querySelector("svg.themis-cloud")).toBeNull();
    // A renamed Vision is shown by its name now.
    dispatchLive({ type: "visions/loaded", visions: [{ ...rook, name: "Rooky" }] });
    expect(within(sidebar).getByRole("button", { name: "Fix the date test, run by Rooky" })).toBe(row);
  });

  it("hand_back_uses_the_cloud_icon", async () => {
    vi.mocked(visionsBridge.handBackThread).mockResolvedValue(owned("none").owner);
    // In plain chat: the person typed in Rook's chat, so it is theirs until handed back.
    render(<Harness initial={fixture({ activeThreadId: "w1", visions: { ...fixture().visions, chats: { c1: [owned("taken_over")] } } })} />);
    expect(screen.getByRole("status")).toHaveTextContent("You took over.");
    const plainHandBack = screen.getByRole("button", { name: "Hand back to Rook" });
    expect(cloudOf(plainHandBack)).toEqual({ amber: true, live: false });
    fireEvent.click(plainHandBack);
    await waitFor(() => expect(visionsBridge.handBackThread).toHaveBeenCalledWith("w1"));
    dispatchLive({ type: "visions/changed", channelId: "c1", page: { entries: [], next_before: null }, chats: [owned("none")], questions: {} });
    expect(screen.queryByRole("button", { name: "Hand back to Rook" })).toBeNull();

    // In the Vision's chat opened from its channel: the same cloud button.
    dispatchLive({ type: "visions/changed", channelId: "c1", page: { entries: [], next_before: null }, chats: [owned("stopped_by_person")], questions: {} });
    dispatchLive({ type: "ui/view", view: "visions" });
    dispatchLive({ type: "visions/open", channelId: "c1", chatId: "w1" });
    const view = screen.getByRole("region", { name: "Rook's chat" });
    const handBack = within(view).getByRole("button", { name: "Hand back to Rook" });
    expect(cloudOf(handBack)).toEqual({ amber: true, live: false });
  });

  it("a Vision's chat chosen in plain chat stays chosen through Visions and back", async () => {
    render(<Harness initial={fixture()} />);
    fireEvent.click(within(screen.getByRole("complementary", { name: "Workspace navigation" })).getByRole("button", { name: /^Fix the date test/ }));
    dispatchLive({ type: "ui/view", view: "visions" });
    dispatchLive({ type: "ui/view", view: "thread" });
    const state = (live.value as { state: AppState }).state;
    expect(state.activeThreadId).toBe("w1");
    const { selectionToSave } = await import("../../state/store");
    expect(selectionToSave(state)).toEqual({ root: "/p", threadId: "w1" });
  });

  it("a Vision's chat opened from its channel, then chosen and typed in from plain chat, stays open through Visions and back", () => {
    render(<Harness initial={fixture({ mainView: "visions" })} />);
    dispatchLive({ type: "visions/open", channelId: "c1", chatId: "w1" });
    dispatchLive({ type: "ui/view", view: "thread" });
    expect((live.value as { state: AppState }).state.activeThreadId).toBe("plain");
    fireEvent.click(within(screen.getByRole("complementary", { name: "Workspace navigation" })).getByRole("button", { name: /^Fix the date test/ }));
    // Typing takes it over (the server's change, read back through the channel).
    dispatchLive({ type: "visions/changed", channelId: "c1", page: { entries: [], next_before: null }, chats: [owned("taken_over")], questions: {} });
    dispatchLive({ type: "ui/view", view: "visions" });
    dispatchLive({ type: "ui/view", view: "thread" });
    expect((live.value as { state: AppState }).state.activeThreadId).toBe("w1");
  });

  it("an update for a chat plain chat does not list (a Vision's own chat) adds nothing", () => {
    render(<Harness initial={fixture()} />);
    const own: ThreadInfo = { ...base, id: "own", title: "Rook · #release", vision: null };
    dispatchLive({ type: "thread/changed", change: { kind: "updated", project_root: "/p", thread_id: "own", thread: own } });
    expect(screen.queryByRole("button", { name: /Rook · #release/ })).toBeNull();
  });

  it("a Vision's chat whose profile is not listed hands back with the chat's own cloud", () => {
    render(<Harness initial={fixture({ mainView: "visions", visions: { ...fixture().visions, list: [], chats: { c1: [owned("taken_over")] } } })} />);
    dispatchLive({ type: "visions/open", channelId: "c1", chatId: "w1" });
    const handBack = screen.getByRole("button", { name: "Hand back to Rook" });
    expect(cloudOf(handBack)).toEqual({ amber: true, live: false });
  });

  it("the plain bar shows nothing for the person's own chat", () => {
    render(<Harness initial={fixture()} />);
    expect(screen.queryByRole("status")).toBeNull();
    expect(screen.queryByRole("button", { name: /Hand back/ })).toBeNull();
  });
});
