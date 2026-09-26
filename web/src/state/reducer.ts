// Pure state model for the Themis app shell. No Tauri/DOM imports —
// safe to load in node-based unit tests.
import type {
  ApprovalRequest,
  Automation,
  DiffState,
  MergeResult,
  ProjectInfo,
  ReviewItem,
  SecretStatus,
  Settings,
  Skill,
  ThreadComment,
  ThreadEventEnvelope,
  HistoryItem,
  PersistedMessage,
  ThreadInfo,
} from "../lib/types";
import type { ToastItem, ToastTone } from "../components";

export type MessageRole = PersistedMessage["role"];
export type ChatMessage = PersistedMessage;

export interface ToolTraceEntry {
  id: string;
  tool: string;
  summary: string;
  /** Undefined while the tool is still running. */
  ok?: boolean;
}

export type MainView = "thread" | "settings" | "queue" | "skills" | "automations";

/** Last merge attempt per thread, kept for display (esp. conflicts). */
export interface LastMerge {
  applied: boolean;
  files: string[];
  conflicts: string[];
}

export function toLastMerge(result: MergeResult): LastMerge {
  return {
    applied: result.applied,
    files: [...result.applied_files],
    conflicts: [...result.conflicts],
  };
}

/** True when a send_message failure is the concurrency-limit rejection. */
export function isConcurrencyLimitError(error: unknown): boolean {
  const text =
    typeof error === "string"
      ? error
      : error instanceof Error
        ? error.message
        : (() => {
            try {
              return JSON.stringify(error) ?? "";
            } catch {
              return String(error);
            }
          })();
  return /concurrency|already active|too many|limit/i.test(text);
}

/** Toast text for a rejected send when the run limit is reached. */
export function concurrencyToast(limit: number): string {
  return `${limit} runs already active — wait or raise the limit in Settings`;
}

/** Append a root to recent_roots (dedupe, most-recent-first, cap 10). */
export function trackRecentRoot(roots: string[], root: string): string[] {
  return [root, ...roots.filter((r) => r !== root)].slice(0, 10);
}

export interface AppState {
  projects: ProjectInfo[];
  threadsByProject: Record<string, ThreadInfo[]>;
  activeProjectRoot: string | null;
  activeThreadId: string | null;
  mainView: MainView;
  messages: Record<string, ChatMessage[]>;
  /** Live streamed assistant text per thread (cleared on finished/failed). */
  streams: Record<string, string>;
  traces: Record<string, ToolTraceEntry[]>;
  running: Record<string, boolean>;
  compacting: Record<string, boolean>;
  runStartedAt: Record<string, number>;
  runDurations: Record<string, number>;
  diffs: Record<string, DiffState | null>;
  comments: Record<string, ThreadComment[]>;
  approvals: ApprovalRequest[];
  merges: Record<string, LastMerge>;
  /** Latest send failure per thread (cleared on next successful send). */
  sendErrors: Record<string, string>;
  skills: Skill[];
  automations: Automation[];
  reviews: ReviewItem[];
  settings: Settings;
  settingsLoaded: boolean;
  secretStatus: SecretStatus;
  toasts: ToastItem[];
  diffPanelOpen: boolean;
  paletteOpen: boolean;
  projectDialogOpen: boolean;
}

