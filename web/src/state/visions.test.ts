import { describe, expect, it, vi } from "vitest";
import type { Question, ThreadInfo } from "../lib/types";
import {
  APPROVAL_MODES, ENTRY_FIELDS, HOLDS, NOTICE_KINDS, OWNED_KINDS,
  type ChannelEntry, type ChannelInfo, type ChannelLine, type ChannelPage, type OwnedChat, type VisionInfo, type VisionsChange,
} from "../lib/visions";
import fixture from "../lib/visions.fixture.json";
import { initialState, reducer, type AppState } from "./reducer";
import {
  followVisions,
  MOOD_LABEL,
  needsCount,
  railMood,
  reloadVisions,
  visionMood,
  visionMoods,
  WINDOW_LINES,
  type Mood,
  type VisionsBridge,
} from "./visions";

const VISION: VisionInfo = {
  id: "v1", name: "Wren", role: "", shape: "cloud", color: "teal", soul: "", paused: false,
  revision: "r1", over_budget: false, vision_chats: [], needs: 0, sorry: false,
};

function chat(id: string, extra: Partial<ThreadInfo> = {}): ThreadInfo {
  return { id, title: id, provider: "go", model: "m", running: false, worktree_path: null, branch: null, base_branch: null, recovered: false, skill_ids: [], ...extra };
}

function owned(id: string, channelId: string, visionId = "v1", extra: Partial<ThreadInfo> = {}, hold: OwnedChat["owner"]["hold"] = "none"): OwnedChat {
  return { ...chat(id, extra), owner: { thread_id: id, vision_id: visionId, channel_id: channelId, kind: "work", hold } };
}

function channel(id: string, root = "/repo"): ChannelInfo {
  return { id, name: id, topic: "", project_root: root, approval_mode: "custom", members: ["v1"], revision: "c1" };
}

function line(seq: number, entry: ChannelEntry = { type: "person", text: `line ${seq}` }): ChannelLine {
  return { seq, at_ms: seq * 1000, entry };
}

/** The server's page of the lines `from..=to`: `next_before` is its first seq (null at 1). */
function page(from: number, to: number): ChannelPage {
  const entries = [];
  for (let seq = from; seq <= to; seq += 1) entries.push(line(seq));
  return { entries, next_before: from > 1 ? from : null };
}

describe("visionMood", () => {
  const cases: Array<[string, VisionInfo, boolean, boolean, Mood]> = [
    ["paused sleeps even when something waits", { ...VISION, paused: true, needs: 2 }, true, true, "sleep"],
    ["over budget sleeps", { ...VISION, over_budget: true }, true, true, "sleep"],
    ["something waiting needs the person first", { ...VISION, needs: 1, sorry: true }, true, true, "needs"],
    ["sorry before work", { ...VISION, sorry: true }, true, true, "sorry"],
    ["a running chat is work before its own thinking", VISION, true, true, "work"],
    ["its own run alone is thinking", VISION, false, true, "think"],
    ["nothing going on is calm", VISION, false, false, "calm"],
    ["sorry stays while nothing runs", { ...VISION, sorry: true }, false, false, "sorry"],
  ];
  it.each(cases)("mood_priority: %s", (_name, vision, working, thinking, mood) => {
    expect(visionMood(vision, working, thinking)).toBe(mood);
  });

  it("labels each mood in words, calm with none", () => {
    expect(MOOD_LABEL).toEqual({ needs: "Needs you", sorry: "Sorry", work: "Working", think: "Thinking", calm: "", sleep: "Asleep" });
  });
});

describe("railMood", () => {
  it("rail_is_asleep_only_when_every_vision_is", () => {
    expect(railMood(["sleep", "sleep"])).toBe("sleep");
    expect(railMood(["sleep", "calm"])).toBe("calm");
    expect(railMood(["calm", "think", "sleep", "work"])).toBe("work");
    expect(railMood(["work", "needs", "sorry"])).toBe("needs");
    expect(railMood(["think", "sorry"])).toBe("sorry");
    expect(railMood([])).toBe("calm");
  });
});

/** State with one channel at `/repo` holding the page `entries`, its owned chats `chats`. */
function withChannel(entries: ChannelLine[], chats: OwnedChat[], visions: VisionInfo[] = [VISION]): AppState {
  let state = reducer(initialState, { type: "visions/loaded", visions, projectRoot: "/repo", channels: [channel("c1")] });
  state = reducer(state, {
    type: "visions/changed", channelId: "c1",
    page: { entries, next_before: entries.length > 0 && entries[0]!.seq > 1 ? entries[0]!.seq : null },
    chats, questions: {},
  });
  return state;
}

