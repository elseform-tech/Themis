// Pure reducer tests — no DOM, no Tauri imports (via ./reducer only).
import { describe, expect, it, beforeEach, vi } from "vitest";
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
  provider: "go",
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
    provider: "go",
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
  it("records elapsed time for live runs but not when replaying history", () => {
    const now = vi.spyOn(Date, "now").mockReturnValue(1_000);
    try {
      let s = applyThreadEvent(withThread(), envelope({ kind: "started", task: "work", max_turns: 1 }));
      now.mockReturnValue(13_500);
      s = applyThreadEvent(s, envelope({ kind: "finished", result: "done" }));
      expect(s.runDurations).toEqual({ r1: 12_500 });
    } finally {
      now.mockRestore();
    }

    const restored = reducer(withThread(), {
      type: "thread/history-loaded",
      threadId: THREAD.id,
      history: [
        { kind: "event", envelope: envelope({ kind: "started", task: "work", max_turns: 1 }) },
        { kind: "event", envelope: envelope({ kind: "finished", result: "done" }) },
      ],
    });
    expect(restored.runDurations).toEqual({});
  });

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
  const last = (state: AppState) => { const rows = state.messages[THREAD.id]!; return rows[rows.length - 1]!; };
  it("tracks context budgets and records a checkpoint divider with before and after usage", () => {
    const budget = (used: number) => ({ budget_tokens: 24000, usable_tokens: 10000, used_tokens: used, tool_definition_tokens: 3000, output_reserve_tokens: 3000, safety_margin_tokens: 480, maintenance_at_tokens: 8300, compaction_at_tokens: 9000, compaction_soon: used > 8500, breakdown: { system: 1000, checkpoint: 0, attachments: 0, conversation: 1000, tool_activity: used - 2000 } });
    let s = withThread();
    s = applyThreadEvent(s, envelope({ kind: "started", task: "work", max_turns: 10 }));
    s = applyThreadEvent(s, envelope({ kind: "context_budget", budget: budget(9100) }));
    expect(s.contextBudgets[THREAD.id]?.compaction_soon).toBe(true);
    s = applyThreadEvent(s, envelope({ kind: "context_compacting" }));
    s = applyThreadEvent(s, envelope({ kind: "context_checkpoint", summary: "state" }));
    const divider = last(s);
    expect(divider.checkpoint).toEqual({ before: 91 });
    s = applyThreadEvent(s, envelope({ kind: "context_budget", budget: budget(3100) }));
    expect(last(s).checkpoint).toEqual({ before: 91, after: 31 });
    s = applyThreadEvent(s, envelope({ kind: "context_budget", budget: budget(4000) }));
    expect(last(s).checkpoint).toEqual({ before: 91, after: 31 });
    expect(s.contextBudgets[THREAD.id]?.used_tokens).toBe(4000);
  });

  it("measures the divider from the budget that triggered the checkpoint, not the previous turn", () => {
    const budget = (used: number) => ({ budget_tokens: 24000, usable_tokens: 10000, used_tokens: used, tool_definition_tokens: 3000, output_reserve_tokens: 3000, safety_margin_tokens: 480, maintenance_at_tokens: 8300, compaction_at_tokens: 9000, compaction_soon: false, breakdown: { system: 1000, checkpoint: 0, attachments: 0, conversation: used - 1000, tool_activity: 0 } });
    let s = withThread();
    s = applyThreadEvent(s, envelope({ kind: "started", task: "work", max_turns: 10 }));
    s = applyThreadEvent(s, envelope({ kind: "context_budget", budget: budget(5500) }));
    // The runtime reports the size with this turn's input before it compacts.
    s = applyThreadEvent(s, envelope({ kind: "context_budget", budget: budget(9900) }));
    s = applyThreadEvent(s, envelope({ kind: "context_compacting" }));
    s = applyThreadEvent(s, envelope({ kind: "context_checkpoint", summary: "state" }));
    s = applyThreadEvent(s, envelope({ kind: "context_budget", budget: budget(5700) }));
    expect(last(s).checkpoint).toEqual({ before: 99, after: 57 });
  });
  it("attaches the files a turn changed to that turn's final reply only", () => {
    let s = withThread();
    s = applyThreadEvent(s, envelope({ kind: "started", task: "edit", max_turns: 3 }));
    s = applyThreadEvent(s, envelope({ kind: "files_changed", files: [{ path: "src/app.py", status: "modified" }, { path: "notes.md", status: "added" }] }));
    s = applyThreadEvent(s, envelope({ kind: "finished", result: "Done." }));
    expect(s.messages[THREAD.id]?.slice(-1)[0]?.changedFiles).toEqual([{ path: "src/app.py", status: "modified" }, { path: "notes.md", status: "added" }]);
    s = applyThreadEvent(s, { thread_id: THREAD.id, run_id: "next-run", event: { kind: "started", task: "chat", max_turns: 1 } });
    s = applyThreadEvent(s, { thread_id: THREAD.id, run_id: "next-run", event: { kind: "finished", result: "No edits." } });
    expect(s.messages[THREAD.id]?.slice(-1)[0]?.changedFiles).toBeUndefined();
  });
  it("attaches media to the turn's final reply in order, once each", () => {
    let s = withThread();
    const shot = { path: "/p/.themis/attachments/a/shot.png", name: "shot.png", kind: "image" as const, caption: "Before" };
    const video = { path: "/p/.themis/attachments/b/flow.mp4", name: "flow.mp4", kind: "video" as const };
    s = applyThreadEvent(s, envelope({ kind: "started", task: "show", max_turns: 3 }));
    s = applyThreadEvent(s, envelope({ kind: "media_attached", files: [shot] }));
    s = applyThreadEvent(s, envelope({ kind: "media_attached", files: [shot, video] }));
    s = applyThreadEvent(s, envelope({ kind: "finished", result: "Here they are." }));
    expect(s.messages[THREAD.id]?.slice(-1)[0]?.media).toEqual([shot, video]);
  });
  it("tracks who drives each chat's browser and its recording", () => {
    let s = withThread();
    s = applyThreadEvent(s, { thread_id: THREAD.id, run_id: "", event: { kind: "browser_changed", controller: "agent", recording: { by: "agent", seconds: 3, max_seconds: 30 } } });
    expect(s.browsers[THREAD.id]).toMatchObject({ controller: "agent", recording: { by: "agent", seconds: 3, max_seconds: 30 } });
    s = applyThreadEvent(s, { thread_id: THREAD.id, run_id: "", event: { kind: "browser_changed", controller: "idle" } });
    expect(s.browsers[THREAD.id]?.recording).toBeUndefined();
    expect(s.running[THREAD.id]).toBeFalsy();
  });
  it("shows an older incomplete handoff as a normal final reply", () => {
    let s = withThread();
    s = applyThreadEvent(s, envelope({ kind: "started", task: "work", max_turns: 1 }));
    s = applyThreadEvent(s, envelope({ kind: "incomplete", result: "Task incomplete. Follow-up tasks: verify." }));
    expect(s.running[THREAD.id]).toBe(false);
    expect(s.messages[THREAD.id]?.slice(-1)[0]).toMatchObject({ role: "assistant", final: true });
    expect(s.messages[THREAD.id]?.slice(-1)[0]).toHaveProperty("incomplete", true);
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

  it("keeps an MCP tool's readable label on its message and trace", () => {
    const s = applyThreadEvent(
      withThread(),
      envelope({ kind: "tool_started", tool: "mcp_932fdceefae46ddd0ad259c8_browser_navigate", summary: "{}", label: "playwright · browser_navigate" }),
    );
    expect(s.traces[THREAD.id]?.[0]?.label).toBe("playwright · browser_navigate");
    expect(s.messages[THREAD.id]?.[s.messages[THREAD.id]!.length - 1]?.tool?.label).toBe("playwright · browser_navigate");
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

  it("keeps the partial streamed answer when the user stops the run", () => {
    let s = applyThreadEvent(withThread(), envelope({ kind: "started", task: "t", max_turns: 3 }));
    s = applyThreadEvent(s, envelope({ kind: "assistant_text", text: "Half an " }));
    s = applyThreadEvent(s, envelope({ kind: "assistant_text", text: "answer" }));
    s = applyThreadEvent(s, envelope({ kind: "failed", error: "Stopped by you. Completed actions remain available for review." }));
    const rows = s.messages[THREAD.id] ?? [];
    expect(rows[0]).toMatchObject({ role: "assistant", text: "Half an answer", final: true, interrupted: "stopped" });
    expect(rows[1]?.text).toMatch(/^Stopped by you/);
    expect(s.streams[THREAD.id]).toBe("");
  });

  it("keeps the partial answer on a failure and reloads the same way from history", () => {
    const events = [
      envelope({ kind: "started", task: "t", max_turns: 3 }),
      envelope({ kind: "assistant_text", text: "Partial" }),
      envelope({ kind: "failed", error: "LLM request failed" }),
    ];
    const live = events.reduce((state, e) => applyThreadEvent(state, e), withThread());
    expect(live.messages[THREAD.id]?.[0]).toMatchObject({ text: "Partial", interrupted: "failed" });
    expect(live.toasts).toHaveLength(1);
    const reloaded = events.reduce((state, e) => applyThreadEvent(state, e), withThread());
    expect(reloaded.messages[THREAD.id]?.map(m => m.text)).toEqual(live.messages[THREAD.id]?.map(m => m.text));
  });

  it("does not add an empty answer when nothing streamed", () => {
    const s = applyThreadEvent(withThread(), envelope({ kind: "failed", error: "Stopped by you." }));
    expect(s.messages[THREAD.id]).toHaveLength(1);
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

  it("removes an expired approval and says once that the action was skipped", () => {
    let s = withThread();
    s = reducer(s, { type: "approval/enqueued", request: REQ });
    s = reducer(s, { type: "approval/expired", approvalId: "a1" });
    expect(s.approvals).toHaveLength(0);
    expect(s.toasts.map(toast => toast.message)).toEqual(["Approval expired — the action was skipped"]);
    // A repeat, or an id that is not shown, changes nothing.
    expect(reducer(s, { type: "approval/expired", approvalId: "a1" })).toBe(s);
  });

  it("keeps other pending approvals when one expires", () => {
    let s = withThread();
    s = reducer(s, { type: "approval/enqueued", request: REQ });
    s = reducer(s, { type: "approval/enqueued", request: { ...REQ, approval_id: "a2" } });
    s = reducer(s, { type: "approval/expired", approvalId: "a1" });
    expect(s.approvals.map(approval => approval.approval_id)).toEqual(["a2"]);
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
        theme_palette: "system", font_family: "system",
        theme: "light",
        default_provider: "go",
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
        completion_sound: true,
        completion_haptic: false,
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

  it("registers a project from Automations without leaving the form", () => {
    const s = reducer({ ...initialState, mainView: "automations" }, {
      type: "project/opened",
      project: PROJECT,
      select: false,
    });
    expect(s.projects).toContainEqual(PROJECT);
    expect(s.activeProjectRoot).toBeNull();
    expect(s.mainView).toBe("automations");
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

  it("syncs a background thread without changing selection or duplicating it", () => {
    let s = withThread();
    const background = makeThread("scheduled", "Scheduled check");
    s = reducer(s, { type: "thread/synced", projectRoot: PROJECT.root, thread: background });
    s = reducer(s, { type: "thread/synced", projectRoot: PROJECT.root, thread: { ...background, title: "Updated" } });
    expect(s.threadsByProject[PROJECT.root]?.filter(thread => thread.id === background.id)).toEqual([{ ...background, title: "Updated" }]);
    expect(s.activeThreadId).toBe(THREAD.id);
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

it("recovers stale running flags after a missed terminal event", () => {
  const running = applyThreadEvent(withThread(), envelope({ kind: "started", task: "work", max_turns: 1 }));
  const recovered = reducer(running, { type: "thread/running-loaded", threadId: THREAD.id, running: false });
  expect(recovered.running[THREAD.id]).toBe(false);
  expect(recovered.threadsByProject[PROJECT.root][0].running).toBe(false);
});

describe("thread/changed", () => {
  const info = (over: Partial<typeof THREAD> = {}) => ({ ...THREAD, ...over });
  it("upserts created and updated threads without stealing the selection", () => {
    let s = withThread();
    const selected = s.activeThreadId;
    s = reducer(s, { type: "thread/changed", change: { kind: "created", project_root: PROJECT.root, thread_id: "n1", thread: info({ id: "n1", title: "From CLI" }) } });
    expect(s.threadsByProject[PROJECT.root]?.map(t => t.id)).toEqual([THREAD.id, "n1"]);
    expect(s.activeThreadId).toBe(selected);
    s = reducer(s, { type: "thread/changed", change: { kind: "updated", project_root: PROJECT.root, thread_id: "n1", thread: info({ id: "n1", title: "Renamed", approval_mode: "yolo" }) } });
    const updated = s.threadsByProject[PROJECT.root]?.find(t => t.id === "n1");
    expect(updated).toMatchObject({ title: "Renamed", approval_mode: "yolo" });
    expect(s.threadsByProject[PROJECT.root]).toHaveLength(2);
    // Replaying the same event is harmless.
    expect(reducer(s, { type: "thread/changed", change: { kind: "updated", project_root: PROJECT.root, thread_id: "n1", thread: updated } })).toEqual(s);
  });
  it("lists a new chat once when the server's created event arrives before the create reply", () => {
    let s = withThread();
    const fresh = info({ id: "n2", title: "New thread" });
    s = reducer(s, { type: "thread/changed", change: { kind: "created", project_root: PROJECT.root, thread_id: "n2", thread: fresh } });
    s = reducer(s, { type: "thread/created", projectRoot: PROJECT.root, thread: fresh });
    expect(s.threadsByProject[PROJECT.root]?.map(t => t.id)).toEqual([THREAD.id, "n2"]);
    expect(s.activeThreadId).toBe("n2");
  });
  it("removes a thread and ignores an update without info", () => {
    const s = withThread();
    expect(reducer(s, { type: "thread/changed", change: { kind: "updated", project_root: PROJECT.root, thread_id: "x" } })).toBe(s);
    const removed = reducer(s, { type: "thread/changed", change: { kind: "removed", project_root: PROJECT.root, thread_id: THREAD.id } });
    expect(removed.threadsByProject[PROJECT.root] ?? []).toEqual([]);
  });
});

describe("thread/changed edge cases", () => {
  const info = (id: string) => ({ ...THREAD, id });
  it("ignores projects that are not open and never resurrects a removed thread", () => {
    let s = withThread();
    const elsewhere = reducer(s, { type: "thread/changed", change: { kind: "created", project_root: "/other", thread_id: "x", thread: info("x") } });
    expect(elsewhere.threadsByProject["/other"]).toBeUndefined();
    s = reducer(s, { type: "thread/removed", projectRoot: PROJECT.root, threadId: THREAD.id });
    for (const kind of ["updated", "created"] as const) {
      s = reducer(s, { type: "thread/changed", change: { kind, project_root: PROJECT.root, thread_id: THREAD.id, thread: info(THREAD.id) } });
    }
    expect(s.threadsByProject[PROJECT.root]).toEqual([]);
  });
  it("does not add an update for a thread this window never listed, and dedupes created", () => {
    let s = withThread();
    s = reducer(s, { type: "thread/changed", change: { kind: "updated", project_root: PROJECT.root, thread_id: "ghost", thread: info("ghost") } });
    expect(s.threadsByProject[PROJECT.root]).toHaveLength(1);
    const created = { kind: "created" as const, project_root: PROJECT.root, thread_id: "n", thread: info("n") };
    s = reducer(reducer(s, { type: "thread/changed", change: created }), { type: "thread/changed", change: created });
    expect(s.threadsByProject[PROJECT.root]?.filter(t => t.id === "n")).toHaveLength(1);
  });
  it("clears run state, streams, traces and approvals when a thread is removed", () => {
    let s = applyThreadEvent(withThread(), envelope({ kind: "started", task: "t", max_turns: 2 }));
    s = applyThreadEvent(s, envelope({ kind: "assistant_text", text: "hi" }));
    s = reducer(s, { type: "approval/enqueued", request: { thread_id: THREAD.id, approval_id: "a", tool: "shell", summary: "", risk: "execute" } });
    s = reducer(s, { type: "thread/changed", change: { kind: "removed", project_root: PROJECT.root, thread_id: THREAD.id } });
    expect(s.running[THREAD.id]).toBeUndefined();
    expect(s.streams[THREAD.id]).toBeUndefined();
    expect(s.traces[THREAD.id]).toBeUndefined();
    expect(s.approvals).toEqual([]);
  });
});
