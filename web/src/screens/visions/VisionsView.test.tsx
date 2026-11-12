// @vitest-environment jsdom
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { useReducer } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { DEFAULT_SETTINGS, initialState, reducer, type AppAction, type AppState } from "../../state/reducer";
import type { ApprovalRequest, ThreadInfo } from "../../lib/types";
import type { ChannelEntry, ChannelInfo, OwnedChat, VisionInfo } from "../../lib/visions";
import * as tauri from "../../lib/tauri";
import * as visionsBridge from "../../lib/visions";
import { SidebarWithVisions } from "./SidebarWithVisions";
import { VisionsView } from "./VisionsView";

const live = vi.hoisted(() => ({ value: null as unknown }));
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
const channel: ChannelInfo = { id: "c1", name: "release", topic: "Ship 0.9", project_root: "/p", approval_mode: "custom", members: ["rook", "wren"], revision: "r" };
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
      list: [vision("rook", "Rook"), vision("wren", "Wren", { needs: 1, color: "violet" })],
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
  const [state, dispatch] = useReducer(reducer, initial);
  live.value = { state, dispatch: dispatch as (action: AppAction) => void, loadOlderLines: vi.fn(async () => undefined) };
  return <><SidebarWithVisions collapsed={false} /><main><div data-testid="main-view">{state.mainView}:{state.activeThreadId}</div><VisionsView /></main></>;
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
    // The channel tag counts what waits on you there: the approval and the passed-on question.
    expect(within(bar).getByRole("button", { name: "# release, 2 need you" })).toBeTruthy();
    const wren = within(bar).getByText("Wren").closest("li")!;
    expect(wren).toHaveTextContent("Needs you");
    expect(within(bar).getByText("Rook").closest("li")).toHaveTextContent("Working");
    expect(within(bar).getByRole("button", { name: "New channel" })).toBeTruthy();
    expect(within(bar).getByRole("button", { name: "New Vision" })).toBeTruthy();
    // # hq opens first: phase 1 has no agent there, only the two ways to start.
    const hq = screen.getByRole("region", { name: "# hq" });
    expect(within(hq).getByRole("button", { name: "New channel" })).toBeTruthy();
    expect(within(hq).getByRole("button", { name: "New Vision" })).toBeTruthy();
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
