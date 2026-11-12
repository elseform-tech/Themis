import { expect, it } from "vitest";
import type { ChannelLine, VisionInfo } from "../lib/visions";
import { countedList, growthRate, SCALE_SIZES } from "../test/scale";
import { initialState, reducer, type AppState } from "./reducer";
import { needsCount, visionMoods } from "./visions";

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
