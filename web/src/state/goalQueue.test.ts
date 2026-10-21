import { describe, expect, it } from "vitest";
import { goalTurnNumber, initialState, reducer } from "./reducer";

const thread = "t1";
const envelope = (event: object, run_id = "r1") => ({ type: "thread/event" as const, envelope: { thread_id: thread, run_id, event: event as never } });

describe("queue and goal events", () => {
  it("keeps the queue and goal of each chat", () => {
    let state = reducer(initialState, envelope({ kind: "queue_changed", queue: [{ id: "q", text: "later", attachments: [], held: false }] }, ""));
    state = reducer(state, envelope({ kind: "goal_changed", goal: { text: "speed up p95", status: "running", started_at: 0, active_ms: 0, turns: 1, stalls: 0, failures: 0 } }, ""));
    expect(state.queues[thread]?.map(item => item.text)).toEqual(["later"]);
    expect(state.goals[thread]?.text).toBe("speed up p95");
    state = reducer(state, envelope({ kind: "goal_changed", goal: null }, ""));
    expect(state.goals[thread]).toBeNull();
  });

  it("puts a steer after the text streamed before it, marked as steered", () => {
    let state = reducer(initialState, envelope({ kind: "started", task: "x", max_turns: 5 }));
    state = reducer(state, envelope({ kind: "assistant_text", text: "Reading the file." }));
    state = reducer(state, envelope({ kind: "steered", text: "Use tabs." }));
    const rows = state.messages[thread]!;
    expect(rows.map(row => [row.role, row.text, row.steered ?? false])).toEqual([["assistant", "Reading the file.", false], ["user", "Use tabs.", true]]);
    expect(state.streams[thread]).toBe("");
  });

  it("shows a message sent from another client once, and adopts this window's own", () => {
    // From the CLI or another window: nothing on screen yet, so it is added.
    let state = reducer(initialState, envelope({ kind: "user_message", text: "Fix the clock", attachments: [] }, "r1"));
    // A repeat of the same run's message changes nothing.
    state = reducer(state, envelope({ kind: "user_message", text: "Fix the clock", attachments: [] }, "r1"));
    // This window's send is drawn before the server saves it; the event adopts it.
    state = reducer(state, { type: "message/append", threadId: thread, message: { id: "mine", role: "user", text: "Now record it" } });
    state = reducer(state, envelope({ kind: "user_message", text: "Now record it", attachments: [] }, "r2"));
    expect(state.messages[thread]!.map(row => [row.text, row.runId])).toEqual([["Fix the clock", "r1"], ["Now record it", "r2"]]);
    expect(state.messages[thread]![1].id).toBe("mine");
  });

  it("shows goal turns as dividers and goal notes as notes, live and from history", () => {
    const turn = "[Goal — runtime, not from the user] Turn 24. Goal: speed up p95\nLast check: still slow";
    expect(goalTurnNumber(turn)).toBe(24);
    expect(goalTurnNumber("Turn 24")).toBeUndefined();
    let state = reducer(initialState, envelope({ kind: "user_message", text: turn }, ""));
    state = reducer(state, envelope({ kind: "goal_note", outcome: "not_yet", reason: "p95 is 238 ms." }));
    expect(state.messages[thread]!.map(row => [row.goalTurn, row.goalNote?.outcome])).toEqual([[24, undefined], [undefined, "not_yet"]]);
    const loaded = reducer(initialState, { type: "thread/history-loaded", threadId: thread, history: [
      { kind: "user", run_id: "r2", text: turn, attachments: [] },
      { kind: "event", envelope: { thread_id: thread, run_id: "r2", event: { kind: "steered", text: "Use tabs." } } },
    ] as never });
    expect(loaded.messages[thread]!.map(row => [row.goalTurn, row.steered])).toEqual([[24, undefined], [undefined, true]]);
  });
});