export type AppAction =
  | { type: "project/opened"; project: ProjectInfo }
  | { type: "project/selected"; root: string }
  | { type: "thread/created"; projectRoot: string; thread: ThreadInfo }
  | { type: "thread/selected"; projectRoot: string; threadId: string }
  | { type: "thread/updated"; projectRoot: string; thread: ThreadInfo }
  | { type: "thread/listed"; projectRoot: string; threads: ThreadInfo[] }
  | { type: "thread/removed"; projectRoot: string; threadId: string }
  | { type: "thread/merged"; threadId: string; result: MergeResult }
  | { type: "thread/send-failed"; threadId: string; error: string }
  | { type: "thread/send-cleared"; threadId: string }
  | {
      type: "threads/restored";
      projects: ProjectInfo[];
      threadsByProject: Record<string, ThreadInfo[]>;
    }
  | { type: "thread/event"; envelope: ThreadEventEnvelope }
  | { type: "thread/history-loaded"; threadId: string; history: HistoryItem[] }
  | { type: "message/append"; threadId: string; message: ChatMessage }
  | { type: "diff/set"; threadId: string; diff: DiffState | null }
  | { type: "comment/added"; threadId: string; comment: ThreadComment }
  | { type: "thread/skills-updated"; thread: ThreadInfo }
  | { type: "skill/loaded"; skills: Skill[] }
  | { type: "skill/added"; skill: Skill }
  | { type: "skill/updated"; skill: Skill }
  | { type: "skill/removed"; skillId: string }
  | { type: "automation/loaded"; automations: Automation[] }
  | { type: "automation/added"; automation: Automation }
  | { type: "automation/updated"; automation: Automation }
  | { type: "automation/removed"; automationId: string }
  | { type: "review/loaded"; reviews: ReviewItem[] }
  | { type: "review/added"; review: ReviewItem }
  | { type: "review/updated"; review: ReviewItem }
  | { type: "approval/enqueued"; request: ApprovalRequest }
  | { type: "approval/dequeued"; approvalId: string }
  | { type: "settings/loaded"; settings: Settings }
  | { type: "settings/patched"; settings: Settings }
  | { type: "secrets/loaded"; status: SecretStatus }
  | { type: "toast/push"; toast: ToastItem }
  | { type: "toast/dismiss"; id: string }
  | { type: "ui/view"; view: MainView }
  | { type: "ui/diff-panel"; open: boolean }
  | { type: "ui/palette"; open: boolean }
  | { type: "ui/project-dialog"; open: boolean };

export const DEFAULT_SETTINGS: Settings = {
  projects_directory: "",
  text_size: 13,
  sidebar_hover: true,
  theme: "system",
  default_provider: "go",
  default_model: "",
  max_turns: 20,
  max_total_turns: 200,
  context_token_budget: 16000,
  context_messages: 20,
  approval_timeout_seconds: 300,
  confirm_reads: false,
  recent_roots: [],
  concurrency_limit: 3,
  automations_enabled: true,
  onboarded: false,
};

export const DEFAULT_SECRET_STATUS: SecretStatus = {
  go: false,
  openai: false,
  anthropic: false,
};

export const initialState: AppState = {
  projects: [],
  threadsByProject: {},
  activeProjectRoot: null,
  activeThreadId: null,
  mainView: "thread",
  messages: {},
  streams: {},
  traces: {},
  running: {},
  compacting: {},
  runStartedAt: {},
  runDurations: {},
  diffs: {},
  comments: {},
  approvals: [],
  merges: {},
  sendErrors: {},
  skills: [],
  automations: [],
  reviews: [],
  settings: DEFAULT_SETTINGS,
  settingsLoaded: false,
  secretStatus: DEFAULT_SECRET_STATUS,
  toasts: [],
  diffPanelOpen: false,
  paletteOpen: false,
  projectDialogOpen: false,
};

let nextId = 0;

/** Deterministic id for reducer-created rows (messages, trace entries). */
export function newId(prefix: string): string {
  nextId += 1;
  return `${prefix}-${Date.now().toString(36)}-${nextId}`;
}

/** Reset the id counter (tests only). */
export function resetIds(): void {
  nextId = 0;
}

export function pushToast(
  state: AppState,
  message: string,
  tone: ToastTone,
): AppState {
  return {
    ...state,
    toasts: [...state.toasts, { id: newId("toast"), message, tone }],
  };
}

