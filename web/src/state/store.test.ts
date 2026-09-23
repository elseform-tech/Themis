// Pure reducer tests — no DOM, no Tauri imports (via ./reducer only).
import { describe, expect, it, beforeEach } from "vitest";
import type {
  ApprovalRequest,
  ProjectInfo,
  ThreadEventEnvelope,
  ThreadInfo,
} from "../lib/types";
import {
  applyThreadEvent,
  concurrencyToast,
  DEFAULT_SETTINGS,
  initialState,
  isConcurrencyLimitError,
  newId,
  reducer,
  resetIds,
  trackRecentRoot,
  type AppState,
} from "./reducer";

const PROJECT: ProjectInfo = { root: "/repo", name: "repo", is_git: true };
const THREAD: ThreadInfo = {
  id: "t1",
  title: "Thread 1",
  provider: "openai",
  model: "gpt",
  running: false,
  worktree_path: "/tmp/themis-wt/t1",
  branch: "themis/t1",
  base_branch: "main",
  recovered: false,
  skill_ids: [],
};

function makeThread(id: string, title: string): ThreadInfo {
  return {
    id,
    title,
    provider: "openai",
    model: "",
    running: false,
    worktree_path: `/tmp/themis-wt/${id}`,
    branch: `themis/${id}`,
    base_branch: "main",
    recovered: false,
    skill_ids: [],
  };
}

function withThread(): AppState {
  let s = reducer(initialState, { type: "project/opened", project: PROJECT });
  s = reducer(s, {
    type: "thread/created",
    projectRoot: PROJECT.root,
    thread: THREAD,
  });
  return s;
}

function envelope(
  event: ThreadEventEnvelope["event"],
  threadId = THREAD.id,
): ThreadEventEnvelope {
  return { thread_id: threadId, run_id: "r1", event };
}

beforeEach(() => {
  resetIds();
});

