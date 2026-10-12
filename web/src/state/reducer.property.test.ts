// Property tests: random event streams and prompt text against invariants.
import { describe, expect, it } from "vitest";
import fc from "fast-check";
import type { ProjectInfo, ThreadEventEnvelope, ThreadInfo } from "../lib/types";
import { pluginToken, promptParts, skillToken } from "../lib/prompt";
import { applyThreadEvent, initialState, reducer, type AppState } from "./reducer";

const PROJECT: ProjectInfo = { root: "/repo", name: "repo", is_git: true };
const THREAD: ThreadInfo = { id: "t1", title: "Thread", provider: "go", model: "m", running: false, worktree_path: null, branch: null, base_branch: null, recovered: false, skill_ids: [] };

function withThread(): AppState {
  const opened = reducer(initialState, { type: "project/opened", project: PROJECT });
  return reducer(opened, { type: "thread/created", projectRoot: PROJECT.root, thread: THREAD });
}

const changedFile = fc.record({ path: fc.string({ minLength: 1 }), status: fc.constantFrom("added" as const, "modified" as const, "deleted" as const, "renamed" as const) });
const event: fc.Arbitrary<ThreadEventEnvelope["event"]> = fc.oneof(
  fc.record({ kind: fc.constant("started" as const), task: fc.string(), max_turns: fc.nat(50) }),
  fc.record({ kind: fc.constant("assistant_text" as const), text: fc.string() }),
  fc.record({ kind: fc.constant("tool_started" as const), tool: fc.constantFrom("shell", "read_file", "write_file"), summary: fc.string() }),
  fc.record({ kind: fc.constant("tool_finished" as const), tool: fc.constantFrom("shell", "read_file", "write_file"), ok: fc.boolean(), output: fc.string() }),
  fc.record({ kind: fc.constant("files_changed" as const), files: fc.array(changedFile, { maxLength: 4 }) }),
  fc.record({ kind: fc.constant("context_compacting" as const) }),
  fc.record({ kind: fc.constant("context_checkpoint" as const), summary: fc.string() }),
  fc.record({ kind: fc.constantFrom("finished" as const, "incomplete" as const), result: fc.string() }),
  fc.record({ kind: fc.constant("failed" as const), error: fc.string() }),
);
const stream = fc.array(fc.record({ run: fc.constantFrom("r1", "r2", "r3"), event }), { maxLength: 40 });

describe("thread event stream properties", () => {
  it("never throws and tracks running from the last start or terminal event", () => {
    fc.assert(fc.property(stream, steps => {
      let state = withThread();
      let expectRunning: boolean | undefined;
      for (const { run, event } of steps) {
        state = applyThreadEvent(state, { thread_id: THREAD.id, run_id: run, event }, false);
        if (event.kind === "started") expectRunning = true;
        if (["finished", "incomplete", "failed"].includes(event.kind)) expectRunning = false;
      }
      if (expectRunning !== undefined) expect(state.running[THREAD.id] ?? false).toBe(expectRunning);
    }));
  });

  it("gives each terminal message the files its run reported before it ended", () => {
    fc.assert(fc.property(stream, steps => {
      let state = withThread();
      const reported = new Map<string, unknown>();
      const expected: unknown[] = [];
      for (const { run, event } of steps) {
        state = applyThreadEvent(state, { thread_id: THREAD.id, run_id: run, event }, false);
        if (event.kind === "files_changed") reported.set(run, event.files);
        if (["finished", "incomplete", "failed"].includes(event.kind)) expected.push(reported.get(run));
      }
      const terminal = (state.messages[THREAD.id] ?? []).filter(message => message.final || message.text.startsWith("Run failed:") || message.role === "system" && !message.checkpoint);
      expect(terminal.map(message => message.changedFiles)).toEqual(expected);
    }));
  });
});

describe("prompt token properties", () => {
  const piece = fc.oneof(
    fc.string().map(text => text.replace(/\[\[/g, "[")),
    fc.stringMatching(/^[a-z0-9-]{1,12}$/).map(skillToken),
    fc.stringMatching(/^[a-z0-9-]{1,12}$/).map(pluginToken),
  );

  it("splits any prompt into parts that rebuild it exactly", () => {
    fc.assert(fc.property(fc.array(piece, { maxLength: 8 }).map(parts => parts.join("")), value => {
      const parts = promptParts(value);
      const rebuilt = parts.map(part => "text" in part ? part.text : "skill" in part ? skillToken(part.skill) : pluginToken(part.plugin)).join("");
      expect(rebuilt).toBe(value);
      parts.forEach((part, index) => {
        if ("text" in part) {
          expect(part.text).not.toBe("");
          expect("text" in (parts[index + 1] ?? {})).toBe(false);
        }
      });
    }));
  });

  it("never throws on arbitrary text", () => {
    fc.assert(fc.property(fc.string({ unit: "binary" }), value => { promptParts(value); }));
  });
});
