// Review-queue flow tests with mocked backend APIs; dispatched actions
// fold through the real reducer. No DOM, no Tauri imports.
import { describe, expect, it, beforeEach } from "vitest";
import type {
  Automation,
  ProjectInfo,
  ReviewItem,
  ThreadInfo,
} from "../lib/types";
import {
  initialState,
  reducer,
  resetIds,
  type AppAction,
  type AppState,
} from "../state/reducer";
import {
  automationName,
  continueReview,
  dismissReview,
  findThreadProject,
} from "./reviewFlow";

const PROJECT: ProjectInfo = { root: "/repo", name: "repo", is_git: true };

const THREAD: ThreadInfo = {
  id: "t9",
  title: "Nightly run",
  provider: "openai",
  model: "",
  running: false,
  worktree_path: "/tmp/themis-wt/t9",
  branch: "themis/t9",
  base_branch: "main",
  recovered: false,
  skill_ids: ["s1"],
};

const AUTOMATION: Automation = {
  id: "a1",
  name: "nightly",
  project_root: PROJECT.root,
  provider: "openai",
  model: "",
  skill_ids: [],
  interval_mins: 60,
  task: "triage",
  enabled: true,
  last_run_at: null,
  next_run_at: "2026-01-01T00:00:00Z",
  run_count: 1,
};

const REVIEW: ReviewItem = {
  id: "r1",
  automation_id: "a1",
  thread_id: "t9",
  created_at: "2026-01-01T00:00:00Z",
  title: "Nightly triage",
  summary: "3 items",
  status: "pending",
};

function seeded(): AppState {
  let s = reducer(initialState, { type: "project/opened", project: PROJECT });
  s = reducer(s, {
    type: "automation/loaded",
    automations: [AUTOMATION],
  });
  s = reducer(s, { type: "review/loaded", reviews: [REVIEW] });
  return s;
}

beforeEach(() => {
  resetIds();
});

describe("findThreadProject / automationName", () => {
  it("finds the project holding a thread", () => {
    expect(findThreadProject({ [PROJECT.root]: [THREAD] }, "t9")).toBe(
      PROJECT.root,
    );
    expect(findThreadProject({ [PROJECT.root]: [THREAD] }, "missing")).toBeNull();
  });

  it("resolves automation names with an unknown fallback", () => {
    expect(automationName([AUTOMATION], "a1")).toBe("nightly");
    expect(automationName([AUTOMATION], "gone")).toBe("Unknown automation");
  });
});

describe("continueReview flow", () => {
  it("upserts the thread via the automation root and selects it", async () => {
    const base = seeded();
    const actions: AppAction[] = [];
    const result = await continueReview(
      { continueReviewItem: () => Promise.resolve(THREAD) },
      (a) => {
        actions.push(a);
      },
      { threadsByProject: base.threadsByProject, automations: base.automations },
      REVIEW,
    );
    expect(result.ok).toBe(true);
    if (!result.ok) return;
    expect(result.projectRoot).toBe(PROJECT.root);
    const state = actions.reduce(reducer, base);
    expect(state.reviews[0]?.status).toBe("continued");
    expect(
      state.threadsByProject[PROJECT.root]?.find((t) => t.id === "t9"),
    ).toBeDefined();
    expect(state.activeThreadId).toBe("t9");
    expect(state.mainView).toBe("thread");
    expect(state.toasts[0]?.tone).toBe("success");
  });

  it("updates (not duplicates) an already-listed thread", async () => {
    let base = seeded();
    base = reducer(base, {
      type: "thread/created",
      projectRoot: PROJECT.root,
      thread: { ...THREAD, title: "stale" },
    });
    const actions: AppAction[] = [];
    await continueReview(
      { continueReviewItem: () => Promise.resolve(THREAD) },
      (a) => {
        actions.push(a);
      },
      { threadsByProject: base.threadsByProject, automations: base.automations },
      REVIEW,
    );
    const state = actions.reduce(reducer, base);
    const threads = state.threadsByProject[PROJECT.root] ?? [];
    expect(threads.filter((t) => t.id === "t9")).toHaveLength(1);
    expect(threads.find((t) => t.id === "t9")?.title).toBe("Nightly run");
  });

  it("marks continued with a warning when no project is known", async () => {
    const actions: AppAction[] = [];
    const result = await continueReview(
      { continueReviewItem: () => Promise.resolve(THREAD) },
      (a) => {
        actions.push(a);
      },
      { threadsByProject: {}, automations: [] },
      REVIEW,
    );
    expect(result.ok).toBe(true);
    if (!result.ok) return;
    expect(result.projectRoot).toBeNull();
    const state = actions.reduce(reducer, seeded());
    expect(state.reviews[0]?.status).toBe("continued");
    expect(state.toasts[0]?.tone).toBe("warning");
  });

  it("backend failure toasts danger and records nothing", async () => {
    const base = seeded();
    const actions: AppAction[] = [];
    const result = await continueReview(
      { continueReviewItem: () => Promise.reject(new Error("gone")) },
      (a) => {
        actions.push(a);
      },
      { threadsByProject: base.threadsByProject, automations: base.automations },
      REVIEW,
    );
    expect(result).toEqual({ ok: false, error: "gone" });
    const state = actions.reduce(reducer, base);
    expect(state.reviews[0]?.status).toBe("pending");
    expect(state.toasts).toHaveLength(1);
    expect(state.toasts[0]?.tone).toBe("danger");
  });
});

describe("dismissReview flow", () => {
  it("records the backend-returned item + success toast", async () => {
    const base = seeded();
    const actions: AppAction[] = [];
    const dismissed: ReviewItem = { ...REVIEW, status: "dismissed" };
    const result = await dismissReview(
      { dismissReviewItem: () => Promise.resolve(dismissed) },
      (a) => {
        actions.push(a);
      },
      "r1",
    );
    expect(result.ok).toBe(true);
    const state = actions.reduce(reducer, base);
    expect(state.reviews[0]?.status).toBe("dismissed");
    expect(state.toasts[0]?.tone).toBe("success");
  });

  it("backend failure toasts danger and keeps pending", async () => {
    const base = seeded();
    const actions: AppAction[] = [];
    const result = await dismissReview(
      { dismissReviewItem: () => Promise.reject(new Error("nope")) },
      (a) => {
        actions.push(a);
      },
      "r1",
    );
    expect(result).toEqual({ ok: false, error: "nope" });
    const state = actions.reduce(reducer, base);
    expect(state.reviews[0]?.status).toBe("pending");
    expect(state.toasts[0]?.tone).toBe("danger");
  });
});
