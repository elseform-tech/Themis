import { describe, expect, it } from "vitest";
import { goalTurnNumber, initialState, reducer } from "./reducer";

const thread = "t1";
const envelope = (event: object, run_id = "r1") => ({ type: "thread/event" as const, envelope: { thread_id: thread, run_id, event: event as never } });

describe("queue and goal events", () => {
  it("mutes a goal turn's replies once the check rejects them, live and from history", () => {
    const turn = "[Goal — runtime, not from the user] Turn 2. Working toward: speed up p95";
    const steps = [
      { kind: "user_message", text: turn },
      { kind: "assistant_text", text: "All requirements are met." },
      { kind: "tool_started", tool: "shell", summary: "ls" },
      { kind: "tool_finished", tool: "shell", ok: true, output: "" },
      { kind: "finished", result: "Everything looks good.", model: "m" },
    ];
    let state = initialState;
    for (const event of steps) state = reducer(state, envelope(event as never));
    const claims = () => (state.messages[thread] ?? []).filter(row => row.role === "assistant" && !row.tool);
    expect(claims().map(row => row.goalRejected)).toEqual([undefined, undefined]);
    state = reducer(state, envelope({ kind: "goal_note", outcome: "status", reason: "I'm checking that against the goal now." }));
    expect(claims().map(row => row.goalRejected)).toEqual([undefined, undefined]);
    state = reducer(state, envelope({ kind: "goal_note", outcome: "not_yet", reason: "That isn't proven yet." }));
    expect(claims().map(row => row.goalRejected)).toEqual([true, true]);
    state = reducer(state, envelope({ kind: "user_message", text: turn.replace("Turn 2", "Turn 3") }));
    state = reducer(state, envelope({ kind: "finished", result: "Fixed it.", model: "m" }));
    expect(claims().map(row => row.goalRejected)).toEqual([true, true, undefined]);
    const loaded = reducer(initialState, { type: "thread/history-loaded", threadId: thread, history: [
      { kind: "user", run_id: "r2", text: turn, attachments: [] },
      { kind: "event", envelope: { thread_id: thread, run_id: "r2", event: { kind: "finished", result: "Everything looks good.", model: "m" } } },
      { kind: "event", envelope: { thread_id: thread, run_id: "", event: { kind: "goal_note", outcome: "not_yet", reason: "That isn't proven yet." } } },
    ] } as never);
    expect((loaded.messages[thread] ?? []).find(row => row.role === "assistant")?.goalRejected).toBe(true);
  });

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
    expect((state.messages[thread] ?? []).slice(-1)[0]).toMatchObject({ text: "p95 is 238 ms.", goalStatus: true });
    const loaded = reducer(initialState, { type: "thread/history-loaded", threadId: thread, history: [
      { kind: "user", run_id: "r2", text: turn, attachments: [] },
      { kind: "event", envelope: { thread_id: thread, run_id: "r2", event: { kind: "steered", text: "Use tabs." } } },
    ] as never });
    expect(loaded.messages[thread]!.map(row => [row.goalTurn, row.steered])).toEqual([[24, undefined], [undefined, true]]);
  });
});

describe("a steered message on screen", () => {
  const steers = (state: ReturnType<typeof reducer>) => (state.messages[thread] ?? []).filter(row => row.role === "user").map(row => row.text);

  it("appears when it is steered and not again when the run reads it", () => {
    let state = reducer(initialState, envelope({ kind: "started", task: "x", max_turns: 5 }));
    const before = steers(state).length;
    state = reducer(state, envelope({ kind: "steer_shown", text: "Remove the goal" }));
    expect(steers(state).slice(before)).toEqual(["Remove the goal"]);
    expect(state.messages[thread]!.slice(-1)[0]).toMatchObject({ steered: true, runId: "r1" });
    state = reducer(state, envelope({ kind: "steered", text: "Remove the goal" }));
    expect(steers(state).slice(before)).toEqual(["Remove the goal"]);
  });

  it("leaves text still streaming alone until the run reads the message", () => {
    let state = reducer(initialState, envelope({ kind: "assistant_text", text: "Half a sen" }));
    state = reducer(state, envelope({ kind: "steer_shown", text: "Use tabs" }));
    expect(state.streams[thread]).toBe("Half a sen");
    state = reducer(state, envelope({ kind: "assistant_text", text: "tence." }));
    state = reducer(state, envelope({ kind: "finished", result: "Half a sentence." }));
    expect(state.messages[thread]!.map(row => row.text)).toEqual(["Use tabs", "Half a sentence."]);
  });

  it("pairs two identical steers one by one", () => {
    let state = reducer(initialState, envelope({ kind: "steer_shown", text: "Again" }));
    state = reducer(state, envelope({ kind: "steered", text: "Again" }));
    state = reducer(state, envelope({ kind: "steer_shown", text: "Again" }));
    state = reducer(state, envelope({ kind: "steered", text: "Again" }));
    expect(steers(state)).toEqual(["Again", "Again"]);
    // A read message that was never shown early (an old chat's history) still appears.
    state = reducer(state, envelope({ kind: "steered", text: "Again" }));
    expect(steers(state)).toEqual(["Again", "Again", "Again"]);
  });

  it("leaves the chat when the run never read it, so it shows once as the next message", () => {
    let state = reducer(initialState, envelope({ kind: "steer_shown", text: "Use tabs" }));
    state = reducer(state, envelope({ kind: "steer_returned", text: "Use tabs" }));
    expect(steers(state)).toEqual([]);
    state = reducer(state, envelope({ kind: "user_message", text: "Use tabs", attachments: [] }, "r2"));
    expect(steers(state)).toEqual(["Use tabs"]);
    // A return only takes back a message still unread.
    state = reducer(state, envelope({ kind: "steer_shown", text: "Read" }));
    state = reducer(state, envelope({ kind: "steered", text: "Read" }));
    state = reducer(state, envelope({ kind: "steer_returned", text: "Read" }));
    expect(steers(state)).toEqual(["Use tabs", "Read"]);
  });

  it("reloads from history in the order it was seen, once", () => {
    const event = (kind: string, more: object = {}) => ({ kind: "event", envelope: { thread_id: thread, run_id: "r1", event: { kind, ...more } } });
    const loaded = reducer(initialState, { type: "thread/history-loaded", threadId: thread, history: [
      { kind: "user", run_id: "r1", text: "Make a dir", attachments: [] },
      event("tool_started", { tool: "create_dir", summary: "create dir" }),
      event("steer_shown", { text: "Remove the goal" }),
      event("approval_decided", { tool: "create_dir", decision: "deny" }),
      event("tool_finished", { tool: "create_dir", ok: false }),
      event("steered", { text: "Remove the goal" }),
      event("finished", { result: "Removed." }),
    ] as never });
    expect(loaded.messages[thread]!.map(row => row.text)).toEqual(["Make a dir", "create dir", "Remove the goal", "Removed."]);
  });

  it("takes an approval box away without the expired warning", () => {
    const request = { thread_id: thread, approval_id: "a1", tool: "create_dir", summary: "s", risk: "write", grant_scope: "", grantable: true };
    let state = reducer(initialState, { type: "approval/enqueued", request: request as never });
    state = reducer(state, { type: "approval/expired", approvalId: "a1", steered: true } as never);
    expect(state.approvals).toEqual([]);
    expect(state.toasts).toEqual([]);
  });
});