function setThreadRunning(state: AppState, threadId: string, running: boolean): AppState {
  const threadsByProject: Record<string, ThreadInfo[]> = {};
  for (const [root, threads] of Object.entries(state.threadsByProject)) {
    threadsByProject[root] = threads.map((t) =>
      t.id === threadId ? { ...t, running } : t,
    );
  }
  return {
    ...state,
    threadsByProject,
    running: { ...state.running, [threadId]: running },
  };
}

function appendTrace(
  state: AppState,
  threadId: string,
  entry: ToolTraceEntry,
): AppState {
  return {
    ...state,
    traces: {
      ...state.traces,
      [threadId]: [...(state.traces[threadId] ?? []), entry],
    },
  };
}

function appendMessage(
  state: AppState,
  threadId: string,
  message: ChatMessage,
): AppState {
  return {
    ...state,
    messages: {
      ...state.messages,
      [threadId]: [...(state.messages[threadId] ?? []), message],
    },
  };
}

/**
 * Apply one backend thread event to state. Pure — the provider calls this
 * via dispatch and performs the diff refresh side effect on `finished`.
 */
export function applyThreadEvent(
  state: AppState,
  envelope: ThreadEventEnvelope,
  measureDuration = true,
): AppState {
  const { thread_id: threadId, run_id: runId, event } = envelope;
  switch (event.kind) {
    case "started": {
      let next = setThreadRunning(state, threadId, true);
      next = {
        ...next,
        traces: { ...next.traces, [threadId]: [] },
        runStartedAt: { ...next.runStartedAt, [threadId]: Date.now() },
        streams: { ...next.streams, [threadId]: "" },
        compacting: { ...next.compacting, [threadId]: false },
      };
      return next;
    }
    case "assistant_text": {
      const prev = state.streams[threadId] ?? "";
      return {
        ...state,
        streams: { ...state.streams, [threadId]: prev + event.text },
      };
    }
    case "tool_started": {
      let next = state;
      const text = state.streams[threadId];
      if (text) next = appendMessage(next, threadId, { id: newId("msg"), role: "assistant", text, runId });
      next = { ...next, streams: { ...next.streams, [threadId]: "" } };
      next = appendMessage(next, threadId, { id: newId("tool"), role: "assistant", text: event.summary, runId, tool: { name: event.tool } });
      return appendTrace(next, threadId, {
        id: newId("trace"),
        tool: event.tool,
        summary: event.summary,
      });
    }
    case "tool_finished": {
      const entries = [...(state.traces[threadId] ?? [])];
      // Mark the most recent open entry for this tool finished; if none is
      // open (out-of-order delivery), record a standalone finished entry.
      let marked = false;
      for (let i = entries.length - 1; i >= 0; i--) {
        const entry = entries[i];
        if (entry !== undefined && entry.tool === event.tool && entry.ok === undefined && !entry.summary.startsWith("approval ")) {
          entries[i] = { ...entry, ok: event.ok };
          marked = true;
          break;
        }
      }
      if (!marked) {
        entries.push({
          id: newId("trace"),
          tool: event.tool,
          summary: event.ok ? "finished" : "failed",
          ok: event.ok,
        });
      }
      return {
        ...state,
        traces: { ...state.traces, [threadId]: entries },
        messages: { ...state.messages, [threadId]: (() => {
          const rows = [...(state.messages[threadId] ?? [])];
          for (let i = rows.length - 1; i >= 0; i--) {
            const row = rows[i]!;
            if (row.runId === runId && row.tool?.name === event.tool && row.tool.ok === undefined) {
              rows[i] = { ...row, tool: { ...row.tool, ok: event.ok, output: event.output } }; break;
            }
          }
          return rows;
        })() },
      };
    }
    case "approval_decided": {
      return appendTrace(state, threadId, {
        id: newId("trace"),
        tool: event.tool,
        summary: `approval ${event.decision}`,
        ok: event.decision === "deny" ? false : undefined,
      });
    }
    case "context_compacting": return { ...state, compacting: { ...state.compacting, [threadId]: true } };
    case "context_checkpoint": return { ...state, compacting: { ...state.compacting, [threadId]: false } };
    case "incomplete":
    case "finished": {
      let next = appendMessage(state, threadId, {
        id: newId("msg"),
        role: "assistant",
        text: event.result,
        runId,
        final: true,
        incomplete: event.kind === "incomplete",
      });
      const startedAt = next.runStartedAt[threadId];
      if (measureDuration && startedAt !== undefined) {
        next = {
          ...next,
          runDurations: { ...next.runDurations, [runId]: Math.max(0, Date.now() - startedAt) },
        };
      }
      next = {
        ...next,
        streams: { ...next.streams, [threadId]: "" },
        compacting: { ...next.compacting, [threadId]: false },
        approvals: next.approvals.filter(approval => approval.thread_id !== threadId),
      };
      return setThreadRunning(next, threadId, false);
    }
    case "failed": {
      const stopped = event.error.startsWith("Stopped by you.");
      let next = appendMessage(state, threadId, {
        id: newId("msg"),
        role: "system",
        text: stopped ? event.error : `Run failed: ${event.error}`,
        runId,
      });
      const startedAt = next.runStartedAt[threadId];
      if (measureDuration && startedAt !== undefined) {
        next = {
          ...next,
          runDurations: { ...next.runDurations, [runId]: Math.max(0, Date.now() - startedAt) },
        };
      }
      if (!stopped) next = pushToast(next, `Run failed: ${event.error}`, "danger");
      next = {
        ...next,
        streams: { ...next.streams, [threadId]: "" },
        compacting: { ...next.compacting, [threadId]: false },
        approvals: next.approvals.filter(approval => approval.thread_id !== threadId),
      };
      return setThreadRunning(next, threadId, false);
    }
  }
}

