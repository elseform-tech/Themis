import { describe, expect, it, vi } from "vitest";
import type { ApprovalRequest, ThreadInfo } from "../lib/types";
import { ENTRY_FIELDS, NOTICE_KINDS, type ChannelEntry, type ChannelInfo, type ChannelLine, type OwnedChat, type VisionInfo } from "../lib/visions";
import fixture from "../lib/visions.fixture.json";
import { initialState, reducer, type AppState } from "./reducer";
import {
  MOOD_LABEL,
  needsCount,
  railMood,
  reloadVisions,
  visionMood,
  type Mood,
  type VisionsBridge,
} from "./visions";

const VISION: VisionInfo = {
  id: "v1", name: "Wren", role: "", shape: "cloud", color: "teal", soul: "", paused: false,
  revision: "r1", over_budget: false, vision_chats: [],
};

function chat(id: string, extra: Partial<ThreadInfo> = {}): ThreadInfo {
  return { id, title: id, provider: "go", model: "m", running: false, worktree_path: null, branch: null, base_branch: null, recovered: false, skill_ids: [], ...extra };
}

function owned(id: string, channelId: string, visionId = "v1", extra: Partial<ThreadInfo> = {}): OwnedChat {
  return { ...chat(id, extra), owner: { thread_id: id, vision_id: visionId, channel_id: channelId, kind: "work", hold: "none" } };
}

function channel(id: string, root = "/repo"): ChannelInfo {
  return { id, name: id, topic: "", project_root: root, approval_mode: "custom", members: ["v1"], revision: "c1" };
}

function line(seq: number, entry: ChannelEntry): ChannelLine {
  return { seq, at_ms: seq * 1000, entry };
}

function approval(threadId: string, id: string): ApprovalRequest {
  return { thread_id: threadId, approval_id: id, tool: "shell", summary: "", risk: "execute" } as ApprovalRequest;
}

