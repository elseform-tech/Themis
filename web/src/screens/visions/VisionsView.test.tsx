// @vitest-environment jsdom
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { useReducer } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { appReducer, DEFAULT_SETTINGS, initialState, type AppAction, type AppState } from "../../state/reducer";
import type { ApprovalRequest, ThreadInfo } from "../../lib/types";
import type { ChannelEntry, ChannelInfo, OwnedChat, VisionInfo } from "../../lib/visions";
import * as tauri from "../../lib/tauri";
import * as visionsBridge from "../../lib/visions";
import { SidebarWithVisions } from "./SidebarWithVisions";
import { VisionsView } from "./VisionsView";

const live = vi.hoisted(() => ({ value: null as unknown }));
const loadOlder = vi.hoisted(() => vi.fn(async (_channelId: string) => false));
vi.mock("../../state/store", async importOriginal => ({
  ...await importOriginal<typeof import("../../state/store")>(),
  useApp: () => live.value,
}));
vi.mock("../ThreadView", () => ({ ThreadView: () => <div data-testid="thread-view" /> }));
vi.mock("../../lib/tauri", async importOriginal => ({
  ...await importOriginal<typeof import("../../lib/tauri")>(),
  pluginAction: vi.fn(async () => []), stopThread: vi.fn(async () => undefined),
  approveAction: vi.fn(async () => undefined), answerQuestion: vi.fn(async () => ({})),
}));
vi.mock("../../lib/visions", async importOriginal => ({
  ...await importOriginal<typeof import("../../lib/visions")>(),
  saveChannel: vi.fn(), saveVision: vi.fn(),
  listQuestions: vi.fn(async () => []), postChannelMessage: vi.fn(async () => ({ seq: 9 })), handBackThread: vi.fn(),
}));

const base = { provider: "go" as const, model: "m", running: false, worktree_path: null, branch: null, base_branch: null, recovered: false, skill_ids: [] };
const plain: ThreadInfo = { ...base, id: "plain", title: "Plain chat", last_activity_seq: 1 };
const vision = (id: string, name: string, extra: Partial<VisionInfo> = {}): VisionInfo => ({
  id, name, role: "", shape: "flat", color: "teal", soul: "", paused: false, revision: "r",
  over_budget: false, vision_chats: [], needs: 0, sorry: false, ...extra,
});
const owned = (id: string, title: string, visionId: string, running: boolean): OwnedChat => ({
  ...base, id, title, running, owner: { thread_id: id, vision_id: visionId, channel_id: "c1", kind: "work", hold: "none" },
});
// The approval on w1 and the passed-on question on w2 wait on the person: two in # release, one
// for each Vision, as the server counts them.
const channel: ChannelInfo = { id: "c1", name: "release", topic: "Ship 0.9", project_root: "/p", approval_mode: "custom", members: ["rook", "wren"], revision: "r", needs: 2 };
const entries: ChannelEntry[] = [
  { type: "person", text: "@Rook fix the date test" },
  { type: "vision", vision_id: "rook", text: "On it." },
  { type: "chat_started", vision_id: "rook", thread_id: "w1", title: "Fix the date test" },
  { type: "passed_on", vision_id: "wren", thread_id: "w2", question_id: "q1", decision: "wording", question: "Which heading?" },
];
const approval: ApprovalRequest = { thread_id: "w1", approval_id: "a1", tool: "shell", summary: "git push origin v0.9.0", risk: "network", command: { argv: ["git", "push", "origin", "v0.9.0"] } };

function fixture(extra: Partial<AppState> = {}): AppState {
  return {
    ...initialState,
    settings: DEFAULT_SETTINGS,
    settingsLoaded: true,
    projects: [{ root: "/p", name: "Ledger", is_git: true }],
    activeProjectRoot: "/p",
    activeThreadId: "plain",
    threadsByProject: { "/p": [plain] },
    approvals: [approval],
    visions: {
      ...initialState.visions,
      list: [vision("rook", "Rook", { needs: 1 }), vision("wren", "Wren", { needs: 1, color: "violet" })],
      channels: { "/p": [channel] },
      lines: { c1: entries.map((entry, seq) => ({ seq, at_ms: Date.UTC(2026, 10, 12, 9, seq), entry })) },
      nextBefore: { c1: null },
      chats: { c1: [owned("w1", "Fix the date test", "rook", true), owned("w2", "Draft the notes", "wren", false)] },
      questions: { w2: ["q1"] },
    },
    ...extra,
  };
}