describe("thread lifecycle events", () => {
  it("shows compaction only while a checkpoint is being written", () => {
    let s = withThread();
    s = applyThreadEvent(s, envelope({ kind: "started", task: "work", max_turns: 10 }));
    s = applyThreadEvent(s, envelope({ kind: "context_compacting" }));
    expect(s.compacting[THREAD.id]).toBe(true);
    s = applyThreadEvent(s, envelope({ kind: "context_checkpoint", summary: "done" }));
    expect(s.compacting[THREAD.id]).toBe(false);
    s = applyThreadEvent(s, envelope({ kind: "context_compacting" }));
    s = applyThreadEvent(s, envelope({ kind: "failed", error: "summary failed" }));
    expect(s.compacting[THREAD.id]).toBe(false);
  });
  it("shows a capped request as incomplete without an error toast", () => {
    let s = withThread();
    s = applyThreadEvent(s, envelope({ kind: "started", task: "work", max_turns: 1 }));
    s = applyThreadEvent(s, envelope({ kind: "incomplete", result: "Task incomplete. Follow-up tasks: verify." }));
    expect(s.running[THREAD.id]).toBe(false);
    expect(s.messages[THREAD.id]?.slice(-1)[0]).toMatchObject({ role: "assistant", final: true, incomplete: true });
    expect(s.toasts).toHaveLength(0);
  });
  it("restores persisted conversation and marks an unfinished run interrupted", () => {
    let s = withThread();
    s = reducer(s, { type: "thread/history-loaded", threadId: THREAD.id, history: [
      { kind: "user", run_id: "r1", text: "Remember ORBIT-17" },
      { kind: "event", envelope: envelope({ kind: "started", task: "Remember ORBIT-17", max_turns: 5 }) },
      { kind: "event", envelope: envelope({ kind: "tool_started", tool: "read_file", summary: "read a file" }) },
    ] });
    expect(s.messages[THREAD.id]?.[0]?.text).toBe("Remember ORBIT-17");
    expect(s.messages[THREAD.id]?.slice(-1)[0]?.text).toContain("interrupted");
    expect(s.running[THREAD.id]).toBe(false);
  });
  it("started marks running and resets the trace", () => {
    let s = withThread();
    s = reducer(s, {
      type: "thread/event",
      envelope: envelope({ kind: "tool_started", tool: "read", summary: "old run" }),
    });
    expect(s.messages[THREAD.id]?.[0]?.tool?.name).toBe("read");
    expect(s.traces[THREAD.id]).toHaveLength(1);
    s = applyThreadEvent(
      s,
      envelope({ kind: "started", task: "do it", max_turns: 5 }),
    );
    expect(s.running[THREAD.id]).toBe(true);
    expect(s.traces[THREAD.id]).toEqual([]);
    expect(s.streams[THREAD.id]).toBe("");
    expect(
      s.threadsByProject[PROJECT.root]?.find((t) => t.id === THREAD.id)?.running,
    ).toBe(true);
  });

  it("assistant_text appends to the stream buffer", () => {
    let s = withThread();
    s = applyThreadEvent(s, envelope({ kind: "assistant_text", text: "hel" }));
    s = applyThreadEvent(s, envelope({ kind: "assistant_text", text: "lo" }));
    expect(s.streams[THREAD.id]).toBe("hello");
    expect(s.messages[THREAD.id] ?? []).toHaveLength(0);
  });

  it("tool_started/finished produce trace entries", () => {
    let s = withThread();
    s = applyThreadEvent(
      s,
      envelope({ kind: "tool_started", tool: "write_file", summary: "writing" }),
    );
    expect(s.traces[THREAD.id]).toHaveLength(1);
    expect(s.traces[THREAD.id]?.[0]?.ok).toBeUndefined();
    s = applyThreadEvent(s, envelope({ kind: "approval_decided", tool: "write_file", decision: "once" }));
    s = applyThreadEvent(
      s,
      envelope({ kind: "tool_finished", tool: "write_file", ok: true }),
    );
    expect(s.traces[THREAD.id]).toHaveLength(2);
    expect(s.traces[THREAD.id]?.[0]).toMatchObject({
      tool: "write_file",
      ok: true,
    });
  });

  it("keeps tools between the user message and final reply across runs", () => {
    let s = withThread();
    s = applyThreadEvent(s, envelope({ kind: "assistant_text", text: "I will read the file." }));
    s = applyThreadEvent(s, envelope({ kind: "tool_started", tool: "read_file", summary: "check.txt" }));
    s = applyThreadEvent(s, envelope({ kind: "tool_finished", tool: "read_file", ok: true }));
    s = applyThreadEvent(s, envelope({ kind: "finished", result: "Verified." }));
    expect(s.messages[THREAD.id]?.map(row => row.tool?.name ?? row.text)).toEqual(["I will read the file.", "read_file", "Verified."]);
    expect(s.messages[THREAD.id]?.[1]?.tool?.ok).toBe(true);
    s = applyThreadEvent(s, envelope({ kind: "started", task: "Next", max_turns: 20 }));
    expect(s.messages[THREAD.id]).toHaveLength(3);
  });

  it("approval_decided records a trace note", () => {
    let s = withThread();
    s = applyThreadEvent(
      s,
      envelope({ kind: "approval_decided", tool: "exec", decision: "once" }),
    );
    expect(s.traces[THREAD.id]).toHaveLength(1);
    expect(s.traces[THREAD.id]?.[0]?.summary).toContain("once");
  });

  it("finished appends an assistant message and clears running", () => {
    let s = withThread();
    s = applyThreadEvent(
      s,
      envelope({ kind: "started", task: "t", max_turns: 1 }),
    );
    s = applyThreadEvent(s, envelope({ kind: "assistant_text", text: "draft" }));
    s = applyThreadEvent(s, envelope({ kind: "finished", result: "done!" }));
    expect(s.running[THREAD.id]).toBe(false);
    expect(s.streams[THREAD.id]).toBe("");
    const msgs = s.messages[THREAD.id] ?? [];
    expect(msgs).toHaveLength(1);
    expect(msgs[0]).toMatchObject({
      role: "assistant",
      text: "done!",
      runId: "r1",
    });
  });

  it("shows user cancellation quietly and releases the thread", () => {
    const s = applyThreadEvent(withThread(), envelope({ kind: "failed", error: "Stopped by you. Completed actions remain available for review." }));
    expect(s.running[THREAD.id]).toBe(false);
    expect(s.toasts).toHaveLength(0);
    expect(s.messages[THREAD.id]?.[0]?.text).toMatch(/^Stopped by you/);
  });

  it("failed appends a system error, toasts, and clears running", () => {
    let s = withThread();
    s = applyThreadEvent(
      s,
      envelope({ kind: "started", task: "t", max_turns: 1 }),
    );
    s = applyThreadEvent(s, envelope({ kind: "failed", error: "boom" }));
    expect(s.running[THREAD.id]).toBe(false);
    const msgs = s.messages[THREAD.id] ?? [];
    expect(msgs).toHaveLength(1);
    expect(msgs[0]?.role).toBe("system");
    expect(msgs[0]?.text).toContain("boom");
    expect(s.toasts).toHaveLength(1);
    expect(s.toasts[0]?.tone).toBe("danger");
  });
});

