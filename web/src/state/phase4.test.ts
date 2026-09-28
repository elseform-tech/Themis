// Pure reducer tests for the Phase 4 slices (skills, automations,
// reviews, thread skills). No DOM, no Tauri imports (via ./reducer only).
import { describe, expect, it, beforeEach } from "vitest";
import type {
  Automation,
  ProjectInfo,
  ReviewItem,
  Skill,
  ThreadInfo,
} from "../lib/types";
import {
  initialState,
  reducer,
  resetIds,
  type AppState,
} from "./reducer";

const PROJECT: ProjectInfo = { root: "/repo", name: "repo", is_git: true };

function makeThread(id: string, skillIds: string[] = []): ThreadInfo {
  return {
    id,
    title: `Thread ${id}`,
    provider: "go",
    model: "",
    running: false,
    worktree_path: `/tmp/themis-wt/${id}`,
    branch: `themis/${id}`,
    base_branch: "main",
    recovered: false,
    skill_ids: skillIds,
  };
}

function makeSkill(id: string, name: string): Skill {
  return {
    id,
    name,
    description: `${name} description`,
    instructions: "do it well",
    allowed_tools: ["read_file"],
    scripts: [],
  };
}

function makeAutomation(id: string, name: string): Automation {
  return {
    id,
    name,
    project_root: PROJECT.root,
    provider: "go",
    model: "",
    skill_ids: [],
    interval_mins: 60,
    task: "triage",
    enabled: true,
    last_run_at: null,
    next_run_at: "2026-01-01T00:00:00Z",
    run_count: 0,
  };
}

function makeReview(
  id: string,
  status: ReviewItem["status"] = "pending",
): ReviewItem {
  return {
    id,
    automation_id: "a1",
    thread_id: "t1",
    created_at: "2026-01-01T00:00:00Z",
    title: `Review ${id}`,
    summary: "summary",
    status,
  };
}

function withThreadAndLists(): AppState {
  let s = reducer(initialState, { type: "project/opened", project: PROJECT });
  s = reducer(s, {
    type: "thread/created",
    projectRoot: PROJECT.root,
    thread: makeThread("t1"),
  });
  return s;
}

beforeEach(() => {
  resetIds();
});

describe("skills slice", () => {
  it("starts empty", () => {
    expect(initialState.skills).toEqual([]);
  });

  it("loaded replaces the list", () => {
    const s = reducer(withThreadAndLists(), {
      type: "skill/loaded",
      skills: [makeSkill("s1", "one"), makeSkill("s2", "two")],
    });
    expect(s.skills.map((x) => x.id)).toEqual(["s1", "s2"]);
  });

  it("added appends, updated replaces, removed drops", () => {
    let s = reducer(withThreadAndLists(), {
      type: "skill/added",
      skill: makeSkill("s1", "one"),
    });
    expect(s.skills).toHaveLength(1);
    s = reducer(s, {
      type: "skill/updated",
      skill: makeSkill("s1", "renamed"),
    });
    expect(s.skills[0]?.name).toBe("renamed");
    s = reducer(s, { type: "skill/removed", skillId: "s1" });
    expect(s.skills).toEqual([]);
  });

  it("added with a duplicate id replaces instead of duplicating", () => {
    let s = reducer(withThreadAndLists(), {
      type: "skill/added",
      skill: makeSkill("s1", "one"),
    });
    s = reducer(s, {
      type: "skill/added",
      skill: makeSkill("s1", "one-again"),
    });
    expect(s.skills).toHaveLength(1);
    expect(s.skills[0]?.name).toBe("one-again");
  });
});

describe("automations slice", () => {
  it("starts empty", () => {
    expect(initialState.automations).toEqual([]);
  });

  it("loaded replaces the list", () => {
    const s = reducer(withThreadAndLists(), {
      type: "automation/loaded",
      automations: [makeAutomation("a1", "nightly")],
    });
    expect(s.automations).toHaveLength(1);
    expect(s.automations[0]?.name).toBe("nightly");
  });

  it("added appends, updated replaces, removed drops", () => {
    let s = reducer(withThreadAndLists(), {
      type: "automation/added",
      automation: makeAutomation("a1", "nightly"),
    });
    expect(s.automations).toHaveLength(1);
    s = reducer(s, {
      type: "automation/updated",
      automation: { ...makeAutomation("a1", "nightly"), enabled: false },
    });
    expect(s.automations[0]?.enabled).toBe(false);
    s = reducer(s, { type: "automation/removed", automationId: "a1" });
    expect(s.automations).toEqual([]);
  });

  it("added with a duplicate id replaces instead of duplicating", () => {
    let s = reducer(withThreadAndLists(), {
      type: "automation/added",
      automation: makeAutomation("a1", "nightly"),
    });
    s = reducer(s, {
      type: "automation/added",
      automation: makeAutomation("a1", "nightly-again"),
    });
    expect(s.automations).toHaveLength(1);
    expect(s.automations[0]?.name).toBe("nightly-again");
  });
});