describe("needs and moods come from the server's list", () => {
  it("needs_count_is_the_listed_needs", () => {
    const state = withChannel([], [], [{ ...VISION, needs: 2 }, { ...VISION, id: "v2", needs: 1 }]);
    expect(needsCount(state)).toBe(3);
  });

  it("moods read the work chats' runs live, not a chat the person took over", () => {
    const vision: VisionInfo = { ...VISION, vision_chats: [{ thread_id: "own", channel_id: "c1", running: false }] };
    let state = withChannel([], [owned("w1", "c1"), owned("mine", "c1", "v1", {}, "taken_over")], [vision]);
    expect(visionMoods(state)).toEqual({ v1: "calm" });
    state = { ...state, running: { ...state.running, mine: true } };
    expect(visionMoods(state)).toEqual({ v1: "calm" });
    state = { ...state, running: { ...state.running, own: true } };
    expect(visionMoods(state)).toEqual({ v1: "think" });
    state = { ...state, running: { ...state.running, w1: true } };
    expect(visionMoods(state)).toEqual({ v1: "work" });
  });
});

function bridge(): VisionsBridge {
  return {
    listVisions: vi.fn(async () => [VISION]),
    listChannels: vi.fn(async (root: string) => [channel("c1", root), channel("c2", root)]),
    getChannelPage: vi.fn(async () => ({ entries: [line(5, { type: "passed_on", vision_id: "v1", thread_id: "w1", question_id: "q1", decision: "scope", question: "?" })], next_before: 5 })),
    listChannelThreads: vi.fn(async (id: string) => [owned("w1", id)]),
    listQuestions: vi.fn(async (): Promise<Question[]> => [{ id: "q1", run_id: "r", question: "?", blocking: false, asked_at: 1 }]),
  };
}

describe("a channel's window of lines", () => {
  it("a newer page keeps the older lines already read when nothing lies between", () => {
    let state = withChannel([line(1), line(2)], []);
    state = reducer(state, { type: "visions/changed", channelId: "c1", page: page(2, 3), chats: [], questions: {} });
    expect(state.visions.lines.c1!.map(l => l.seq)).toEqual([1, 2, 3]);
    expect(state.visions.nextBefore.c1).toBeNull();
  });

  it("keeps only the newest lines, and says where the log continues back", () => {
    let state = withChannel(page(1, WINDOW_LINES).entries, []);
    state = reducer(state, { type: "visions/changed", channelId: "c1", page: page(WINDOW_LINES, WINDOW_LINES + 10), chats: [], questions: {} });
    const seqs = state.visions.lines.c1!.map(l => l.seq);
    expect(seqs).toHaveLength(WINDOW_LINES);
    expect(seqs[seqs.length - 1]).toBe(WINDOW_LINES + 10);
    expect(state.visions.nextBefore.c1).toBe(seqs[0]);
  });

  it("older lines read on request stay with the window", async () => {
    let state = withChannel(page(501, 600).entries, []);
    const api = { ...bridge(), getChannelPage: vi.fn(async (_id: string, before?: number | null) => page(Math.max(1, (before ?? 601) - 100), (before ?? 601) - 1)) };
    const follower = followVisions(api, () => state, action => { state = reducer(state, action); }, () => false);
    await follower.loadOlder("c1");
    expect(api.getChannelPage).toHaveBeenCalledWith("c1", 501);
    expect(state.visions.lines.c1!.map(l => l.seq)).toEqual(page(401, 600).entries.map(l => l.seq));
    expect(state.visions.nextBefore.c1).toBe(401);
    // A page read again keeps what was asked for; the window keeps that many lines, so each new
    // line moves it on by one.
    state = reducer(state, { type: "visions/changed", channelId: "c1", page: page(550, 600), chats: [], questions: {} });
    expect(state.visions.lines.c1![0]!.seq).toBe(401);
    state = reducer(state, { type: "visions/changed", channelId: "c1", page: page(550, 601), chats: [], questions: {} });
    expect(state.visions.lines.c1!.map(l => l.seq)).toEqual(page(402, 601).entries.map(l => l.seq));
    expect(state.visions.nextBefore.c1).toBe(402);
  });

  it("a channel gone from its project's list is forgotten", () => {
    let state = withChannel([line(1)], [owned("w1", "c1")]);
    state = reducer(state, { type: "visions/loaded", projectRoot: "/repo", channels: [] });
    expect(state.visions.channels["/repo"]).toEqual([]);
    expect(state.visions.lines.c1).toBeUndefined();
    expect(state.visions.chats.c1).toBeUndefined();
  });
});