describe("approval queue", () => {
  const REQ: ApprovalRequest = {
    thread_id: THREAD.id,
    approval_id: "a1",
    tool: "exec",
    summary: "run tests",
    risk: "execute",
  };

  it("enqueues and dequeues requests", () => {
    let s = withThread();
    s = reducer(s, { type: "approval/enqueued", request: REQ });
    expect(s.approvals).toHaveLength(1);
    s = reducer(s, { type: "approval/dequeued", approvalId: "a1" });
    expect(s.approvals).toHaveLength(0);
  });

  it("ignores duplicate approval ids", () => {
    let s = withThread();
    s = reducer(s, { type: "approval/enqueued", request: REQ });
    s = reducer(s, { type: "approval/enqueued", request: REQ });
    expect(s.approvals).toHaveLength(1);
  });
});

describe("messages", () => {
  it("appends user messages per thread", () => {
    let s = withThread();
    s = reducer(s, {
      type: "message/append",
      threadId: THREAD.id,
      message: { id: newId("msg"), role: "user", text: "hi" },
    });
    expect(s.messages[THREAD.id]).toHaveLength(1);
    expect(s.messages["other"]).toBeUndefined();
  });
});

describe("settings", () => {
  it("patches settings and marks them loaded", () => {
    const s = reducer(withThread(), {
      type: "settings/patched",
      settings: {
        projects_directory: "/tmp/projects", text_size: 14, sidebar_hover: true,
        theme: "light",
        default_provider: "anthropic",
        default_model: "claude",
      max_turns: 20,
      max_total_turns: 200,
      context_token_budget: 16000,
      context_messages: 20,
      approval_timeout_seconds: 300,
      confirm_reads: false,
        recent_roots: ["/repo"],
        concurrency_limit: 4,
        automations_enabled: true,
        onboarded: true,
      },
    });
    expect(s.settingsLoaded).toBe(true);
    expect(s.settings.theme).toBe("light");
    expect(s.settings.max_turns).toBe(20);
  });
});

describe("projects and threads", () => {
  it("opening a project activates it; re-open updates it", () => {
    let s = reducer(initialState, { type: "project/opened", project: PROJECT });
    expect(s.activeProjectRoot).toBe(PROJECT.root);
    s = reducer(s, {
      type: "project/opened",
      project: { ...PROJECT, is_git: false },
    });
    expect(s.projects).toHaveLength(1);
    expect(s.projects[0]?.is_git).toBe(false);
  });

  it("selecting a project falls back to its first thread", () => {
    const other: ProjectInfo = { root: "/b", name: "b", is_git: true };
    let s = withThread();
    s = reducer(s, { type: "project/opened", project: other });
    expect(s.activeThreadId).toBe(THREAD.id); // opened keeps selection…
    s = reducer(s, { type: "project/selected", root: other.root });
    expect(s.activeThreadId).toBeNull();
  });
});