export function reducer(state: AppState, action: AppAction): AppState {
  switch (action.type) {
    case "project/opened": {
      const exists = state.projects.some((p) => p.root === action.project.root);
      return {
        ...state,
        projects: exists
          ? state.projects.map((p) =>
              p.root === action.project.root ? action.project : p,
            )
          : [...state.projects, action.project],
        threadsByProject:
          state.threadsByProject[action.project.root] === undefined
            ? { ...state.threadsByProject, [action.project.root]: [] }
            : state.threadsByProject,
        activeProjectRoot: action.project.root,
        mainView: "thread",
      };
    }
    case "project/selected": {
      const threads = state.threadsByProject[action.root] ?? [];
      return {
        ...state,
        activeProjectRoot: action.root,
        activeThreadId: threads.some((t) => t.id === state.activeThreadId)
          ? state.activeThreadId
          : (threads[0]?.id ?? null),
        mainView: "thread",
      };
    }
    case "thread/created": {
      return {
        ...state,
        threadsByProject: {
          ...state.threadsByProject,
          [action.projectRoot]: [
            ...(state.threadsByProject[action.projectRoot] ?? []),
            action.thread,
          ],
        },
        activeProjectRoot: action.projectRoot,
        activeThreadId: action.thread.id,
        mainView: "thread",
      };
    }
    case "thread/selected": {
      return {
        ...state,
        activeProjectRoot: action.projectRoot,
        activeThreadId: action.threadId,
        mainView: "thread",
      };
    }
    case "thread/updated": {
      return {
        ...state,
        threadsByProject: {
          ...state.threadsByProject,
          [action.projectRoot]: (state.threadsByProject[action.projectRoot] ?? []).map(
            (t) => (t.id === action.thread.id ? action.thread : t),
          ),
        },
      };
    }
    case "thread/listed": {
      const current = state.activeThreadId;
      const stillThere =
        current !== null && action.threads.some((t) => t.id === current);
      return {
        ...state,
        threadsByProject: {
          ...state.threadsByProject,
          [action.projectRoot]: action.threads,
        },
        // When the active project is (re)listed, adopt its first thread
        // unless the current selection survives.
        activeThreadId:
          state.activeProjectRoot === action.projectRoot
            ? stillThere
              ? current
              : (action.threads[0]?.id ?? null)
            : state.activeThreadId,
      };
    }
    case "thread/removed": {
      const remaining = (state.threadsByProject[action.projectRoot] ?? []).filter(
        (t) => t.id !== action.threadId,
      );
      const wasActive =
        state.activeProjectRoot === action.projectRoot &&
        state.activeThreadId === action.threadId;
      // Select the neighbor that slid into the removed slot, else the last
      // one, else nothing.
      let nextActive = state.activeThreadId;
      if (wasActive) {
        const removedIndex = (
          state.threadsByProject[action.projectRoot] ?? []
        ).findIndex((t) => t.id === action.threadId);
        nextActive =
          remaining[Math.min(Math.max(removedIndex, 0), remaining.length - 1)]
            ?.id ?? null;
      }
      const restMerges = { ...state.merges };
      delete restMerges[action.threadId];
      const restErrors = { ...state.sendErrors };
      delete restErrors[action.threadId];
      return {
        ...state,
        threadsByProject: {
          ...state.threadsByProject,
          [action.projectRoot]: remaining,
        },
        activeThreadId: nextActive,
        merges: restMerges,
        sendErrors: restErrors,
      };
    }
    case "thread/merged": {
      return {
        ...state,
        merges: {
          ...state.merges,
          [action.threadId]: toLastMerge(action.result),
        },
      };
    }
    case "thread/send-failed": {
      const message = isConcurrencyLimitError(action.error)
        ? concurrencyToast(state.settings.concurrency_limit)
        : `Send failed: ${action.error}`;
      const next = pushToast(state, message, "danger");
      return {
        ...next,
        sendErrors: { ...next.sendErrors, [action.threadId]: message },
      };
    }
    case "thread/send-cleared": {
      const rest = { ...state.sendErrors };
      delete rest[action.threadId];
      return { ...state, sendErrors: rest };
    }
    case "threads/restored": {
      const projects = [...action.projects];
      const threadsByProject = { ...action.threadsByProject };
      const firstRoot = projects[0]?.root ?? null;
      const keepSelection =
        state.activeProjectRoot !== null &&
        state.activeThreadId !== null &&
        (threadsByProject[state.activeProjectRoot] ?? []).some(
          (t) => t.id === state.activeThreadId,
        );
      return {
        ...state,
        projects,
        threadsByProject: { ...state.threadsByProject, ...threadsByProject },
        activeProjectRoot: firstRoot ?? state.activeProjectRoot,
        activeThreadId: keepSelection
          ? state.activeThreadId
          : firstRoot !== null
            ? (threadsByProject[firstRoot]?.[0]?.id ?? null)
            : state.activeThreadId,
        mainView: "thread",
      };
    }
    case "thread/event": {
      return applyThreadEvent(state, action.envelope);
    }
    case "thread/history-loaded": {
      let next: AppState = {
        ...state,
        messages: { ...state.messages, [action.threadId]: [] },
        streams: { ...state.streams, [action.threadId]: "" },
        traces: { ...state.traces, [action.threadId]: [] },
      };
      for (const item of action.history) {
        if (item.kind === "user") next = appendMessage(next, action.threadId, { id: newId("msg"), role: "user", text: item.text, runId: item.run_id });
        else if (item.kind === "legacy") next = appendMessage(next, action.threadId, item.message);
        else next = applyThreadEvent(next, item.envelope, false);
      }
      if (next.running[action.threadId]) {
        next = appendMessage(next, action.threadId, { id: newId("msg"), role: "system", text: "Run interrupted. Review any changes, then send a follow-up to continue." });
        next = setThreadRunning(next, action.threadId, false);
      }
      return { ...next, toasts: state.toasts };
    }
    case "message/append": {
      return appendMessage(state, action.threadId, action.message);
    }
    case "diff/set": {
      return {
        ...state,
        diffs: { ...state.diffs, [action.threadId]: action.diff },
      };
    }
    case "comment/added": {
      return {
        ...state,
        comments: {
          ...state.comments,
          [action.threadId]: [...(state.comments[action.threadId] ?? []), action.comment],
        },
      };
    }
    case "thread/skills-updated": {
      // The owning project is not always known here (review-queue flows),
      // so update the thread everywhere it appears.
      const threadsByProject: Record<string, ThreadInfo[]> = {};
      for (const [root, threads] of Object.entries(state.threadsByProject)) {
        threadsByProject[root] = threads.map((t) =>
          t.id === action.thread.id ? action.thread : t,
        );
      }
      return { ...state, threadsByProject };
    }
    case "skill/loaded": {
      return { ...state, skills: action.skills };
    }
    case "skill/added": {
      if (state.skills.some((s) => s.id === action.skill.id)) {
        return {
          ...state,
          skills: state.skills.map((s) =>
            s.id === action.skill.id ? action.skill : s,
          ),
        };
      }
      return { ...state, skills: [...state.skills, action.skill] };
    }
    case "skill/updated": {
      return {
        ...state,
        skills: state.skills.map((s) =>
          s.id === action.skill.id ? action.skill : s,
        ),
      };
    }
    case "skill/removed": {
      return {
        ...state,
        skills: state.skills.filter((s) => s.id !== action.skillId),
      };
    }
    case "automation/loaded": {
      return { ...state, automations: action.automations };
    }
    case "automation/added": {
      if (state.automations.some((a) => a.id === action.automation.id)) {
        return {
          ...state,
          automations: state.automations.map((a) =>
            a.id === action.automation.id ? action.automation : a,
          ),
        };
      }
      return { ...state, automations: [...state.automations, action.automation] };
    }
    case "automation/updated": {
      return {
        ...state,
        automations: state.automations.map((a) =>
          a.id === action.automation.id ? action.automation : a,
        ),
      };
    }
    case "automation/removed": {
      return {
        ...state,
        automations: state.automations.filter(
          (a) => a.id !== action.automationId,
        ),
      };
    }
    case "review/loaded": {
      return { ...state, reviews: action.reviews };
    }
    case "review/added": {
      if (state.reviews.some((r) => r.id === action.review.id)) {
        return {
          ...state,
          reviews: state.reviews.map((r) =>
            r.id === action.review.id ? action.review : r,
          ),
        };
      }
      return { ...state, reviews: [...state.reviews, action.review] };
    }
    case "review/updated": {
      return {
        ...state,
        reviews: state.reviews.map((r) =>
          r.id === action.review.id ? action.review : r,
        ),
      };
    }
    case "approval/enqueued": {
      if (
        state.approvals.some((a) => a.approval_id === action.request.approval_id)
      ) {
        return state;
      }
      return { ...state, approvals: [...state.approvals, action.request] };
    }
    case "approval/dequeued": {
      return {
        ...state,
        approvals: state.approvals.filter(
          (a) => a.approval_id !== action.approvalId,
        ),
      };
    }
    case "settings/loaded":
    case "settings/patched": {
      return { ...state, settings: action.settings, settingsLoaded: true };
    }
    case "secrets/loaded": {
      return { ...state, secretStatus: action.status };
    }
    case "toast/push": {
      return { ...state, toasts: [...state.toasts, action.toast] };
    }
    case "toast/dismiss": {
      return {
        ...state,
        toasts: state.toasts.filter((t) => t.id !== action.id),
      };
    }
    case "ui/view": {
      return { ...state, mainView: action.view, diffPanelOpen: action.view === "thread" && state.diffPanelOpen };
    }
    case "ui/diff-panel": {
      return { ...state, diffPanelOpen: action.open };
    }
    case "ui/palette": {
      return { ...state, paletteOpen: action.open };
    }
    case "ui/project-dialog": {
      return { ...state, projectDialogOpen: action.open };
    }
  }
}