function Harness({ initial }: { initial: AppState }) {
  const [state, dispatch] = useReducer(appReducer, initial);
  live.value = { state, dispatch: dispatch as (action: AppAction) => void, loadOlderLines: loadOlder };
  return <><SidebarWithVisions collapsed={false} /><main><div data-testid="main-view">{state.mainView}:{state.activeThreadId}</div><VisionsView /></main></>;
}

const dispatchLive = (action: AppAction) => act(() => { (live.value as { dispatch: (action: AppAction) => void }).dispatch(action); });
const stateLive = () => (live.value as { state: AppState }).state;
const hold = (chat: OwnedChat, value: OwnedChat["owner"]["hold"], running = chat.running): OwnedChat => ({ ...chat, running, owner: { ...chat.owner, hold: value } });

/** Opens # release and then its chat w1 (Rook's, running). */
async function openChat(extra: Partial<AppState> = {}) {
  mount({ mainView: "visions", ...extra });
  fireEvent.click(within(screen.getByRole("complementary", { name: "Visions navigation" })).getByRole("button", { name: /release/ }));
  fireEvent.click(within(screen.getByRole("region", { name: "# release" })).getByRole("button", { name: /^Open Fix the date test/ }));
  return screen.getByRole("region", { name: "Rook's chat" });
}

function mount(extra: Partial<AppState> = {}) {
  render(<Harness initial={fixture(extra)} />);
}

async function openRelease() {
  mount({ mainView: "visions" });
  fireEvent.click(within(screen.getByRole("complementary", { name: "Visions navigation" })).getByRole("button", { name: /release/ }));
  return screen.getByRole("region", { name: "# release" });
}

beforeEach(() => {
  vi.clearAllMocks();
  window.matchMedia = vi.fn().mockReturnValue({ matches: false, addEventListener: vi.fn(), removeEventListener: vi.fn() });
  HTMLElement.prototype.scrollIntoView = vi.fn();
});