describe("relaunch restore", () => {
  const OTHER: ProjectInfo = { root: "/b", name: "b", is_git: true };

  it("settings/loaded carries recent_roots + concurrency_limit", () => {
    const s = reducer(initialState, {
      type: "settings/loaded",
      settings: {
        ...DEFAULT_SETTINGS,
        recent_roots: [PROJECT.root, OTHER.root],
        concurrency_limit: 3,
      },
    });
    expect(s.settingsLoaded).toBe(true);
    expect(s.settings.recent_roots).toEqual([PROJECT.root, OTHER.root]);
    expect(s.settings.concurrency_limit).toBe(3);
  });

  it("threads/restored selects the first project/thread", () => {
    const t2 = makeThread("t2", "Thread 2");
    const s = reducer(initialState, {
      type: "threads/restored",
      projects: [PROJECT, OTHER],
      threadsByProject: {
        [PROJECT.root]: [THREAD, t2],
        [OTHER.root]: [],
      },
    });
    expect(s.projects).toHaveLength(2);
    expect(s.activeProjectRoot).toBe(PROJECT.root);
    expect(s.activeThreadId).toBe(THREAD.id);
    expect(s.mainView).toBe("thread");
  });

  it("threads/restored with an empty first project selects no thread", () => {
    const s = reducer(initialState, {
      type: "threads/restored",
      projects: [OTHER],
      threadsByProject: { [OTHER.root]: [] },
    });
    expect(s.activeProjectRoot).toBe(OTHER.root);
    expect(s.activeThreadId).toBeNull();
  });

  it("thread/listed replaces threads and keeps a surviving selection", () => {
    let s = withThread();
    const t2 = makeThread("t2", "Thread 2");
    s = reducer(s, {
      type: "thread/listed",
      projectRoot: PROJECT.root,
      threads: [THREAD, t2],
    });
    expect(s.threadsByProject[PROJECT.root]).toHaveLength(2);
    expect(s.activeThreadId).toBe(THREAD.id);
    // Re-list without the active thread falls back to the first listed.
    s = reducer(s, {
      type: "thread/listed",
      projectRoot: PROJECT.root,
      threads: [t2],
    });
    expect(s.activeThreadId).toBe("t2");
  });
});

describe("thread discard", () => {
  function threeThreads(): AppState {
    let s = reducer(initialState, { type: "project/opened", project: PROJECT });
    for (const t of [makeThread("t1", "one"), makeThread("t2", "two"), makeThread("t3", "three")]) {
      s = reducer(s, { type: "thread/created", projectRoot: PROJECT.root, thread: t });
    }
    return reducer(s, {
      type: "thread/selected",
      projectRoot: PROJECT.root,
      threadId: "t2",
    });
  }

  it("removes the thread and selects the neighbor", () => {
    let s = threeThreads();
    s = reducer(s, {
      type: "thread/removed",
      projectRoot: PROJECT.root,
      threadId: "t2",
    });
    expect((s.threadsByProject[PROJECT.root] ?? []).map((t) => t.id)).toEqual([
      "t1",
      "t3",
    ]);
    expect(s.activeThreadId).toBe("t3");
  });

  it("removing the last remaining thread selects nothing", () => {
    let s = withThread();
    s = reducer(s, {
      type: "thread/removed",
      projectRoot: PROJECT.root,
      threadId: THREAD.id,
    });
    expect(s.threadsByProject[PROJECT.root]).toEqual([]);
    expect(s.activeThreadId).toBeNull();
  });

  it("removing a non-active thread keeps the selection", () => {
    let s = threeThreads();
    s = reducer(s, {
      type: "thread/removed",
      projectRoot: PROJECT.root,
      threadId: "t1",
    });
    expect(s.activeThreadId).toBe("t2");
  });

  it("clears the removed thread's merge + send-error state", () => {
    let s = withThread();
    s = reducer(s, {
      type: "thread/merged",
      threadId: THREAD.id,
      result: { applied: false, applied_files: [], conflicts: ["a.ts"] },
    });
    s = reducer(s, {
      type: "thread/send-failed",
      threadId: THREAD.id,
      error: "boom",
    });
    s = reducer(s, {
      type: "thread/removed",
      projectRoot: PROJECT.root,
      threadId: THREAD.id,
    });
    expect(s.merges[THREAD.id]).toBeUndefined();
    expect(s.sendErrors[THREAD.id]).toBeUndefined();
  });
});

describe("thread merge results", () => {
  it("stores applied files on success", () => {
    const s = reducer(withThread(), {
      type: "thread/merged",
      threadId: THREAD.id,
      result: { applied: true, applied_files: ["a.ts", "b.ts"], conflicts: [] },
    });
    expect(s.merges[THREAD.id]).toEqual({
      applied: true,
      files: ["a.ts", "b.ts"],
      conflicts: [],
    });
  });

  it("stores conflict paths when nothing was applied", () => {
    const s = reducer(withThread(), {
      type: "thread/merged",
      threadId: THREAD.id,
      result: { applied: false, applied_files: [], conflicts: ["x.ts", "y/z.ts"] },
    });
    const last = s.merges[THREAD.id];
    expect(last?.applied).toBe(false);
    expect(last?.conflicts).toEqual(["x.ts", "y/z.ts"]);
    expect(last?.files).toEqual([]);
  });

  it("a later merge replaces the earlier result", () => {
    let s = reducer(withThread(), {
      type: "thread/merged",
      threadId: THREAD.id,
      result: { applied: false, applied_files: [], conflicts: ["x.ts"] },
    });
    s = reducer(s, {
      type: "thread/merged",
      threadId: THREAD.id,
      result: { applied: true, applied_files: ["x.ts"], conflicts: [] },
    });
    expect(s.merges[THREAD.id]?.applied).toBe(true);
  });
});

