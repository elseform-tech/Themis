import { afterEach, expect, it, vi } from "vitest";
import type { HistoryItem, ThreadEvent } from "../lib/types";
import { growthRate, SCALE_SIZES } from "../test/scale";
import { applyThreadEvent, initialState, reducer, type AppState } from "./reducer";

const THREAD = "chat";

function event(run: string, body: ThreadEvent): HistoryItem {
  return { kind: "event", envelope: { thread_id: THREAD, run_id: run, event: body } };
}

/** A chat of about `size` items: each exchange is a message, a tool call and an answer. */
function history(size: number): HistoryItem[] {
  const items: HistoryItem[] = [];
  for (let turn = 0; items.length < size; turn += 1) {
    const run = `run-${turn}`;
    items.push(
      { kind: "user", run_id: run, text: `question ${turn}` },
      event(run, { kind: "started" } as ThreadEvent),
      event(run, { kind: "tool_started", tool: "shell", summary: `step ${turn}` } as ThreadEvent),
      event(run, { kind: "tool_finished", tool: "shell", ok: true, output: "done" } as ThreadEvent),
      event(run, { kind: "finished", result: `answer ${turn}`, model: "m" } as ThreadEvent),
    );
  }
  return items;
}

/** Runs `work` counting the items of every array copied with a spread. */
function itemsSpread(work: () => void): number {
  const iterate = Array.prototype[Symbol.iterator];
  let items = 0;
  const spy = vi.spyOn(Array.prototype, Symbol.iterator).mockImplementation(function (this: unknown[]) {
    items += this.length;
    return iterate.call(this);
  });
  try {
    work();
  } finally {
    spy.mockRestore();
  }
  return items;
}

const load = (state: AppState, items: HistoryItem[]) =>
  reducer(state, { type: "thread/history-loaded", threadId: THREAD, history: items });

afterEach(() => vi.restoreAllMocks());

it("replaying a chat's history stays linear as the chat grows", () => {
  const rate = growthRate(SCALE_SIZES, size => {
    const items = history(size);
    return itemsSpread(() => load(initialState, items));
  });
  expect(rate).toBeLessThan(1.2);
});

/** What replay builds with no ids, which are drawn afresh each time. */
function rows(state: AppState) {
  return (state.messages[THREAD] ?? []).map(({ id: _id, ...row }) => row);
}

it("replay builds the same rows as the same events arriving one at a time", () => {
  const items = history(40);
  let live = initialState;
  for (const item of items) {
    if (item.kind === "event") live = applyThreadEvent(live, item.envelope, false);
    else live = reducer(live, { type: "message/append", threadId: THREAD, message: { id: "x", role: "user", text: (item as { text: string }).text, runId: (item as { run_id: string }).run_id } });
  }
  expect(rows(load(initialState, items))).toEqual(rows(live));
});

it("replay leaves earlier state alone, and later events don't change what it returned", () => {
  const before = load(initialState, history(10));
  const shown = before.messages[THREAD]!;
  const length = shown.length;
  const again = load(before, history(5));
  expect(before.messages[THREAD]).toBe(shown);
  expect(shown).toHaveLength(length);
  const after = reducer(again, { type: "message/append", threadId: THREAD, message: { id: "new", role: "user", text: "more" } });
  expect(again.messages[THREAD]).not.toBe(after.messages[THREAD]);
  expect(after.messages[THREAD]).toHaveLength(again.messages[THREAD]!.length + 1);
});