describe("reviews slice", () => {
  it("starts empty", () => {
    expect(initialState.reviews).toEqual([]);
  });

  it("loaded replaces the list", () => {
    const s = reducer(withThreadAndLists(), {
      type: "review/loaded",
      reviews: [makeReview("r1"), makeReview("r2", "dismissed")],
    });
    expect(s.reviews.map((r) => r.id)).toEqual(["r1", "r2"]);
  });

  it("added appends new items and dedupes re-delivered ones", () => {
    let s = reducer(withThreadAndLists(), {
      type: "review/added",
      review: makeReview("r1"),
    });
    expect(s.reviews).toHaveLength(1);
    // Event re-delivery must not duplicate the row.
    s = reducer(s, { type: "review/added", review: makeReview("r1") });
    expect(s.reviews).toHaveLength(1);
  });

  it("updated transitions pending -> continued", () => {
    let s = reducer(withThreadAndLists(), {
      type: "review/added",
      review: makeReview("r1"),
    });
    s = reducer(s, {
      type: "review/updated",
      review: makeReview("r1", "continued"),
    });
    expect(s.reviews[0]?.status).toBe("continued");
  });

  it("updated transitions pending -> dismissed", () => {
    let s = reducer(withThreadAndLists(), {
      type: "review/added",
      review: makeReview("r1"),
    });
    s = reducer(s, {
      type: "review/updated",
      review: makeReview("r1", "dismissed"),
    });
    expect(s.reviews[0]?.status).toBe("dismissed");
  });

  it("updated leaves other items untouched", () => {
    let s = reducer(withThreadAndLists(), {
      type: "review/loaded",
      reviews: [makeReview("r1"), makeReview("r2")],
    });
    s = reducer(s, {
      type: "review/updated",
      review: makeReview("r2", "dismissed"),
    });
    expect(s.reviews.find((r) => r.id === "r1")?.status).toBe("pending");
    expect(s.reviews.find((r) => r.id === "r2")?.status).toBe("dismissed");
  });
});

describe("thread skills update", () => {
  it("replaces the thread wherever it lives", () => {
    const other: ProjectInfo = { root: "/b", name: "b", is_git: true };
    let s = withThreadAndLists();
    s = reducer(s, { type: "project/opened", project: other });
    s = reducer(s, {
      type: "thread/created",
      projectRoot: other.root,
      thread: makeThread("t2", ["s1"]),
    });
    const updated = makeThread("t2", ["s1", "s2"]);
    s = reducer(s, { type: "thread/skills-updated", thread: updated });
    expect(
      s.threadsByProject[other.root]?.find((t) => t.id === "t2")?.skill_ids,
    ).toEqual(["s1", "s2"]);
    // Untouched thread keeps its (empty) skills.
    expect(
      s.threadsByProject[PROJECT.root]?.find((t) => t.id === "t1")?.skill_ids,
    ).toEqual([]);
  });

  it("is a no-op for unknown thread ids", () => {
    const s = reducer(withThreadAndLists(), {
      type: "thread/skills-updated",
      thread: makeThread("ghost", ["s1"]),
    });
    expect(s.threadsByProject[PROJECT.root]).toHaveLength(1);
    expect(s.threadsByProject[PROJECT.root]?.[0]?.id).toBe("t1");
  });
});

describe("phase 4 navigation", () => {
  it("switches to the skills and automations views", () => {
    let s = reducer(initialState, { type: "ui/view", view: "skills" });
    expect(s.mainView).toBe("skills");
    s = reducer(s, { type: "ui/view", view: "automations" });
    expect(s.mainView).toBe("automations");
  });
});