const apply = (state: AppState, dispatch: ReturnType<typeof vi.fn>) =>
  dispatch.mock.calls.reduce((s: AppState, [action]) => reducer(s, action), state);

describe("reloadVisions", () => {
  it("visions_changed_reloads_only_that_channel", async () => {
    const api = bridge();
    const dispatch = vi.fn();
    let state = withChannel([], []);
    state = reducer(state, { type: "visions/changed", channelId: "c2", page: { entries: [], next_before: null }, chats: [], questions: {} });
    await reloadVisions(api, { channel_id: "c1" }, state, dispatch);
    expect(api.getChannelPage).toHaveBeenCalledTimes(1);
    expect(api.getChannelPage).toHaveBeenCalledWith("c1");
    expect(api.listChannelThreads).toHaveBeenCalledTimes(1);
    expect(api.listQuestions).toHaveBeenCalledWith("w1");
    expect(api.listVisions).not.toHaveBeenCalled();
    const next = apply(state, dispatch);
    expect(next.visions.lines.c1!.map(l => l.seq)).toEqual([5]);
    expect(next.visions.questions).toEqual({ w1: ["q1"] });
  });

  it("a change naming a Vision reads the list again, with a channel too", async () => {
    const api = bridge();
    const flipped: VisionInfo[] = [{ ...VISION, over_budget: true }];
    api.listVisions = vi.fn(async () => flipped);
    const dispatch = vi.fn();
    const state = withChannel([], []);
    await reloadVisions(api, { channel_id: "c1", vision_id: "v1" }, state, dispatch);
    expect(api.listVisions).toHaveBeenCalledTimes(1);
    // Only the channel named is read again.
    expect(api.getChannelPage).toHaveBeenCalledTimes(1);
    expect(visionMoods(apply(state, dispatch))).toEqual({ v1: "sleep" });
  });

  it("a Vision alone reads only the list; no ids reads everything", async () => {
    const api = bridge();
    const state = withChannel([], []);
    await reloadVisions(api, { vision_id: "v1" }, state, vi.fn());
    expect(api.listVisions).toHaveBeenCalledTimes(1);
    expect(api.listChannels).not.toHaveBeenCalled();
    expect(api.getChannelPage).not.toHaveBeenCalled();
    const all = bridge();
    await reloadVisions(all, {}, state, vi.fn());
    expect(all.listVisions).toHaveBeenCalledTimes(1);
    expect(all.listChannels).toHaveBeenCalledWith("/repo");
    expect(all.getChannelPage).toHaveBeenCalledTimes(2);
  });

  it("one channel failing to read keeps the others", async () => {
    const api = bridge();
    api.getChannelPage = vi.fn(async (id: string) => {
      if (id === "c2") throw new Error("gone");
      return page(1, 2);
    });
    const dispatch = vi.fn();
    const state = withChannel([], []);
    await reloadVisions(api, {}, state, dispatch);
    const changed = dispatch.mock.calls.map(([action]) => action).filter(action => action.type === "visions/changed");
    expect(changed.map(action => action.channelId)).toEqual(["c1"]);
  });

  it("a failed question read leaves what was shown", async () => {
    const api = bridge();
    api.listQuestions = vi.fn(async () => { throw new Error("offline"); });
    const dispatch = vi.fn();
    let state = withChannel([], []);
    state = reducer(state, { type: "visions/changed", channelId: "c1", page: { entries: [], next_before: null }, chats: [], questions: { w1: ["q1"] } });
    await reloadVisions(api, { channel_id: "c1" }, state, dispatch);
    expect(apply(state, dispatch).visions.questions).toEqual({ w1: ["q1"] });
  });
});

