// @vitest-environment jsdom
// The team screens against boards TeamChannel, AdminChannel and ChiefProfile.
import { COLORS } from "./cast";
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { useReducer, type ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { appReducer, DEFAULT_SETTINGS, initialState, type AppAction, type AppState } from "../../state/reducer";
import type { ThreadInfo } from "../../lib/types";
import type { ChannelEntry, ChannelInfo, OwnedChat, VisionInfo } from "../../lib/visions";
import * as visions from "../../lib/visions";
import * as tauri from "../../lib/tauri";
import { ProposalCard } from "./ProposalCard";
import { Routines } from "./Routines";
import { SidebarWithVisions } from "./SidebarWithVisions";
import { TeamThread } from "./TeamThread";
import type { Routine, TeamLine } from "./team";
import { VisionProfile } from "./VisionProfile";
import { VisionsView } from "./VisionsView";

const live = vi.hoisted(() => ({ value: null as unknown }));
vi.mock("../../state/store", async importOriginal => ({
  ...await importOriginal<typeof import("../../state/store")>(),
  useApp: () => live.value,
}));
vi.mock("../ThreadView", () => ({ ThreadView: () => <div data-testid="thread-view" /> }));
const changeListeners: Array<(change: { channel_id?: string }) => void> = [];
vi.mock("../../lib/tauri", async importOriginal => ({
  ...await importOriginal<typeof import("../../lib/tauri")>(),
  listGoModels: vi.fn(async () => [{ id: "longcat-2.5", effort_levels: [] }]),
  listModelProviders: vi.fn(async () => []),
  stopThread: vi.fn(async () => undefined),
  approveAction: vi.fn(async () => undefined),
  getEffectivePermissions: vi.fn(async () => ({})),
  onVisionsChanged: vi.fn(async (cb: (change: { channel_id?: string }) => void) => { changeListeners.push(cb); return () => undefined; }),
}));
vi.mock("../../lib/visions", async importOriginal => ({
  ...await importOriginal<typeof import("../../lib/visions")>(),
  getVisionsSettings: vi.fn(async () => ({ settings: { enabled: true, autonomy: "most", custom: {}, daily_budget_usd: 5, max_wakes: 8 }, revision: "s1", can_run: true })),
  saveVision: vi.fn(async (draft: object) => ({ ...draft, revision: "r2" })),
  saveChannel: vi.fn(async (draft: object) => ({ ...draft, revision: "c2" })),
  pauseVision: vi.fn(async () => ({})),
  listQuestions: vi.fn(async () => []),
  postChannelMessage: vi.fn(async () => ({ seq: 9, woken: ["juno"] })),
  getTeamLog: vi.fn(async () => ({ entries: [{ seq: 1, at_ms: 0, entry: { kind: "task", from_chief: "juno", text: "Fix the parser from the server." } }], next_before: null })),
  getChannelSpend: vi.fn(async () => ({ spent_usd: 1.2 })),
  answerChannelRequest: vi.fn(async () => undefined),
  listRoutines: vi.fn(async () => []),
}));

const base = { provider: "go" as const, model: "m", running: false, worktree_path: null, branch: null, base_branch: null, recovered: false, skill_ids: [] };
const plain: ThreadInfo = { ...base, id: "plain", title: "Plain chat", last_activity_seq: 1 };
const vision = (id: string, name: string, extra: Partial<VisionInfo> = {}): VisionInfo => ({
  id, name, role: "", color: "blue", soul: "", paused: false, revision: "r1",
  over_budget: false, vision_chats: [], needs: 0, sorry: false, ...extra,
});
const owned = (id: string, title: string, visionId: string, running: boolean): OwnedChat => ({
  ...base, id, title, running, owner: { thread_id: id, vision_id: visionId, channel_id: "c1", kind: "work", hold: "none" },
});
const hq: ChannelInfo = { id: "h1", name: "administration", topic: "", project_root: "/p", full_access: false, members: ["admin"], chief: "admin", revision: "h" };
const release: ChannelInfo = { id: "c1", name: "release", topic: "", project_root: "/p", full_access: false, members: ["juno", "rook", "wren"], chief: "juno", autonomy: "most", daily_budget_usd: 3, revision: "c" };
const api: ChannelInfo = { id: "c2", name: "api", topic: "", project_root: "/p", full_access: false, members: ["sol"], chief: "sol", revision: "a" };
const releaseLines: ChannelEntry[] = [
  { type: "person", text: "Release notes for 0.9 are due Friday." },
  { type: "vision", vision_id: "juno", text: "Yes. Rook fixes the date rollover, Wren checks it.", tasked: ["rook", "wren"] },
  { type: "vision", vision_id: "rook", text: "Fallback is in." },
  { type: "chat_started", vision_id: "rook", thread_id: "w1", title: "Fix the date rollover" },
  // A question passed on to the person shows whoever's chat asks: a worker's chat passes its
  // permission questions straight to the person.
  { type: "passed_on", vision_id: "wren", thread_id: "w9", question_id: "q0", decision: "wording", question: "Which heading, Wren asks?" },
  { type: "passed_on", vision_id: "juno", thread_id: "w9", question_id: "q1", decision: "wording", question: "Which heading for the notes?" },
  // The person's own action on a worker stays; anything else a worker caused does not.
  { type: "notice", kind: "paused", vision_id: "rook" },
  { type: "notice", kind: "chat_failed", vision_id: "rook", thread_id: "w1" },
];
const cast = () => [
  vision("admin", "Atlas", { color: "slate", accessory: "crown", role: "Admin", soul: "Sets up teams." }),
  vision("juno", "Juno", { accessory: "clipboard", role: "Chief of staff", soul: "Calm and direct.", model: "longcat-2.5" }),
  vision("rook", "Rook", { color: "teal", accessory: "hard_hat", role: "Builder" }),
  vision("wren", "Wren", { color: "violet", accessory: "glasses", role: "Reviewer" }),
  vision("sol", "Sol", { color: "rose", accessory: "headphones", role: "Writer" }),
];

function fixture(extra: Partial<AppState["visions"]> = {}): AppState {
  return {
    ...initialState,
    settings: DEFAULT_SETTINGS,
    settingsLoaded: true,
    mainView: "visions",
    projects: [{ root: "/p", name: "Ledger", is_git: true }],
    activeProjectRoot: "/p",
    activeThreadId: "plain",
    threadsByProject: { "/p": [plain] },
    visions: {
      ...initialState.visions,
      list: cast(),
      channels: { "/p": [hq, release, api] },
      lines: { c1: releaseLines.map((entry, seq) => ({ seq, at_ms: Date.UTC(2026, 10, 14, 9, seq), entry })), h1: [] },
      nextBefore: { c1: null, h1: null },
      chats: { c1: [owned("w1", "Fix the date rollover", "rook", true)] },
      open: { ...initialState.visions.open, channelId: "c1" },
      ...extra,
    },
  };
}

function Harness({ initial, view }: { initial: AppState; view: () => ReactNode }) {
  const [state, dispatch] = useReducer(appReducer, initial);
  live.value = { state, dispatch: dispatch as (action: AppAction) => void, loadOlderLines: vi.fn(async () => false) };
  return <>{view()}</>;
}
const app = () => <><SidebarWithVisions collapsed={false} /><main><VisionsView /></main></>;
async function show(view: () => ReactNode = app, initial = fixture()) {
  await act(async () => { render(<Harness initial={initial} view={view} />); });
}
const bar = () => screen.getByRole("complementary", { name: "Visions navigation" });

beforeEach(() => {
  vi.clearAllMocks();
  window.matchMedia = vi.fn().mockReturnValue({ matches: false, addEventListener: vi.fn(), removeEventListener: vi.fn() });
  HTMLElement.prototype.scrollIntoView = vi.fn();
});
afterEach(cleanup);

describe("Team screens", () => {
  it("the_sidebar_pins_administration_then_the_open_channels_team", async () => {
    await show();
    const rows = within(bar()).getAllByRole("button", { name: /^# / });
    expect(rows.map(row => row.getAttribute("aria-label"))).toEqual(["# administration", "# release", "# api"]);
    // Channel rows carry no clouds; the team of the open channel sits under Team.
    expect(rows.every(row => row.querySelector("svg") === null)).toBe(true);
    const heading = within(bar()).getByRole("heading", { name: /Channels/ });
    expect(heading.compareDocumentPosition(rows[0]!) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(within(bar()).queryByRole("heading", { name: /Visions/ })).toBeNull();
    const team = within(bar()).getByRole("list", { name: "Team" });
    expect(within(team).getAllByRole("button").map(row => row.getAttribute("aria-label"))).toEqual(["Juno, Chief of staff, Calm", "Rook, Builder, Working", "Wren, Reviewer, Idle"]);
  });

  it("the_channel_shows_only_the_person_and_the_chief", async () => {
    await show();
    const channel = screen.getByRole("region", { name: "# release" });
    // No header: the channel's name small at the top left, the chief floating at the top middle.
    expect(within(channel).queryByRole("banner")).toBeNull();
    expect(within(channel).queryByRole("group", { name: "Team" })).toBeNull();
    // Rook's work chat runs; Juno's own reply does not: her state is Calm.
    expect(within(channel).getByRole("button", { name: "Juno, chief of staff, Calm" })).toHaveTextContent("JunoCalm");
    expect(within(channel).queryByRole("img", { name: "Juno, Calm" })).toBeNull();
    // The log: the person and Juno; Rook's post and his chat stay in his thread.
    expect(channel).toHaveTextContent("Release notes for 0.9 are due Friday.");
    expect(channel).toHaveTextContent("Rook fixes the date rollover");
    expect(channel).not.toHaveTextContent("Fallback is in.");
    expect(within(channel).queryByRole("button", { name: /^Open Fix the date rollover/ })).toBeNull();
    expect(channel).toHaveTextContent("Which heading for the notes?");
    expect(channel).toHaveTextContent("Which heading, Wren asks?");
    expect(channel).toHaveTextContent("You paused Rook.");
    expect(channel).not.toHaveTextContent("A chat Rook started failed");
    // The box talks to Juno, with no mention button.
    expect(within(channel).getByRole("textbox", { name: "Message Juno" })).toHaveAttribute("placeholder", "Message Juno");
    expect(within(channel).queryByRole("button", { name: "Mention a Vision" })).toBeNull();
    fireEvent.change(within(channel).getByRole("textbox", { name: "Message Juno" }), { target: { value: "@" } });
    expect(screen.queryByRole("listbox")).toBeNull();
  });

  it("the_person_writes_on_the_right_and_the_chief_on_the_left", async () => {
    await show();
    const channel = screen.getByRole("region", { name: "# release" });
    const mine = within(channel).getByText("Release notes for 0.9 are due Friday.").closest(".themis-bubble")!;
    const chief = within(channel).getByText(/Rook fixes the date rollover/).closest(".themis-bubble")!;
    expect(mine).toHaveClass("themis-bubble-me");
    expect(chief).toHaveClass("themis-bubble-chief");
    // No avatar or name in either bubble's row; the person's keeps its time.
    for (const bubble of [mine, chief]) {
      const row = bubble.closest("li")!;
      expect(row.querySelector("svg.themis-cloud")).toBeNull();
      expect(row).not.toHaveTextContent(/^(You|Juno)/);
    }
    expect(mine.closest("li")!.querySelector("time")).toBeTruthy();
    // The person's bubbles take the chief's colour, so the two sides read apart in every sky.
    const juno = fixture().visions.list.find(vision => vision.name === "Juno")!;
    const log = channel.querySelector(".themis-channel-log") as HTMLElement;
    expect(log.style.getPropertyValue("--themis-chief-color")).toBe(COLORS[juno.color]!.mid);
  });

  it("a_handoff_shows_one_indicator_per_tasked_worker", async () => {
    const waiting = fixture();
    waiting.approvals = [{ thread_id: "w1", approval_id: "a1", tool: "write_file", summary: "Write src/date.rs", risk: "write" } as AppState["approvals"][number]];
    await show(app, waiting);
    const channel = screen.getByRole("region", { name: "# release" });
    const chief = within(channel).getByText(/Rook fixes the date rollover/).closest("li")!;
    const shown = () => within(chief).getAllByRole("button", { name: /'s thread/ });
    expect(shown().map(button => button.getAttribute("aria-label"))).toEqual(["Rook's thread, Needs you", "Wren's thread, Idle"]);
    expect(shown()[0]).toHaveTextContent("Rook's threadNeeds you");
    // No cloud on an indicator.
    expect(chief.querySelector("svg.themis-cloud")).toBeNull();
    // Answered, and his chat finished: Done.
    const live2 = live.value as { state: AppState; dispatch: (action: AppAction) => void };
    await act(async () => {
      live2.dispatch({ type: "approval/dequeued", approvalId: "a1" });
      live2.dispatch({ type: "visions/changed", channelId: "c1", page: { entries: live2.state.visions.lines.c1!, next_before: null }, chats: [{ ...owned("w1", "Fix the date rollover", "rook", false), last_outcome: "done" }], questions: {} });
    });
    expect(shown()[0]).toHaveAttribute("aria-label", "Rook's thread, Done");
    // An indicator opens that worker's thread.
    fireEvent.click(shown()[0]!);
    expect(screen.getByRole("complementary", { name: "Rook with Juno" })).toBeTruthy();
  });

  it("an open question of a worker's chat shows Needs you", async () => {
    const asking = fixture({ questions: { w1: ["q5"] } });
    await show(app, asking);
    expect(screen.getByRole("button", { name: "Rook's thread, Needs you" })).toBeTruthy();
  });

  it("the_channel_name_opens_the_details", async () => {
    await show();
    const name = within(screen.getByRole("region", { name: "# release" })).getByRole("button", { name: "# release: channel details" });
    expect(name).toHaveTextContent("# release");
    fireEvent.click(name);
    expect(name).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByRole("complementary", { name: "Channel details" })).toBeTruthy();
  });

  it("the_floating_chief_opens_its_profile", async () => {
    await show();
    const chief = within(screen.getByRole("region", { name: "# release" })).getByRole("button", { name: "Juno, chief of staff, Calm" });
    expect(chief.querySelector("svg.themis-cloud")).toHaveAttribute("width", "40");
    fireEvent.click(chief);
    expect(screen.getByRole("region", { name: "Juno, profile" })).toBeTruthy();
  });

  it("approvals_are_not_in_the_channel_log", async () => {
    const waiting = fixture();
    waiting.approvals = [{ thread_id: "w1", approval_id: "a1", tool: "write_file", summary: "Write src/date.rs", risk: "write" } as AppState["approvals"][number]];
    await show(app, waiting);
    expect(within(screen.getByRole("region", { name: "# release" })).queryByRole("region", { name: "Approval needed" })).toBeNull();
  });

  it("a_worker_thread_opens_read_only", async () => {
    await show();
    fireEvent.click(screen.getByRole("button", { name: /^Rook's thread/ }));
    const thread = screen.getByRole("complementary", { name: "Rook with Juno" });
    // His work chat is a link; there is no box to write in.
    expect(within(thread).getByRole("button", { name: /^Open Fix the date rollover/ })).toBeTruthy();
    expect(within(thread).queryByRole("textbox")).toBeNull();
    expect(within(thread).queryByRole("combobox")).toBeNull();
    fireEvent.click(within(thread).getByRole("button", { name: "Close" }));
    expect(screen.queryByRole("complementary", { name: "Rook with Juno" })).toBeNull();
  });

  it("a worker's thread shows as the server keeps it", async () => {
    await show();
    fireEvent.click(screen.getByRole("button", { name: /^Rook's thread/ }));
    const thread = screen.getByRole("complementary", { name: "Rook with Juno" });
    expect(await within(thread).findByText("Fix the parser from the server.")).toBeTruthy();
  });

  it("a worker's thread reads again when the server says the channel changed", async () => {
    await show();
    fireEvent.click(screen.getByRole("button", { name: /^Rook's thread/ }));
    await screen.findByText("Fix the parser from the server.");
    const before = vi.mocked(visions.getTeamLog).mock.calls.length;
    await act(async () => { for (const listener of changeListeners) listener({ channel_id: "c1" }); });
    expect(vi.mocked(visions.getTeamLog).mock.calls.length).toBeGreaterThan(before);
  });

  it("a worker's thread lists the exchange with its chief, oldest first", async () => {
    const log: TeamLine[] = [
      { seq: 1, at_ms: Date.UTC(2026, 10, 14, 9, 4), entry: { kind: "task", text: "Fix the date rollover in the parser." } },
      { seq: 2, at_ms: Date.UTC(2026, 10, 14, 9, 12), entry: { kind: "pushback", text: "Changing the parser breaks old exports." } },
      { seq: 3, at_ms: Date.UTC(2026, 10, 14, 9, 13), entry: { kind: "paused" } },
    ];
    await show(() => <TeamThread channel={release} workerId="rook" log={log} />);
    const items = within(screen.getByRole("complementary", { name: "Rook with Juno" })).getAllByRole("listitem");
    // The chief's lines on the right, the worker's on the left, each with "name · time" above.
    expect(items[0]).toHaveTextContent(/^Juno · \S+.*Fix the date rollover in the parser\.$/);
    expect(items[0]).toHaveClass("is-mine");
    expect(items[0]!.querySelector(".themis-bubble")).toBeTruthy();
    expect(items[1]).toHaveTextContent(/^Rook · \S+.*Changing the parser breaks old exports\.$/);
    expect(items[1]).toHaveClass("is-theirs");
    expect(items[2]).toHaveTextContent("Juno paused Rook");
  });

  it("the_thread_header_shows_both_clouds", async () => {
    await show(() => <TeamThread channel={release} workerId="rook" log={[]} />);
    const thread = screen.getByRole("complementary", { name: "Rook with Juno" });
    const head = thread.querySelector(".themis-visions-panel-head")!;
    expect(head).toHaveTextContent(/^Rook with Juno$/);
    const clouds = head.querySelectorAll("svg.themis-cloud");
    expect([...clouds].map(cloud => cloud.getAttribute("width"))).toEqual(["22", "16"]);
    for (const cloud of clouds) expect(cloud).toHaveAttribute("data-accessory", "none");
  });

  it("a_workers_approval_waits_in_its_thread", async () => {
    const waiting = fixture({ open: { ...initialState.visions.open, channelId: "c1", panel: { kind: "worker", visionId: "rook" } } });
    waiting.approvals = [{ thread_id: "w1", approval_id: "a1", tool: "write_file", summary: "Write src/date.rs", risk: "write" } as AppState["approvals"][number]];
    await show(app, waiting);
    const thread = screen.getByRole("complementary", { name: "Rook with Juno" });
    const box = within(thread).getByRole("region", { name: "Approval needed" });
    // Under the log, above the chat link at the foot.
    const link = within(thread).getByRole("button", { name: /^Open Fix the date rollover/ });
    expect(box.compareDocumentPosition(link) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    await act(async () => { fireEvent.click(within(box).getByRole("button", { name: "Allow once" })); });
    expect(tauri.approveAction).toHaveBeenCalledWith("w1", "a1", "once");
    expect(within(thread).queryByRole("region", { name: "Approval needed" })).toBeNull();
    // Another worker's approval is not in Rook's thread.
    cleanup();
    const wrens = fixture({ open: { ...initialState.visions.open, channelId: "c1", panel: { kind: "worker", visionId: "rook" } }, chats: { c1: [owned("w1", "Fix the date rollover", "rook", true), owned("w2", "Review", "wren", true)] } });
    wrens.approvals = [{ thread_id: "w2", approval_id: "a2", tool: "write_file", summary: "Write notes.md", risk: "write" } as AppState["approvals"][number]];
    await show(app, wrens);
    expect(within(screen.getByRole("complementary", { name: "Rook with Juno" })).queryByRole("region", { name: "Approval needed" })).toBeNull();
  });

  it("the_chief_profile_edits_soul_and_routines", async () => {
    await show();
    fireEvent.click(screen.getByRole("button", { name: /^Juno, chief of staff/ }));
    const profile = screen.getByRole("region", { name: "Juno, profile" });
    expect(profile).toHaveTextContent("Chief of staff, # release");
    expect(within(profile).getByRole("button", { name: "Pause Juno" })).toBeTruthy();
    // The soul is saved in place, against the profile's revision.
    const soul = within(profile).getByRole("textbox", { name: "Soul" });
    expect(soul).toHaveValue("Calm and direct.");
    fireEvent.change(soul, { target: { value: "Calm, direct and brief." } });
    await act(async () => { fireEvent.blur(soul); });
    expect(visions.saveVision).toHaveBeenCalledWith(expect.objectContaining({ id: "juno", soul: "Calm, direct and brief.", accessory: "clipboard" }), "r1");
    // Autonomy and budget are the channel's; the team lists each worker once.
    expect(within(profile).getByRole("combobox", { name: "Autonomy" })).toHaveDisplayValue("Most decisions");
    // Today's spend from the server, of the channel's limit (the board's wording).
    expect(await within(profile).findByText("$1.20 today")).toBeTruthy();
    expect(profile).toHaveTextContent("of $3");
    const team = within(profile).getByRole("list", { name: "Team" });
    expect(within(team).getAllByRole("listitem").map(item => item.textContent)).toEqual([expect.stringMatching(/^RookBuilder/), expect.stringMatching(/^WrenReviewer/)]);
    // Back to the channel.
    fireEvent.click(within(profile).getByRole("button", { name: "# release" }));
    expect(screen.getByRole("region", { name: "# release" })).toBeTruthy();
  });

  it("routines switch on and off and are removed from the chief's profile", async () => {
    const routines: Routine[] = [
      { id: "r1", chief: "juno", channel_id: "c1", schedule: { days: [1, 2, 3, 4, 5], time: "09:00" }, instruction: "Tell me what happened overnight", enabled: true },
      { id: "r2", chief: "juno", channel_id: "c1", schedule: { days: [5], time: "15:00" }, instruction: "Draft the release notes", enabled: false },
    ];
    const setEnabled = vi.fn(async () => undefined);
    const remove = vi.fn(async () => undefined);
    await show(() => <VisionProfile visionId="juno" routines={{ list: routines, setEnabled, remove }} />);
    const list = screen.getByRole("list", { name: "Routines" });
    expect(within(list).getByRole("switch", { name: "Weekdays 09:00" })).toBeChecked();
    expect(within(list).getByRole("switch", { name: "Fridays 15:00" })).not.toBeChecked();
    expect(list).toHaveTextContent("Tell me what happened overnight");
    await act(async () => { fireEvent.click(within(list).getByRole("switch", { name: "Fridays 15:00" })); });
    expect(setEnabled).toHaveBeenCalledWith("r2", true);
    await act(async () => { fireEvent.click(within(list).getByRole("button", { name: "Remove Weekdays 09:00" })); });
    expect(remove).toHaveBeenCalledWith("r1");
    cleanup();
    // With no routines the section is not there.
    await show(() => <Routines list={[]} setEnabled={setEnabled} remove={remove} />);
    expect(screen.queryByRole("list", { name: "Routines" })).toBeNull();
  });

  it("a_team_card_shows_what_was_created", async () => {
    const proposal = {
      summary: "Here is a team for it.", channels: [], visions: [], changes: [],
      team: {
        channel: { name: "release" },
        chief: { name: "Juno", role: "Chief of staff", color: "blue", accessory: "clipboard" },
        workers: [{ name: "Rook", role: "Builder", color: "teal", accessory: "hard_hat" }, { name: "Wren", role: "Reviewer", color: "violet", accessory: "glasses" }],
        autonomy: "most" as const, daily_budget_usd: 3,
      },
    };
    const entry = { type: "proposal" as const, vision_id: "admin", proposal_id: "p1", proposal };
    const applied = { type: "proposal_applied" as const, vision_id: "admin", proposal_id: "p1", items: [{ kind: "channel" as const, id: "c1", name: "release", action: "created" as const }] };
    const open = vi.fn();
    await show(() => <ProposalCard entry={entry} onOpenChannel={open} />);
    let card = screen.getByRole("group", { name: "New channel # release" });
    expect(within(card).getAllByRole("listitem").map(item => item.textContent)).toEqual(["JunoChief of staff", "RookBuilder", "WrenReviewer"]);
    expect(card).toHaveTextContent("Autonomy Most decisions");
    expect(card).toHaveTextContent("Budget $3 a day");
    expect(card).not.toHaveTextContent("Created");
    expect(card.querySelectorAll("svg[data-accessory]").length).toBe(3);
    cleanup();
    await show(() => <ProposalCard entry={entry} applied={applied} onOpenChannel={open} />);
    card = screen.getByRole("group", { name: "New channel # release" });
    expect(within(card).getByText("Created")).toBeTruthy();
    fireEvent.click(within(card).getByRole("button", { name: "Open # release" }));
    expect(open).toHaveBeenCalledWith("c1");
  });

  it("a hire card names the channel and the new worker", async () => {
    const proposal = { summary: "Juno asks for someone to test the screens.", channels: [], visions: [], changes: [], hire: { channel: "release", worker: { name: "Tess", role: "UI tester", color: "coral", accessory: "magnifier" } } };
    await show(() => <ProposalCard entry={{ type: "proposal", vision_id: "admin", proposal_id: "p2", proposal }} onOpenChannel={vi.fn()} />);
    const card = screen.getByRole("group", { name: "New worker for # release" });
    expect(card).toHaveTextContent("# releaseNew worker");
    expect(within(card).getByRole("listitem")).toHaveTextContent("TessUI tester");
  });

  it("# administration shows Atlas and no mode shield", async () => {
    await show(app, fixture({ open: { ...initialState.visions.open, channelId: "h1" } }));
    const channel = screen.getByRole("region", { name: "# administration" });
    // A new channel: the chief stands large in the middle with its name and job, and does not float.
    expect(within(channel).getAllByRole("button", { name: /^Atlas, admin/ })).toHaveLength(1);
    const welcome = within(channel).getByRole("img", { name: "Atlas, Calm" }).closest(".themis-chief-welcome") as HTMLElement;
    expect(welcome).toHaveTextContent("AtlasAdmin");
    expect(within(welcome).getByRole("button", { name: /^Atlas, admin/ })).toBeTruthy();
    expect(within(channel).getByRole("textbox", { name: "Message Atlas" })).toBeTruthy();
    expect(within(channel).queryByLabelText(/^Permissions/)).toBeNull();
  });

  it("requests between channels and new routines show as one line", async () => {
    const notes = [
      { type: "request_sent", vision_id: "juno", request_id: "q1", channel_id: "c2", ask: "fix the export date format" },
      { type: "routine_set", vision_id: "juno", routine_id: "r1", schedule: { days: [1, 2, 3, 4, 5], time: "09:00" }, instruction: "Tell me what happened overnight", changed: false },
      { type: "request_answered", vision_id: "tess", request_id: "q1", channel_id: "c2", accepted: true, note: "Friday" },
    ] as unknown as ChannelEntry[];
    const lines = notes.map((entry, index) => ({ seq: 10 + index, at_ms: Date.UTC(2026, 10, 14, 9, 5), entry }));
    await show(app, fixture({ lines: { c1: lines } }));
    const notesShown = screen.getAllByRole("note");
    // The request is quoted, so a question reads as well as an instruction.
    expect(notesShown[0]).toHaveTextContent(/^Asked# api“fix the export date format”/);
    expect(notesShown[1]).toHaveTextContent(/^Added a routine: Weekdays 09:00, Tell me what happened overnight/);
    expect(notesShown[2]).toHaveTextContent(/^Request with# apitaken on: Friday/);
    fireEvent.click(within(notesShown[0]!).getByRole("button", { name: "# api" }));
    expect(screen.getByRole("region", { name: "# api" })).toBeTruthy();
  });

  it("a request waiting on the person can be taken on or declined from its line", async () => {
    const waiting = [{ type: "request_received", vision_id: "tess", request_id: "q9", channel_id: "c2", ask: "review the changelog" }] as unknown as ChannelEntry[];
    await show(app, fixture({ lines: { c1: waiting.map((entry, index) => ({ seq: 30 + index, at_ms: 0, entry })) } }));
    const note = screen.getByRole("note");
    await act(async () => { fireEvent.click(within(note).getByRole("button", { name: "Take it on" })); });
    expect(visions.answerChannelRequest).toHaveBeenCalledWith("q9", true);
    // Once it is answered, the buttons go.
    cleanup();
    const answered = [...waiting, { type: "request_answered", vision_id: "", request_id: "q9", channel_id: "c2", accepted: true, note: "" }] as unknown as ChannelEntry[];
    await show(app, fixture({ lines: { c1: answered.map((entry, index) => ({ seq: 30 + index, at_ms: 0, entry })) } }));
    expect(screen.queryByRole("button", { name: "Take it on" })).toBeNull();
  });

  it("a malformed request or routine line is skipped, the rest of the channel stays", async () => {
    const broken = [
      { type: "request_sent", vision_id: "juno" },
      { type: "routine_set", vision_id: "juno", routine_id: "r1" },
      { type: "person", text: "Still here." },
    ] as unknown as ChannelEntry[];
    await show(app, fixture({ lines: { c1: broken.map((entry, index) => ({ seq: 20 + index, at_ms: 0, entry })) } }));
    const channel = screen.getByRole("region", { name: "# release" });
    expect(channel).toHaveTextContent("Still here.");
    expect(within(channel).queryAllByRole("note")).toHaveLength(0);
  });

  it("the_channel_composer_has_no_shield", async () => {
    await show();
    const channel = screen.getByRole("region", { name: "# release" });
    expect(within(channel).getByRole("textbox", { name: "Message Juno" })).toBeTruthy();
    expect(within(channel).queryByLabelText(/^Permissions/)).toBeNull();
    expect(tauri.getEffectivePermissions).not.toHaveBeenCalled();
  });

  it("channel_details_set_autonomy_and_full_access", async () => {
    await show(app, fixture({ open: { ...initialState.visions.open, channelId: "c1", panel: { kind: "channel" } } }));
    const details = screen.getByRole("complementary", { name: "Channel details" });
    expect(within(details).queryByRole("combobox", { name: "Approval mode" })).toBeNull();
    const field = within(details).getByRole("button", { name: "Autonomy" });
    expect(field).toHaveTextContent("Most decisions");
    fireEvent.click(field);
    const menu = within(details).getByRole("listbox", { name: "Autonomy" });
    // Each level says the approval mode it sets for the team's chats.
    expect(within(menu).getAllByRole("option").map(option => option.textContent)).toEqual([
      "Always askAsks before changes", "Routine decisionsAuto", "Most decisionsAuto", "Nearly all decisionsAuto", "CustomAuto",
    ]);
    expect(within(menu).getByRole("option", { name: /^Most decisions/ })).toHaveAttribute("aria-selected", "true");
    await act(async () => { fireEvent.click(within(menu).getByRole("option", { name: /^Always ask/ })); });
    expect(visions.saveChannel).toHaveBeenLastCalledWith(expect.objectContaining({ id: "c1", autonomy: "ask", full_access: false }), "c");
    expect(within(details).queryByRole("listbox")).toBeNull();
    // Custom starts from the rows the current level hands over.
    fireEvent.click(within(details).getByRole("button", { name: "Autonomy" }));
    await act(async () => { fireEvent.click(within(details).getByRole("option", { name: /^Custom/ })); });
    expect(visions.saveChannel).toHaveBeenLastCalledWith(expect.objectContaining({ autonomy: "custom", custom: expect.objectContaining({ wording: false, scope: false }) }), "c2");
    // Full access is its own switch.
    const full = within(details).getByRole("switch", { name: "Full access" });
    expect(full).not.toBeChecked();
    await act(async () => { fireEvent.click(full); });
    expect(visions.saveChannel).toHaveBeenLastCalledWith(expect.objectContaining({ full_access: true }), "c2");
  });

  it("a channel with no autonomy of its own shows Settings' level", async () => {
    const own = fixture({ open: { ...initialState.visions.open, channelId: "c2", panel: { kind: "channel" } } });
    await show(app, own);
    const details = screen.getByRole("complementary", { name: "Channel details" });
    expect(await within(details).findByText("Most decisions")).toBeTruthy();
    // # administration has neither.
    cleanup();
    await show(app, fixture({ open: { ...initialState.visions.open, channelId: "h1", panel: { kind: "channel" } } }));
    expect(screen.queryByRole("button", { name: "Autonomy" })).toBeNull();
    expect(screen.queryByRole("switch", { name: "Full access" })).toBeNull();
  });

  it("a hire for a channel that is not listed shows no empty pill", async () => {
    const proposal = { summary: "Add Tess?", channels: [], visions: [], changes: [], hire: { channel: "gone", worker: { name: "Tess", role: "UI tester" } } };
    await show(() => <ProposalCard entry={{ type: "proposal", vision_id: "admin", proposal_id: "p3", proposal }} onOpenChannel={vi.fn()} />);
    const card = screen.getByRole("group", { name: "New worker" });
    expect(card.querySelector(".themis-mention")).toBeNull();
    expect(card).toHaveTextContent(/^New worker/);
  });

  it("an unsaved soul survives another save, and follows a soul saved elsewhere", async () => {
    await show(() => <VisionProfile visionId="juno" />);
    const soul = () => screen.getByRole("textbox", { name: "Soul" });
    fireEvent.change(soul(), { target: { value: "Half typed" } });
    // The model saves meanwhile (a new revision): the draft stays.
    await act(async () => { fireEvent.change(screen.getByRole("combobox", { name: "Model" }), { target: { value: "" } }); });
    expect(visions.saveVision).toHaveBeenCalled();
    expect(soul()).toHaveValue("Half typed");
    // The soul saved from elsewhere replaces what is shown.
    const live2 = live.value as { state: AppState; dispatch: (action: AppAction) => void };
    await act(async () => { live2.dispatch({ type: "visions/loaded", visions: live2.state.visions.list.map(each => each.id === "juno" ? { ...each, soul: "From elsewhere.", revision: "r9" } : each) }); });
    expect(soul()).toHaveValue("From elsewhere.");
  });

  it("no_hint_text_on_team_screens", async () => {
    const scan = () => {
      // The composer's mode shield keeps its approved mode lines; nothing else explains itself.
      // The team screens: the channels bar and the Visions view (the project switcher is the app's).
      const outside = (element: Element) => !element.closest(".themis-composer-permissions") && !!element.closest(".themis-channels-bar, .themis-visions-main");
      // An icon button's tooltip may repeat its name; nothing says more than that.
      const titled = [...document.querySelectorAll("[title]")].filter(outside).filter(element => element.getAttribute("title") !== element.getAttribute("aria-label"));
      expect(titled.map(element => element.getAttribute("title"))).toEqual([]);
      const placeholders = [...document.querySelectorAll("[placeholder]")].map(element => element.getAttribute("placeholder"));
      expect(placeholders.every(text => /^Message \w+$/.test(text ?? ""))).toBe(true);
      expect(document.querySelector(".themis-settings-hint, .themis-visions-meta")).toBeNull();
      expect(screen.queryByText(/Waiting for your answer|Mention a Vision|No messages yet/)).toBeNull();
    };
    await show(app, fixture({ lines: { c1: [] } }));
    scan();
    fireEvent.click(within(within(bar()).getByRole("list", { name: "Team" })).getByRole("button", { name: /^Rook/ }));
    expect(screen.getByRole("complementary", { name: "Rook with Juno" })).toBeTruthy();
    scan();
    // The channel is new: the chief's name in the middle opens its profile.
    fireEvent.click(screen.getByRole("button", { name: /^Juno, chief of staff/ }));
    scan();
    cleanup();
    await show(app, fixture({ open: { ...initialState.visions.open, channelId: "h1" } }));
    scan();
  });
});