describe("Visions view", () => {
  it("channels_bar_replaces_the_chat_list", () => {
    mount();
    expect(screen.getByRole("complementary", { name: "Workspace navigation" })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: /^Visions/ }));
    expect(screen.queryByRole("complementary", { name: "Workspace navigation" })).toBeNull();
    const bar = screen.getByRole("complementary", { name: "Visions navigation" });
    expect(within(bar).getByLabelText("Project: Ledger")).toBeTruthy();
    expect(within(bar).getByRole("button", { name: "# hq" })).toBeTruthy();
    // The channel tag is the server's count of what waits in that channel's own chats.
    expect(within(bar).getByRole("button", { name: "# release, 2 need you" })).toBeTruthy();
    const wren = within(bar).getByText("Wren").closest("li")!;
    expect(wren).toHaveTextContent("Needs you");
    expect(within(bar).getByText("Rook").closest("li")).toHaveTextContent("Needs you");
    expect(within(bar).getByRole("button", { name: "New channel" })).toBeTruthy();
    expect(within(bar).getByRole("button", { name: "New Vision" })).toBeTruthy();
    // # hq is a channel like any other; with no hq channel yet it is empty and cannot be written to.
    const hq = screen.getByRole("region", { name: "# hq" });
    expect(hq).toHaveTextContent("No messages yet.");
    expect(within(hq).getByLabelText("Message #hq")).toBeDisabled();
    expect(within(hq).queryByRole("button", { name: /^New / })).toBeNull();
  });

  it("the_admin_lives_in_hq_with_its_proposals", () => {
    const hq: ChannelInfo = { id: "h1", name: "hq", topic: "", project_root: "/p", approval_mode: "custom", members: ["admin"], revision: "r" };
    const proposal: ChannelEntry = { type: "proposal", vision_id: "admin", proposal_id: "p1", proposal: { summary: "A channel for releases.", channels: [{ name: "releases", topic: "", members: [] }], visions: [], changes: [] } };
    const start = fixture();
    mount({
      mainView: "visions",
      visions: {
        ...start.visions,
        list: [...start.visions.list, vision("admin", "Atlas", { color: "indigo" })],
        channels: { "/p": [hq, channel] },
        lines: { ...start.visions.lines, h1: [{ seq: 1, at_ms: Date.UTC(2026, 10, 12, 9), entry: proposal }] },
      },
    });
    const bar = screen.getByRole("complementary", { name: "Visions navigation" });
    expect(within(bar).queryByText("Atlas")).toBeNull();
    const room = screen.getByRole("region", { name: "# hq" });
    expect(within(room).getByRole("img", { name: "Here: Atlas" }).querySelector(".themis-cloud-crown")).toBeTruthy();
    expect(within(room).getByText("Atlas").closest("li")!.querySelector(".themis-cloud-crown")).toBeTruthy();
    expect(within(room).getByRole("group", { name: "Proposal" })).toHaveTextContent("A channel for releases.");
    expect(within(room).getByLabelText("Message #hq")).not.toBeDisabled();
  });

  it("chats_returns_to_plain_chat_from_visions", async () => {
    const view = await openRelease();
    fireEvent.click(within(view).getByRole("button", { name: /^Open Fix the date test/ }));
    expect(screen.getByTestId("main-view")).toHaveTextContent("visions:w1");
    expect(screen.getByTestId("thread-view")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Chats" }));
    expect(screen.getByTestId("main-view")).toHaveTextContent("thread:plain");
    expect(screen.getByRole("complementary", { name: "Workspace navigation" })).toBeTruthy();
    expect(screen.queryByRole("complementary", { name: "Visions navigation" })).toBeNull();
  });

  it("stop_on_a_chat_link_calls_stop_thread", async () => {
    const view = await openRelease();
    fireEvent.click(within(view).getByRole("button", { name: "Stop Rook's chat, Fix the date test" }));
    await waitFor(() => expect(tauri.stopThread).toHaveBeenCalledWith("w1"));
    // A chat that is not running has no Stop.
    expect(within(view).queryByRole("button", { name: /Stop Wren's chat/ })).toBeNull();
  });

  it("a_passed_question_is_answered_in_the_channel", async () => {
    vi.mocked(visionsBridge.listQuestions).mockResolvedValue([{ id: "q1", run_id: "r", question: "Which heading?", choices: ["Changed", "Fixed"], assumption: "Changed", blocking: false, asked_at: 1, passed_on: true }]);
    const view = await openRelease();
    const card = within(view).getByRole("group", { name: "Wren's chat Draft the notes asks you" });
    expect(card).toHaveTextContent("Which heading?");
    fireEvent.click(await within(card).findByRole("button", { name: "Changed (recommended)" }));
    await waitFor(() => expect(tauri.answerQuestion).toHaveBeenCalledWith("w2", "q1", "Changed"));
    await waitFor(() => expect(card).toHaveTextContent("You answered: Changed"));
    expect(within(card).queryByRole("button", { name: "Fixed" })).toBeNull();
  });

  it("a passed question can be answered in the person's own words", async () => {
    const view = await openRelease();
    const card = within(view).getByRole("group", { name: "Wren's chat Draft the notes asks you" });
    fireEvent.change(within(card).getByLabelText("Answer in your own words"), { target: { value: "Under Fixed" } });
    fireEvent.submit(within(card).getByLabelText("Answer in your own words"));
    await waitFor(() => expect(tauri.answerQuestion).toHaveBeenCalledWith("w2", "q1", "Under Fixed"));
  });

  it("a question someone already answered shows as answered", async () => {
    render(<Harness initial={fixture({ mainView: "visions", visions: { ...fixture().visions, questions: { w2: [] }, open: { channelId: "c1", chatId: null, plainThreadId: null } } })} />);
    const card = screen.getByRole("group", { name: "Wren's chat Draft the notes asks you" });
    expect(card).toHaveTextContent("Answered");
    expect(within(card).queryByLabelText("Answer in your own words")).toBeNull();
  });

  it("an_approval_from_an_owned_chat_shows_allow_and_deny", async () => {
    const view = await openRelease();
    const box = within(view).getByRole("region", { name: "Approval needed" });
    expect(within(box).getByRole("button", { name: "Allow once" })).toBeTruthy();
    fireEvent.click(within(box).getByRole("button", { name: "Deny" }));
    await waitFor(() => expect(tauri.approveAction).toHaveBeenCalledWith("w1", "a1", "deny"));
    await waitFor(() => expect(within(view).queryByRole("region", { name: "Approval needed" })).toBeNull());
  });

  it("an approval from a chat no Vision owns stays out of the channel", async () => {
    const view = await openRelease();
    expect(within(view).getAllByRole("region", { name: "Approval needed" })).toHaveLength(1);
    act(() => { (live.value as { dispatch: (action: AppAction) => void }).dispatch({ type: "approval/enqueued", request: { ...approval, thread_id: "plain", approval_id: "a2" } }); });
    expect(within(view).getAllByRole("region", { name: "Approval needed" })).toHaveLength(1);
  });

  it("message_avatars_do_not_animate", async () => {
    const view = await openRelease();
    const avatars = view.querySelectorAll(".themis-channel-entry .themis-cloud");
    expect(avatars.length).toBeGreaterThan(0);
    for (const avatar of avatars) expect(avatar).not.toHaveClass("is-live");
    // The same Vision's cloud in the bar is live.
    const bar = screen.getByRole("complementary", { name: "Visions navigation" });
    expect(within(bar).getByText("Rook").closest("li")!.querySelector(".themis-cloud")).toHaveClass("is-live");
  });

  it("a_running_vision_shows_it_is_thinking", () => {
    const thinking = (lines: ChannelEntry[]) => fixture({ mainView: "visions", visions: {
      ...fixture().visions,
      list: [vision("rook", "Rook", { vision_chats: [{ thread_id: "own", channel_id: "c1", running: true }] }), vision("wren", "Wren")],
      lines: { c1: lines.map((entry, seq) => ({ seq, at_ms: 0, entry })) },
      open: { channelId: "c1", chatId: null, plainThreadId: null },
    } });
    const { unmount } = render(<Harness initial={thinking([{ type: "person", text: "@Rook check the build" }])} />);
    const row = screen.getByRole("status");
    expect(row).toHaveTextContent("Rook is thinking…");
    expect(row.querySelector(".themis-cloud")).toHaveClass("themis-cloud--think");
    expect(row.querySelector(".themis-shim")).toBeTruthy();
    unmount();
    // Once it has posted, the row goes.
    render(<Harness initial={thinking([{ type: "person", text: "@Rook check the build" }, { type: "vision", vision_id: "rook", text: "It is green." }])} />);
    expect(screen.queryByText("Rook is thinking…")).toBeNull();
  });

  it("a Vision's new post fades in, lines already there do not", () => {
    mount({ mainView: "visions", visions: { ...fixture().visions, open: { channelId: "c1", chatId: null, plainThreadId: null } } });
    const view = screen.getByRole("region", { name: "# release" });
    expect(view.querySelector(".is-fresh")).toBeNull();
    const page = { entries: [...fixture().visions.lines.c1!, { seq: 9, at_ms: 0, entry: { type: "vision" as const, vision_id: "rook", text: "Done." } }], next_before: null };
    act(() => { (live.value as { dispatch: (action: AppAction) => void }).dispatch({ type: "visions/changed", channelId: "c1", page, chats: fixture().visions.chats.c1!, questions: { w2: ["q1"] } }); });
    expect(within(view).getByText("Done.").closest("li")).toHaveClass("is-fresh");
  });

  it("lines read after the channel opened empty do not fade in, later ones do", () => {
    mount({ mainView: "visions", visions: { ...fixture().visions, lines: {}, open: { channelId: "c1", chatId: null, plainThreadId: null } } });
    const page = (extra: number) => ({ entries: [...fixture().visions.lines.c1!, ...Array.from({ length: extra }, (_, i) => ({ seq: 10 + i, at_ms: 0, entry: { type: "vision" as const, vision_id: "rook", text: `Line ${i}` } }))], next_before: null });
    dispatchLive({ type: "visions/changed", channelId: "c1", page: page(0), chats: fixture().visions.chats.c1!, questions: { w2: ["q1"] } });
    const view = screen.getByRole("region", { name: "# release" });
    expect(view.querySelector(".is-fresh")).toBeNull();
    dispatchLive({ type: "visions/changed", channelId: "c1", page: page(1), chats: fixture().visions.chats.c1!, questions: { w2: ["q1"] } });
    expect(within(view).getByText("Line 0").closest("li")).toHaveClass("is-fresh");
  });

  it("new channel saves its name, topic and members, then opens it", async () => {
    vi.mocked(visionsBridge.saveChannel).mockImplementation(async draft => ({ ...draft, id: "c2", topic: draft.topic ?? "", revision: "r" }));
    mount({ mainView: "visions" });
    fireEvent.click(screen.getByRole("button", { name: "New channel" }));
    const sheet = screen.getByRole("dialog", { name: "New channel" });
    fireEvent.change(within(sheet).getByLabelText("Channel name"), { target: { value: "Watch CI" } });
    fireEvent.change(within(sheet).getByLabelText("Topic"), { target: { value: "Keep main green" } });
    fireEvent.click(within(sheet).getByRole("button", { name: "Wren" }));
    fireEvent.click(within(sheet).getByRole("button", { name: "Create" }));
    await waitFor(() => expect(visionsBridge.saveChannel).toHaveBeenCalledWith({ name: "watch-ci", topic: "Keep main green", project_root: "/p", approval_mode: "custom", members: ["wren"] }, null));
    expect(await screen.findByRole("region", { name: "# watch-ci" })).toBeTruthy();
  });

  it("new vision saves its name, cloud and role", async () => {
    vi.mocked(visionsBridge.saveVision).mockImplementation(async draft => ({ ...draft, id: "v3", role: draft.role ?? "", soul: "", paused: false, revision: "r" }));
    mount({ mainView: "visions" });
    fireEvent.click(screen.getByRole("button", { name: "New Vision" }));
    const sheet = screen.getByRole("dialog", { name: "New Vision" });
    fireEvent.change(within(sheet).getByLabelText("Name"), { target: { value: "Echo" } });
    fireEvent.click(within(sheet).getByRole("button", { name: "Wide cloud" }));
    fireEvent.click(within(sheet).getByRole("button", { name: "Rose" }));
    fireEvent.change(within(sheet).getByLabelText("Role"), { target: { value: "Scribe" } });
    fireEvent.click(within(sheet).getByRole("button", { name: "Create" }));
    await waitFor(() => expect(visionsBridge.saveVision).toHaveBeenCalledWith({ name: "Echo", role: "Scribe", shape: "wide", color: "rose" }, null));
    const bar = screen.getByRole("complementary", { name: "Visions navigation" });
    expect(await within(bar).findByText("Echo")).toBeTruthy();
  });

  it("an open chat stays open through visions reloads and a hand back", async () => {
    vi.mocked(visionsBridge.handBackThread).mockResolvedValue({ ...owned("w1", "", "rook", false).owner });
    const chats = fixture().visions.chats.c1!;
    await openChat({ visions: { ...fixture().visions, chats: { c1: [hold(chats[0]!, "taken_over"), chats[1]!] } } });
    expect(screen.getByRole("status")).toHaveTextContent("You took over.");
    fireEvent.click(screen.getByRole("button", { name: "Hand back to Rook" }));
    await waitFor(() => expect(visionsBridge.handBackThread).toHaveBeenCalledWith("w1"));
    // The server's reload after the hand back: the channel list and the channel read again.
    dispatchLive({ type: "visions/loaded", projectRoot: "/p", channels: [channel] });
    dispatchLive({ type: "visions/loaded", visions: stateLive().visions.list });
    dispatchLive({ type: "visions/changed", channelId: "c1", page: { entries: stateLive().visions.lines.c1!, next_before: null }, chats: [hold(chats[0]!, "none"), chats[1]!], questions: {} });
    expect(screen.getByRole("region", { name: "Rook's chat" })).toBeTruthy();
    expect(screen.getByRole("status")).toHaveTextContent("Rook is running this chat.");
    expect(screen.getByTestId("main-view")).toHaveTextContent("visions:w1");
    expect(screen.getByTestId("thread-view")).toBeTruthy();
  });

  it("a Vision's chat shows who holds it and stops from its bar", async () => {
    const view = await openChat();
    expect(within(view).getByRole("status")).toHaveTextContent("Rook is running this chat.");
    expect(within(view).queryByRole("button", { name: /Hand back/ })).toBeNull();
    fireEvent.click(within(view).getByRole("button", { name: "Stop this chat" }));
    await waitFor(() => expect(tauri.stopThread).toHaveBeenCalledWith("w1"));
    const chats = stateLive().visions.chats.c1!;
    dispatchLive({ type: "visions/changed", channelId: "c1", page: { entries: stateLive().visions.lines.c1!, next_before: null }, chats: [hold(chats[0]!, "stopped_by_person", false), chats[1]!], questions: {} });
    expect(within(view).getByRole("status")).toHaveTextContent("You stopped this chat.");
    expect(within(view).queryByRole("button", { name: "Stop this chat" })).toBeNull();
    expect(within(view).getByRole("button", { name: "Hand back to Rook" })).toBeTruthy();
    // Idle and with the Vision: no note at all.
    dispatchLive({ type: "visions/changed", channelId: "c1", page: { entries: stateLive().visions.lines.c1!, next_before: null }, chats: [hold(chats[0]!, "none", false), chats[1]!], questions: {} });
    expect(within(view).queryByRole("status")).toBeNull();
  });

  it("a deleted chat says so in place instead of leaving", async () => {
    await openChat();
    dispatchLive({ type: "visions/changed", channelId: "c1", page: { entries: stateLive().visions.lines.c1!, next_before: null }, chats: [], questions: {} });
    expect(screen.getByText("This chat was deleted.")).toBeTruthy();
    expect(screen.queryByTestId("thread-view")).toBeNull();
    expect(stateLive().visions.open.chatId).toBe("w1");
    fireEvent.click(screen.getByRole("button", { name: "Back to # release" }));
    expect(screen.getByRole("region", { name: "# release" })).toBeTruthy();
  });

  it("a deleted channel says so in place", async () => {
    await openRelease();
    dispatchLive({ type: "visions/loaded", projectRoot: "/p", channels: [] });
    expect(screen.getByText("This channel was deleted.")).toBeTruthy();
    expect(stateLive().visions.open.channelId).toBe("c1");
  });

  it("show earlier messages reads the page before the window", async () => {
    mount({ mainView: "visions", visions: { ...fixture().visions, nextBefore: { c1: 40 }, open: { channelId: "c1", chatId: null, plainThreadId: null } } });
    fireEvent.click(screen.getByRole("button", { name: "Show earlier messages" }));
    await waitFor(() => expect(loadOlder).toHaveBeenCalledWith("c1"));
  });

  it("switching projects in Visions stays in Visions on the new project's # hq", () => {
    mount({ mainView: "visions", projects: [{ root: "/p", name: "Ledger", is_git: true }, { root: "/q", name: "Other", is_git: true }], threadsByProject: { "/p": [plain], "/q": [] }, visions: { ...fixture().visions, open: { channelId: "c1", chatId: null, plainThreadId: null } } });
    const bar = screen.getByRole("complementary", { name: "Visions navigation" });
    // The same switcher as the chat list's: its project actions are here too.
    expect(within(bar).getByRole("button", { name: "New project" })).toBeTruthy();
    expect(within(bar).getByRole("button", { name: "Rename Other" })).toBeTruthy();
    fireEvent.click(within(bar).getByRole("button", { name: "Other" }));
    expect(stateLive().mainView).toBe("visions");
    expect(stateLive().activeProjectRoot).toBe("/q");
    expect(screen.getByRole("region", { name: "# hq" })).toBeTruthy();
  });

  it("an open Vision's chat is never saved as the plain chat to restore", async () => {
    await openChat();
    expect(stateLive().activeThreadId).toBe("w1");
    const { selectionToSave } = await import("../../state/store");
    expect(selectionToSave(stateLive())).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Chats" }));
    expect(selectionToSave(stateLive())).toEqual({ root: "/p", threadId: "plain" });
  });

  it("the mention list follows the arrow keys", async () => {
    const view = await openRelease();
    const box = within(view).getByLabelText("Message #release");
    fireEvent.change(box, { target: { value: "@" } });
    const list = within(view).getByRole("listbox", { name: "Mention" });
    expect(box).toHaveAttribute("aria-controls", list.id);
    const [rook, wren] = within(list).getAllByRole("option");
    expect(box).toHaveAttribute("aria-activedescendant", rook!.id);
    fireEvent.keyDown(box, { key: "ArrowDown" });
    expect(box).toHaveAttribute("aria-activedescendant", wren!.id);
    expect(wren).toHaveAttribute("aria-selected", "true");
    fireEvent.keyDown(box, { key: "Enter" });
    expect(box).toHaveValue("@Wren ");
  });

  it("the mention list closes on Escape, starts over on a new prefix, and is a combobox", async () => {
    const view = await openRelease();
    const box = within(view).getByRole("combobox", { name: "Message #release" });
    expect(box).toHaveAttribute("aria-expanded", "false");
    fireEvent.change(box, { target: { value: "@" } });
    expect(box).toHaveAttribute("aria-expanded", "true");
    fireEvent.keyDown(box, { key: "ArrowDown" });
    fireEvent.change(box, { target: { value: "@W" } });
    const [wren] = within(view).getAllByRole("option");
    expect(box).toHaveAttribute("aria-activedescendant", wren!.id);
    fireEvent.change(box, { target: { value: "@" } });
    expect(within(view).getAllByRole("option")[0]).toHaveAttribute("aria-selected", "true");
    fireEvent.keyDown(box, { key: "Escape" });
    expect(within(view).queryByRole("listbox")).toBeNull();
    expect(box).toHaveAttribute("aria-expanded", "false");
    // Typing on brings it back.
    fireEvent.change(box, { target: { value: "@R" } });
    expect(within(view).getByRole("listbox")).toBeTruthy();
  });

  it("re-showing the open chat never changes where Chats returns to", async () => {
    const other: ThreadInfo = { ...plain, id: "other", title: "Other chat" };
    await openChat({ threadsByProject: { "/p": [other, plain] } });
    expect(stateLive().visions.open.plainThreadId).toBe("plain");
    // Something else moves the active chat (here the plain list picks a neighbour).
    dispatchLive({ type: "thread/removed", projectRoot: "/p", threadId: "w1" });
    await waitFor(() => expect(stateLive().activeThreadId).toBe("w1"));
    expect(stateLive().visions.open).toMatchObject({ channelId: "c1", chatId: "w1", plainThreadId: "plain" });
  });

  it("the first Vision post in a channel opened empty fades in", () => {
    mount({ mainView: "visions", visions: { ...fixture().visions, lines: { c1: [] }, open: { channelId: "c1", chatId: null, plainThreadId: null } } });
    const page = { entries: [{ seq: 0, at_ms: 0, entry: { type: "vision" as const, vision_id: "rook", text: "Hello." } }], next_before: null };
    dispatchLive({ type: "visions/changed", channelId: "c1", page, chats: fixture().visions.chats.c1!, questions: {} });
    expect(within(screen.getByRole("region", { name: "# release" })).getByText("Hello.").closest("li")).toHaveClass("is-fresh");
  });

  it("lines loaded above keep the person's place in the log", async () => {
    mount({ mainView: "visions", visions: { ...fixture().visions, nextBefore: { c1: 0 }, open: { channelId: "c1", chatId: null, plainThreadId: null } } });
    const log = screen.getByRole("region", { name: "# release" }).querySelector(".themis-channel-log") as HTMLElement;
    let height = 1000;
    let top = 600;
    Object.defineProperty(log, "scrollHeight", { configurable: true, get: () => height });
    Object.defineProperty(log, "clientHeight", { configurable: true, get: () => 400 });
    Object.defineProperty(log, "scrollTop", { configurable: true, get: () => top, set: (value: number) => { top = value; } });
    // The person reads back to the top of the window.
    top = 0;
    fireEvent.scroll(log);
    loadOlder.mockImplementationOnce(async () => {
      // The page arrives later, outside the click, as from the server.
      await new Promise(resolve => setTimeout(resolve, 0));
      height = 1600;
      const entries = [-3, -2, -1].map(seq => ({ seq, at_ms: 0, entry: { type: "person" as const, text: `older ${seq}` } }));
      (live.value as { dispatch: (action: AppAction) => void }).dispatch({ type: "visions/older", channelId: "c1", page: { entries, next_before: null } });
      return true;
    });
    fireEvent.click(screen.getByRole("button", { name: "Show earlier messages" }));
    await screen.findByText("older -3");
    // The 600px added above are scrolled past: what was on screen stays where it was.
    await waitFor(() => expect(top).toBe(600));
  });

  it("a load that adds nothing lets the log stick to the end again", async () => {
    mount({ mainView: "visions", visions: { ...fixture().visions, nextBefore: { c1: 0 }, open: { channelId: "c1", chatId: null, plainThreadId: null } } });
    const log = screen.getByRole("region", { name: "# release" }).querySelector(".themis-channel-log") as HTMLElement;
    let height = 1000;
    let top = 600;
    Object.defineProperty(log, "scrollHeight", { configurable: true, get: () => height });
    Object.defineProperty(log, "clientHeight", { configurable: true, get: () => 400 });
    Object.defineProperty(log, "scrollTop", { configurable: true, get: () => top, set: (value: number) => { top = value; } });
    loadOlder.mockImplementationOnce(async () => false);
    fireEvent.click(screen.getByRole("button", { name: "Show earlier messages" }));
    await waitFor(() => expect(loadOlder).toHaveBeenCalled());
    await act(async () => { await Promise.resolve(); });
    // A new post at the bottom while the person is at the end: the log follows it.
    height = 1100;
    const page = { entries: [...fixture().visions.lines.c1!, { seq: 9, at_ms: 0, entry: { type: "vision" as const, vision_id: "rook", text: "New." } }], next_before: 0 };
    dispatchLive({ type: "visions/changed", channelId: "c1", page, chats: fixture().visions.chats.c1!, questions: { w2: ["q1"] } });
    expect(top).toBe(1100);
  });

  it("each channel keeps its own fade baseline", () => {
    const c2: ChannelInfo = { ...channel, id: "c2", name: "watch", needs: 0 };
    mount({ mainView: "visions", visions: { ...fixture().visions, channels: { "/p": [channel, c2] }, lines: { c1: [], c2: fixture().visions.lines.c1! }, chats: { ...fixture().visions.chats, c2: [] }, open: { channelId: "c1", chatId: null, plainThreadId: null } } });
    fireEvent.click(within(screen.getByRole("complementary", { name: "Visions navigation" })).getByRole("button", { name: /watch/ }));
    const view = screen.getByRole("region", { name: "# watch" });
    expect(view.querySelectorAll(".themis-channel-entry").length).toBeGreaterThan(0);
    expect(view.querySelector(".is-fresh")).toBeNull();
  });

  it("asks to mention a Vision when several are here, and posts the message", async () => {
    const view = await openRelease();
    const box = within(view).getByLabelText("Message #release");
    expect(within(view).getByRole("button", { name: "Mention a Vision" })).toBeTruthy();
    fireEvent.change(box, { target: { value: "@Wr" } });
    expect(within(view).queryByRole("button", { name: "Mention a Vision" })).toBeNull();
    fireEvent.click(within(view).getByRole("option", { name: "Wren" }));
    expect(box).toHaveValue("@Wren ");
    fireEvent.change(box, { target: { value: "@Wren draft the notes" } });
    fireEvent.keyDown(box, { key: "Enter" });
    await waitFor(() => expect(visionsBridge.postChannelMessage).toHaveBeenCalledWith("c1", "@Wren draft the notes"));
    await waitFor(() => expect(box).toHaveValue(""));
  });
});