describe("concurrency-limit send failures", () => {
  it("detects limit errors from backend text", () => {
    expect(isConcurrencyLimitError("concurrency limit reached")).toBe(true);
    expect(isConcurrencyLimitError("3 runs already active")).toBe(true);
    expect(isConcurrencyLimitError("too many parallel runs")).toBe(true);
    expect(isConcurrencyLimitError(new Error("Limit exceeded"))).toBe(true);
    expect(isConcurrencyLimitError("connection refused")).toBe(false);
  });

  it("records a clear message + toast naming the limit", () => {
    let s = reducer(initialState, {
      type: "settings/loaded",
      settings: { ...DEFAULT_SETTINGS, concurrency_limit: 3 },
    });
    s = reducer(s, {
      type: "thread/send-failed",
      threadId: "t9",
      error: "concurrency limit reached (3)",
    });
    expect(s.sendErrors["t9"]).toBe(concurrencyToast(3));
    expect(s.sendErrors["t9"]).toContain("3 runs already active");
    expect(s.sendErrors["t9"]).toContain("Settings");
    expect(s.toasts).toHaveLength(1);
    expect(s.toasts[0]?.message).toBe(concurrencyToast(3));
  });

  it("records ordinary send failures verbatim", () => {
    const s = reducer(withThread(), {
      type: "thread/send-failed",
      threadId: THREAD.id,
      error: "connection refused",
    });
    expect(s.sendErrors[THREAD.id]).toBe("Send failed: connection refused");
  });

  it("send-cleared drops the recorded error", () => {
    let s = reducer(withThread(), {
      type: "thread/send-failed",
      threadId: THREAD.id,
      error: "boom",
    });
    expect(s.sendErrors[THREAD.id]).toBeDefined();
    s = reducer(s, { type: "thread/send-cleared", threadId: THREAD.id });
    expect(s.sendErrors[THREAD.id]).toBeUndefined();
  });
});

describe("trackRecentRoot", () => {
  it("prepends, dedupes, and caps at 10", () => {
    expect(trackRecentRoot([], "/a")).toEqual(["/a"]);
    expect(trackRecentRoot(["/a", "/b"], "/b")).toEqual(["/b", "/a"]);
    const nine = Array.from({ length: 9 }, (_, i) => `/${i}`);
    expect(trackRecentRoot(nine, "/new")).toHaveLength(10);
    expect(trackRecentRoot(nine, "/new")[0]).toBe("/new");
    const ten = Array.from({ length: 10 }, (_, i) => `/${i}`);
    const capped = trackRecentRoot(ten, "/new");
    expect(capped).toHaveLength(10);
    expect(capped).not.toContain("/9");
  });
});

describe("extended fixtures", () => {
  it("DEFAULT_SETTINGS carries the new required fields", () => {
    expect(DEFAULT_SETTINGS.recent_roots).toEqual([]);
    expect(DEFAULT_SETTINGS.concurrency_limit).toBeGreaterThanOrEqual(1);
    expect(DEFAULT_SETTINGS.concurrency_limit).toBeLessThanOrEqual(16);
    expect(DEFAULT_SETTINGS.automations_enabled).toBe(true);
  });

  it("extended ThreadInfo fixtures are valid", () => {
    expect(THREAD.worktree_path).toBe("/tmp/themis-wt/t1");
    expect(THREAD.branch).toBe("themis/t1");
    expect(THREAD.base_branch).toBe("main");
    expect(THREAD.recovered).toBe(false);
    const bare: ThreadInfo = {
      id: "x",
      title: "bare",
      provider: "go",
      model: "",
      running: false,
      worktree_path: null,
      branch: null,
      base_branch: null,
      recovered: true,
      skill_ids: ["s1"],
    };
    expect(bare.worktree_path).toBeNull();
    expect(bare.recovered).toBe(true);
  });
});
