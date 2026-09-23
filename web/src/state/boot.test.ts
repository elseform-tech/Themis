// Boot-load orchestration tests: the injected mock API resolves or
// rejects, dispatched actions fold through the real reducer, and the
// resulting state is asserted. No DOM, no Tauri imports.
import { describe, expect, it, beforeEach } from "vitest";
import type { Automation, ReviewItem, Skill } from "../lib/types";
import { loadPhase4Lists, type Phase4ListsApi } from "./boot";
import { initialState, reducer, resetIds, type AppAction } from "./reducer";

const SKILL: Skill = {
  id: "s1",
  name: "review",
  description: "d",
  instructions: "i",
  allowed_tools: [],
  scripts: [],
};

const AUTOMATION: Automation = {
  id: "a1",
  name: "nightly",
  project_root: "/repo",
  provider: "openai",
  model: "",
  skill_ids: ["s1"],
  interval_mins: 60,
  task: "triage",
  enabled: true,
  last_run_at: null,
  next_run_at: "2026-01-01T00:00:00Z",
  run_count: 0,
};

const REVIEW: ReviewItem = {
  id: "r1",
  automation_id: "a1",
  thread_id: "t1",
  created_at: "2026-01-01T00:00:00Z",
  title: "Nightly triage",
  summary: "3 items",
  status: "pending",
};

function resolvingApi(): Phase4ListsApi {
  return {
    listSkills: () => Promise.resolve([SKILL]),
    listAutomations: () => Promise.resolve([AUTOMATION]),
    listReviewItems: () => Promise.resolve([REVIEW]),
  };
}

beforeEach(() => {
  resetIds();
});

describe("loadPhase4Lists", () => {
  it("populates all three slices when the API resolves", async () => {
    const actions: AppAction[] = [];
    await loadPhase4Lists(
      (action) => {
        actions.push(action);
      },
      resolvingApi(),
    );
    const state = actions.reduce(reducer, initialState);
    expect(state.skills).toEqual([SKILL]);
    expect(state.automations).toEqual([AUTOMATION]);
    expect(state.reviews).toEqual([REVIEW]);
    expect(state.toasts).toEqual([]);
  });

  it("a failing list toasts a warning while the others still load", async () => {
    const actions: AppAction[] = [];
    await loadPhase4Lists(
      (action) => {
        actions.push(action);
      },
      {
        ...resolvingApi(),
        listAutomations: () => Promise.reject(new Error("backend down")),
      },
    );
    const state = actions.reduce(reducer, initialState);
    expect(state.skills).toEqual([SKILL]);
    expect(state.automations).toEqual([]);
    expect(state.reviews).toEqual([REVIEW]);
    expect(state.toasts).toHaveLength(1);
    expect(state.toasts[0]?.tone).toBe("warning");
    expect(state.toasts[0]?.message).toContain("automations");
    expect(state.toasts[0]?.message).toContain("backend down");
  });

  it("all three failing leaves empty slices with three warnings", async () => {
    const actions: AppAction[] = [];
    const failing: Phase4ListsApi = {
      listSkills: () => Promise.reject(new Error("nope")),
      listAutomations: () => Promise.reject("strings too"),
      listReviewItems: () => Promise.reject(new Error("nope")),
    };
    await loadPhase4Lists(
      (action) => {
        actions.push(action);
      },
      failing,
    );
    const state = actions.reduce(reducer, initialState);
    expect(state.skills).toEqual([]);
    expect(state.automations).toEqual([]);
    expect(state.reviews).toEqual([]);
    expect(state.toasts).toHaveLength(3);
  });
});