describe("visionMood", () => {
  const failed = chat("a", { last_outcome: "failed" });
  const running = chat("b", { running: true });
  const cases: Array<[string, VisionInfo, ThreadInfo[], number, boolean, Mood]> = [
    ["paused sleeps even when something waits", { ...VISION, paused: true }, [running], 2, true, "sleep"],
    ["over budget sleeps", { ...VISION, over_budget: true }, [running], 0, true, "sleep"],
    ["something waiting needs the person first", VISION, [failed, running], 1, true, "needs"],
    ["a failed chat is sorry before work", VISION, [failed, running], 0, true, "sorry"],
    ["a running chat is work before its own thinking", VISION, [running], 0, true, "work"],
    ["its own run alone is thinking", VISION, [chat("c", { last_outcome: "done" })], 0, true, "think"],
    ["nothing going on is calm", VISION, [], 0, false, "calm"],
    ["a stopped chat is not sorry", VISION, [chat("d", { last_outcome: "stopped" })], 0, false, "calm"],
  ];
  it.each(cases)("mood_priority: %s", (_name, vision, chats, waiting, runningSelf, mood) => {
    expect(visionMood(vision, chats, waiting, runningSelf)).toBe(mood);
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

/** State with one channel holding `entries`, its owned chats `chats`. */
function withChannel(entries: ChannelEntry[], chats: OwnedChat[], questions: Record<string, string[]> = {}): AppState {
  let state = reducer(initialState, { type: "visions/loaded", visions: [VISION], projectRoot: "/repo", channels: [channel("c1")] });
  state = reducer(state, {
    type: "visions/changed", channelId: "c1",
    page: { entries: entries.map((entry, at) => line(at + 1, entry)), next_before: null },
    chats, questions,
  });
  return state;
}

describe("needsCount", () => {
  const passed = (thread: string, question: string): ChannelEntry => ({ type: "passed_on", vision_id: "v1", thread_id: thread, question_id: question, decision: "scope", question: "Which?" });

  it("needs_count_adds_approvals_and_passed_questions", () => {
    let state = withChannel(
      [passed("w1", "q1"), passed("w1", "q2"), passed("w1", "q1"), passed("w2", "q3")],
      [owned("w1", "c1"), owned("w2", "c1")],
      // q2 was answered since it was passed on; q3's chat history is loaded below.
      { w1: ["q1"], w2: ["q3"] },
    );
    state = { ...state, approvals: [approval("w1", "a1"), approval("w2", "a2"), approval("plain", "a3")] };
    // Two approvals on owned chats (not the plain chat's), and q1 once.
    expect(needsCount(state)).toBe(2 + 2);
    // A loaded chat's history says what is open: q3 answered there.
    state = {
      ...state,
      historyLoaded: { ...state.historyLoaded, w2: true },
      messages: { ...state.messages, w2: [{ id: "question-q3", role: "system", text: "Which?", question: { id: "q3", run_id: "r", question: "Which?", blocking: false, asked_at: 1, answer: "B" } }] },
    };
    expect(needsCount(state)).toBe(2 + 1);
    // An answer arriving live closes it too.
    state = reducer(state, { type: "thread/event", envelope: { thread_id: "w1", event: { kind: "question_answered", id: "q1", answer: "A" } } as never });
    expect(needsCount(state)).toBe(2);
  });
});

describe("reloadVisions", () => {
  function bridge(): VisionsBridge {
    return {
      listVisions: vi.fn(async () => [VISION]),
      listChannels: vi.fn(async (root: string) => [channel("c1", root), channel("c2", root)]),
      getChannelPage: vi.fn(async () => ({ entries: [line(5, { type: "passed_on", vision_id: "v1", thread_id: "w1", question_id: "q1", decision: "scope", question: "?" })], next_before: 4 })),
      listChannelThreads: vi.fn(async (id: string) => [owned("w1", id)]),
      listQuestions: vi.fn(async () => [{ id: "q1", run_id: "r", question: "?", blocking: false, asked_at: 1 }]),
    };
  }

  it("visions_changed_reloads_only_that_channel", async () => {
    const api = bridge();
    const dispatch = vi.fn();
    let state = withChannel([], []);
    state = reducer(state, { type: "visions/changed", channelId: "c2", page: { entries: [], next_before: null }, chats: [], questions: {} });
    await reloadVisions(api, { channel_id: "c1" }, state, dispatch);
    expect(api.getChannelPage).toHaveBeenCalledTimes(1);
    expect(api.getChannelPage).toHaveBeenCalledWith("c1");
    expect(api.listChannelThreads).toHaveBeenCalledTimes(1);
    expect(api.listChannelThreads).toHaveBeenCalledWith("c1");
    expect(api.listQuestions).toHaveBeenCalledWith("w1");
    expect(api.listVisions).not.toHaveBeenCalled();
    const changed = dispatch.mock.calls.map(([action]) => action).filter(action => action.type === "visions/changed");
    expect(changed).toHaveLength(1);
    expect(changed[0]).toMatchObject({ channelId: "c1", questions: { w1: ["q1"] } });
    const next = dispatch.mock.calls.reduce((s: AppState, [action]) => reducer(s, action), state);
    expect(next.visions.lines.c1.map(l => l.seq)).toEqual([5]);
    expect(needsCount(next)).toBe(1);
  });

  it("a Vision's change reads the Visions and the channels it is in; no ids reads everything", async () => {
    const api = bridge();
    const dispatch = vi.fn();
    const state = withChannel([], []);
    await reloadVisions(api, { vision_id: "v1" }, state, dispatch);
    expect(api.listVisions).toHaveBeenCalledTimes(1);
    expect(api.getChannelPage).toHaveBeenCalledWith("c1");
    const all = bridge();
    await reloadVisions(all, {}, state, vi.fn());
    expect(all.listVisions).toHaveBeenCalledTimes(1);
    expect(all.listChannels).toHaveBeenCalledWith("/repo");
    expect(all.getChannelPage).toHaveBeenCalledTimes(2);
  });

  it("a newer page keeps the older entries already read", () => {
    let state = withChannel([{ type: "person", text: "one" }, { type: "person", text: "two" }], []);
    state = reducer(state, { type: "visions/changed", channelId: "c1", page: { entries: [line(2, { type: "person", text: "two" }), line(3, { type: "person", text: "three" })], next_before: 1 }, chats: [], questions: {} });
    expect(state.visions.lines.c1.map(l => l.seq)).toEqual([1, 2, 3]);
  });

  it("a channel gone from its project's list is forgotten", () => {
    let state = withChannel([{ type: "person", text: "one" }], [owned("w1", "c1")]);
    state = reducer(state, { type: "visions/loaded", projectRoot: "/repo", channels: [] });
    expect(state.visions.channels["/repo"]).toEqual([]);
    expect(state.visions.lines.c1).toBeUndefined();
    expect(state.visions.chats.c1).toBeUndefined();
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
    const listed = fixture.vision as VisionInfo;
    const profile: Array<keyof VisionInfo> = ["id", "name", "role", "shape", "color", "model", "autonomy", "custom", "daily_budget_usd", "soul", "paused", "revision", "over_budget", "vision_chats"];
    expect(Object.keys(listed).sort()).toEqual([...profile].sort());
    const chatKeys: Array<keyof VisionInfo["vision_chats"][number]> = ["thread_id", "channel_id", "running"];
    expect(Object.keys(listed.vision_chats[0]!).sort()).toEqual([...chatKeys].sort());
    const channelKeys: Array<keyof ChannelInfo> = ["id", "name", "topic", "project_root", "approval_mode", "members", "revision"];
    expect(Object.keys(fixture.channel).sort()).toEqual([...channelKeys].sort());
    const ownerKeys: Array<keyof OwnedChat["owner"]> = ["thread_id", "vision_id", "channel_id", "kind", "hold"];
    expect(Object.keys(fixture.owner).sort()).toEqual([...ownerKeys].sort());
  });
});