describe("followVisions", () => {
  it("changes arriving while one reads are merged into one more read", async () => {
    const api = bridge();
    let release!: () => void;
    const first = new Promise<void>(resolve => { release = resolve; });
    let calls = 0;
    api.listVisions = vi.fn(async () => { calls += 1; if (calls === 1) await first; return [VISION]; });
    let state = withChannel([], []);
    const follower = followVisions(api, () => state, action => { state = reducer(state, action); }, () => false);
    void follower.changed({ vision_id: "v1" });
    void follower.changed({ vision_id: "v1" });
    void follower.changed({ channel_id: "c1", vision_id: "v1" });
    const done = follower.changed({ vision_id: "v1" });
    release();
    await done;
    expect(api.listVisions).toHaveBeenCalledTimes(2);
  });

  it("an approval asked or gone in a chat a Vision drives reads only the list again", async () => {
    const api = bridge();
    const vision: VisionInfo = { ...VISION, vision_chats: [{ thread_id: "own", channel_id: "c1", running: true }] };
    api.listVisions = vi.fn(async () => [vision]);
    let state = withChannel([], [owned("w1", "c1")], [vision]);
    const follower = followVisions(api, () => state, action => { state = reducer(state, action); }, () => false);
    await follower.approvalChanged("w1");
    expect(api.listVisions).toHaveBeenCalledTimes(1);
    await follower.approvalChanged("own");
    expect(api.listVisions).toHaveBeenCalledTimes(2);
    await follower.approvalChanged("plain");
    expect(api.listVisions).toHaveBeenCalledTimes(2);
    expect(api.getChannelPage).not.toHaveBeenCalled();
    // A run's end reads nothing: the server announces the Vision once its outcome is recorded.
    expect("threadEvent" in follower).toBe(false);
  });

  it("reads nothing once cancelled", async () => {
    const api = bridge();
    let cancelled = false;
    const dispatch = vi.fn();
    const follower = followVisions(api, () => withChannel([], []), dispatch, () => cancelled);
    cancelled = true;
    await follower.changed({});
    expect(api.listVisions).not.toHaveBeenCalled();
    expect(dispatch).not.toHaveBeenCalled();
  });
});

describe("wire fixture", () => {
  it("holds every channel entry with exactly the fields the app's types name", () => {
    const lines = fixture.lines as ChannelLine[];
    const seen = new Set(lines.map(l => l.entry.type));
    expect([...seen].sort()).toEqual(Object.keys(ENTRY_FIELDS).sort());
    for (const { entry } of lines) {
      expect(Object.keys(entry).sort(), entry.type).toEqual(["type", ...ENTRY_FIELDS[entry.type]].sort());
    }
    expect([...fixture.notice_kinds].sort()).toEqual(Object.keys(NOTICE_KINDS).sort());
  });

  it("names every value of the unions the app reads", () => {
    expect([...fixture.holds].sort()).toEqual(Object.keys(HOLDS).sort());
    expect([...fixture.owned_kinds].sort()).toEqual(Object.keys(OWNED_KINDS).sort());
    expect([...fixture.approval_modes].sort()).toEqual(Object.keys(APPROVAL_MODES).sort());
    expect(fixture.owner.kind in OWNED_KINDS && fixture.owner.hold in HOLDS && fixture.channel.approval_mode in APPROVAL_MODES).toBe(true);
  });

  it("holds each shape with exactly the fields the app's types name", () => {
    const keys = (value: object) => Object.keys(value).sort();
    const visionKeys: Array<keyof VisionInfo> = ["id", "name", "role", "shape", "color", "model", "autonomy", "custom", "daily_budget_usd", "soul", "paused", "revision", "over_budget", "vision_chats", "needs", "sorry"];
    expect(keys(fixture.vision)).toEqual([...visionKeys].sort());
    const chatKeys: Array<keyof VisionInfo["vision_chats"][number]> = ["thread_id", "channel_id", "running"];
    expect(keys(fixture.vision.vision_chats[0]!)).toEqual([...chatKeys].sort());
    const channelKeys: Array<keyof ChannelInfo> = ["id", "name", "topic", "project_root", "approval_mode", "members", "revision"];
    expect(keys(fixture.channel)).toEqual([...channelKeys].sort());
    const ownerKeys: Array<keyof OwnedChat["owner"]> = ["thread_id", "vision_id", "channel_id", "kind", "hold"];
    expect(keys(fixture.owner)).toEqual([...ownerKeys].sort());
    const pageKeys: Array<keyof ChannelPage> = ["entries", "next_before"];
    expect(keys(fixture.page)).toEqual([...pageKeys].sort());
    const changeKeys: Array<keyof VisionsChange> = ["channel_id", "vision_id"];
    expect(keys(fixture.change)).toEqual([...changeKeys].sort());
    const questionKeys: Array<keyof Question> = ["id", "run_id", "question", "choices", "assumption", "reasons", "group", "blocking", "topic", "subagent", "asked_at", "answer", "answered_by", "permission", "passed_on"];
    expect(keys(fixture.question)).toEqual([...questionKeys].sort());
  });
});
