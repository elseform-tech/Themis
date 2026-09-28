// Boot-load orchestration tests: the injected mock API resolves or
// rejects, dispatched actions fold through the real reducer, and the
// resulting state is asserted. No DOM, no Tauri imports.
import { describe, expect, it, beforeEach } from "vitest";
import type {
  Automation,
  HistoryItem,
  PersistedMessage,
  ProjectInfo,
  ReviewItem,
  Skill,
  ThreadInfo,
} from "../lib/types";
import {
  loadPhase4Lists,
  restoreRecentProjects,
  type Phase4ListsApi,
  type RecentProjectApi,
} from "./boot";
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
  provider: "go",
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

const PROJECT: ProjectInfo = { root: "/repo", name: "repo", is_git: false };
const THREAD: ThreadInfo = {
  id: "t1",
  title: "First thread",
  provider: "go",
  model: "",
  running: false,
  worktree_path: null,
  branch: null,
  base_branch: null,
  recovered: false,
  skill_ids: [],
};
const HISTORY: HistoryItem[] = [{ kind: "user", run_id: "r1", text: "hello" }];
const LEGACY_MESSAGE: PersistedMessage = { id: "m1", role: "user", text: "legacy" };

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

describe("restoreRecentProjects", () => {
  it("restores history, imports legacy messages, and reapplies saved selection", async () => {
    const actions: AppAction[] = [];
    const imported: PersistedMessage[][] = [];
    const warnings: string[] = [];
    let historyReads = 0;
    const api: RecentProjectApi = {
      openProject: () => Promise.resolve(PROJECT),
      listThreads: () => Promise.resolve([THREAD]),
      getThreadHistory: () => {
        historyReads += 1;
        return Promise.resolve(historyReads === 1 ? [] : HISTORY);
      },
      importLegacyHistory: (_threadId, messages) => {
        imported.push(messages);
        return Promise.resolve();
      },
    };

    const completed = await restoreRecentProjects([PROJECT.root], {
      api,
      dispatch: (action) => { actions.push(action); },
      isCancelled: () => false,
      legacyMessages: { [THREAD.id]: [LEGACY_MESSAGE] },
      savedSelection: { root: PROJECT.root, threadId: THREAD.id },
      warn: (message) => { warnings.push(message); },
    });

    expect(completed).toBe(true);
    expect(imported).toEqual([[LEGACY_MESSAGE]]);
    expect(actions).toContainEqual({ type: "thread/history-loaded", threadId: THREAD.id, history: HISTORY });
    expect(actions).toContainEqual({ type: "project/selected", root: PROJECT.root });
    expect(actions).toContainEqual({ type: "thread/selected", projectRoot: PROJECT.root, threadId: THREAD.id });
    expect(warnings).toEqual([]);
  });

  it("warns and continues when one recent project cannot be reopened", async () => {
    const actions: AppAction[] = [];
    const warnings: string[] = [];
    const api: RecentProjectApi = {
      openProject: (root) => root === "/missing"
        ? Promise.reject(new Error("folder missing"))
        : Promise.resolve(PROJECT),
      listThreads: () => Promise.resolve([]),
      getThreadHistory: () => Promise.resolve([]),
      importLegacyHistory: () => Promise.resolve(),
    };

    await restoreRecentProjects(["/missing", PROJECT.root], {
      api,
      dispatch: (action) => { actions.push(action); },
      isCancelled: () => false,
      legacyMessages: {},
      savedSelection: null,
      warn: (message) => { warnings.push(message); },
    });

    expect(warnings).toEqual(["Could not reopen /missing: folder missing"]);
    expect(actions).toContainEqual({ type: "project/opened", project: PROJECT });
  });
});
