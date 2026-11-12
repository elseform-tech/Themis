import { expect, it } from "vitest";
import type { ChannelLine, VisionInfo } from "../lib/visions";
import { countedList, growthRate, SCALE_SIZES } from "../test/scale";
import { initialState, reducer, type AppState } from "./reducer";
import { needsCount, visionMoods } from "./visions";
import { selectionToSave } from "./store";
import type { OwnedChat } from "../lib/visions";

const VISION: VisionInfo = {
  id: "v1", name: "Wren", role: "", shape: "cloud", color: "teal", soul: "", paused: false,
  revision: "r1", over_budget: false, vision_chats: [], needs: 0, sorry: false,
};

const line = (seq: number): ChannelLine => ({ seq, at_ms: seq, entry: { type: "person", text: `line ${seq}` } });

/** The server's newest page after `last` entries: the newest 50, `next_before` its first seq. */
function newest(last: number) {
  const from = Math.max(1, last - 49);
  const entries: ChannelLine[] = [];
  for (let seq = from; seq <= last; seq += 1) entries.push(line(seq));
  return { entries, next_before: from > 1 ? from : null };
}

/** A channel after `size` entries were written one at a time, each followed by a read of its
 *  newest page, as the app does on each `visions-changed`. */
function session(size: number): AppState {
  let state = reducer(initialState, {
    type: "visions/loaded", visions: [VISION], projectRoot: "/repo",
    channels: [{ id: "c1", name: "hq", topic: "", project_root: "/repo", approval_mode: "custom", members: ["v1"], revision: "r" }],
  });
  for (let last = 1; last <= size; last += 1) {
    state = reducer(state, { type: "visions/changed", channelId: "c1", page: newest(last), chats: [], questions: {} });
  }
  return state;
}

/** Lines of the channel read while one more entry's page joins it and the cloud is drawn. */
function linesReadOnOneEvent(size: number): number {
  const state = session(size);
  const counted = countedList(state.visions.lines.c1!);
  const before: AppState = { ...state, visions: { ...state.visions, lines: { c1: counted.list } } };
  const after = reducer(before, { type: "visions/changed", channelId: "c1", page: newest(size + 1), chats: [], questions: {} });
  needsCount(after);
  visionMoods(after);
  // One more than the lines read, so the growth rate never takes a log of 0.
  return counted.reads() + 1;
}

it("one more channel entry costs the same however long the channel has been followed", () => {
  const work = SCALE_SIZES.map(linesReadOnOneEvent);
  // The window holds the newest lines only, so the work per event is flat.
  expect(work[0]).toBe(work[1]);
  expect(growthRate(SCALE_SIZES, linesReadOnOneEvent)).toBeLessThan(0.1);
});

const OWNED = (id: string): OwnedChat => ({
  id, title: id, provider: "go", model: "m", running: false, worktree_path: null, branch: null, base_branch: null, recovered: false, skill_ids: [],
  owner: { thread_id: id, vision_id: "v1", channel_id: "c1", kind: "work", hold: "none" },
});

/** Owned chats read while the selection to save is worked out on each of 50 thread events, with
 *  `size` owned chats followed (the chats list is unchanged by thread events). */
function ownedChatsReadPerEvents(size: number): number {
  const counted = countedList(Array.from({ length: size }, (_, at) => OWNED(`w${at}`)));
  let state: AppState = { ...initialState, activeProjectRoot: "/repo", activeThreadId: "plain", visions: { ...initialState.visions, chats: { c1: counted.list } } };
  selectionToSave(state);
  const before = counted.reads();
  for (let event = 0; event < 50; event += 1) {
    state = reducer(state, { type: "thread/running-loaded", threadId: "plain", running: event % 2 === 0 });
    selectionToSave(state);
  }
  return counted.reads() - before + 1;
}

it("saving the plain selection reads no owned chats per thread event, however many there are", () => {
  const work = SCALE_SIZES.map(ownedChatsReadPerEvents);
  expect(work).toEqual([1, 1]);
  expect(growthRate(SCALE_SIZES, ownedChatsReadPerEvents)).toBeLessThan(0.1);
});
