// Pure state model for the Themis app shell. No Tauri/DOM imports —
// safe to load in node-based unit tests.
import type {
  AttachedMedia,
  BrowserRecording,
  ChangedFile,
  ContextBudget,
  McpConnections,
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
  ThreadChange,
  ThreadTasks,
  QueuedMessage,
  Goal,
} from "../lib/types";
import type { ToastItem, ToastTone } from "../components";
import { reduceCollectionActions } from "./reducer/collections";
import { reduceUiActions } from "./reducer/ui";
import { DEFAULT_MODEL } from "../lib/models";

export type MessageRole = PersistedMessage["role"];
export type ChatMessage = PersistedMessage;

export interface ToolTraceEntry {
  id: string;
  tool: string;
  label?: string;
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
  mcpConnections: McpConnections;
  projects: ProjectInfo[];
  threadsByProject: Record<string, ThreadInfo[]>;
  activeProjectRoot: string | null;
  activeThreadId: string | null;
  /** Settings › Browser: the agent's first browser step opens Workspace on the Browser tab. */
  browserOpensWorkspace: boolean;
  mainView: MainView;
  /** Where Settings opens when another screen sends the person there (such as a missing key). */
  settingsFocus: { section: string; key?: string } | null;
  messages: Record<string, ChatMessage[]>;
  /** Live streamed assistant text per thread (cleared on finished/failed). */
  streams: Record<string, string>;
  traces: Record<string, ToolTraceEntry[]>;
  running: Record<string, boolean>;
  activeRunIds: Record<string, string>;
  compacting: Record<string, boolean>;
  /** Latest context accounting per thread, live or replayed from history. */
  contextBudgets: Record<string, ContextBudget>;
  runStartedAt: Record<string, number>;
  runDurations: Record<string, number>;
  /** Files a run changed, held until its final message arrives. */
  runChanges: Record<string, ChangedFile[]>;
  /** Media attached during a run, waiting for its final response. */
  runMedia: Record<string, AttachedMedia[]>;
  diffs: Record<string, DiffState | null>;
  comments: Record<string, ThreadComment[]>;
  approvals: ApprovalRequest[];
  /** Recently removed thread ids, so a late event cannot resurrect them. */
  removedThreadIds: string[];
  merges: Record<string, LastMerge>;
  /** Latest send failure per thread (cleared on next successful send). */
  sendErrors: Record<string, string>;
  skills: Skill[];
  automations: Automation[];
  reviews: ReviewItem[];
  settings: Settings;
  settingsLoaded: boolean;
  secretStatus: SecretStatus;
  /** The secret status has loaded once (before that, nothing claims a key is missing). */
  secretStatusLoaded: boolean;
  /** What OpenCode Go said about the key last time it was checked (null = not yet). */
  goKeyCheck: "works" | "rejected" | "unavailable" | "missing" | null;
  toasts: ToastItem[];
  diffPanelOpen: boolean;
  /** A file to show in the Workspace changes view, e.g. from an edited-files list. */
  diffFocus: { path: string; at: number } | null;
  /** Asks the open (or next opened) Workspace panel to show a tab. */
  workspaceTabRequest: { tab: WorkspaceTab; at: number } | null;
  /** Each chat's built-in browser: who drives it and any recording (live only). */
  browsers: Record<string, { controller: string; recording?: BrowserRecording; at: number }>;
  /** Per-chat task feed behind the header indicator and the Tasks tab. */
  tasks: Record<string, ThreadTasks>;
  /** Messages waiting for each chat's current run to end. */
  queues: Record<string, QueuedMessage[]>;
  /** Each chat's goal (`null` when it has none). */
  goals: Record<string, Goal | null>;
  paletteOpen: boolean;
  projectDialogOpen: boolean;
}

/** The Workspace panel's views. */
export type WorkspaceTab = "changes" | "files" | "branches" | "pulls" | "tasks" | "browser";

export type AppAction =
  | { type: "mcp/status"; connections: McpConnections }
  | { type: "project/opened"; project: ProjectInfo; select?: boolean }
  | { type: "project/selected"; root: string }
  | { type: "project/removed"; root: string }
  | { type: "thread/created"; projectRoot: string; thread: ThreadInfo }
  | { type: "thread/running-loaded"; threadId: string; running: boolean }
  | { type: "approval/loaded"; requests: ApprovalRequest[] }
  | { type: "thread/synced"; projectRoot: string; thread: ThreadInfo }
  | { type: "thread/selected"; projectRoot: string; threadId: string }
  | { type: "thread/updated"; projectRoot: string; thread: ThreadInfo }
  | { type: "thread/listed"; projectRoot: string; threads: ThreadInfo[] }
  | { type: "thread/removed"; projectRoot: string; threadId: string }
  | { type: "thread/changed"; change: ThreadChange }
  | { type: "thread/merged"; threadId: string; result: MergeResult }
  | { type: "thread/send-failed"; threadId: string; error: string }
  | { type: "thread/send-cleared"; threadId: string }
  | { type: "queue/loaded"; threadId: string; queue: QueuedMessage[] }
  | { type: "goal/loaded"; threadId: string; goal: Goal | null }
  | {
      type: "threads/restored";
      projects: ProjectInfo[];
      threadsByProject: Record<string, ThreadInfo[]>;
    }
  | { type: "thread/event"; envelope: ThreadEventEnvelope }
  | { type: "thread/history-loaded"; threadId: string; history: HistoryItem[]; running?: boolean }
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
  | { type: "approval/expired"; approvalId: string }
  | { type: "settings/loaded"; settings: Settings }
  | { type: "settings/patched"; settings: Settings }
  | { type: "secrets/loaded"; status: SecretStatus }
  | { type: "provider/checked"; state: NonNullable<AppState["goKeyCheck"]> }
  | { type: "toast/push"; toast: ToastItem }
  | { type: "toast/dismiss"; id: string }
  | { type: "ui/view"; view: MainView; settings?: { section: string; key?: string } }
  | { type: "ui/settings-section"; section: string }
  | { type: "ui/diff-panel"; open: boolean; focus?: string; tab?: WorkspaceTab }
  | { type: "browser/opens-workspace"; value: boolean }
  | { type: "ui/tab-request-consumed" }
  | { type: "tasks/loaded"; threadId: string; tasks: ThreadTasks }
  | { type: "ui/palette"; open: boolean }
  | { type: "ui/project-dialog"; open: boolean };

export const DEFAULT_SETTINGS: Settings = {
  projects_directory: "",
  text_size: 13,
  theme_palette: "system",
  font_family: "system",
  sidebar_hover: true,
  theme: "dark",
  default_provider: "go",
  default_model: DEFAULT_MODEL,
  max_total_turns: 200,
  context_token_cap: 0,
  approval_timeout_seconds: 300,
  confirm_reads: false,
  recent_roots: [],
  concurrency_limit: 3,
  automations_enabled: true,
  completion_sound: false,
  completion_haptic: false,
  goal_notifications: true,
  onboarded: false,
};

export const DEFAULT_SECRET_STATUS: SecretStatus = {
  go: false,
};

export const initialState: AppState = {
  mcpConnections: {},
  projects: [],
  threadsByProject: {},
  activeProjectRoot: null,
  activeThreadId: null,
  browserOpensWorkspace: false,
  mainView: "thread",
  settingsFocus: null,
  messages: {},
  streams: {},
  traces: {},
  running: {},
  activeRunIds: {},
  compacting: {},
  contextBudgets: {},
  runStartedAt: {},
  runDurations: {},
  runChanges: {},
  runMedia: {},
  diffs: {},
  comments: {},
  approvals: [],
  removedThreadIds: [],
  merges: {},
  sendErrors: {},
  skills: [],
  automations: [],
  reviews: [],
  settings: DEFAULT_SETTINGS,
  settingsLoaded: false,
  secretStatus: DEFAULT_SECRET_STATUS,
  secretStatusLoaded: false,
  goKeyCheck: null,
  toasts: [],
  diffPanelOpen: false,
  diffFocus: null,
  workspaceTabRequest: null,
  browsers: {},
  tasks: {},
  queues: {},
  goals: {},
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

/** Fixed note shown when an approval timed out and its action was skipped. */
export const APPROVAL_EXPIRED_NOTE = "Approval expired — the action was skipped";

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
  const activeRunIds = { ...state.activeRunIds };
  if (!running) delete activeRunIds[threadId];
  for (const [root, threads] of Object.entries(state.threadsByProject)) {
    threadsByProject[root] = threads.map((t) =>
      t.id === threadId ? { ...t, running } : t,
    );
  }
  return {
    ...state,
    threadsByProject,
    running: { ...state.running, [threadId]: running },
    activeRunIds,
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

/** The marker the runtime puts on the message that starts a goal turn. */
export const GOAL_TURN_MARKER = "[Goal — runtime, not from the user]";

/** The turn number of a goal-turn message, if `text` is one. */
export function goalTurnNumber(text: string): number | undefined {
  if (!text.startsWith(GOAL_TURN_MARKER)) return undefined;
  const match = /^\s*Turn (\d+)/.exec(text.slice(GOAL_TURN_MARKER.length));
  return match ? Number(match[1]) : undefined;
}

/** A user message row; a goal-turn message becomes a divider row. */
function userMessage(text: string, runId: string | undefined, attachments?: string[]): ChatMessage {
  const goalTurn = goalTurnNumber(text);
  return {
    id: newId("msg"), role: "user", text, runId,
    ...(attachments?.length ? { attachments } : {}),
    ...(goalTurn === undefined ? {} : { goalTurn }),
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

export function usedPercent(budget: ContextBudget): number {
  return budget.usable_tokens > 0 ? Math.min(100, Math.round(budget.used_tokens / budget.usable_tokens * 100)) : 0;
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
        activeRunIds: { ...next.activeRunIds, [threadId]: runId },
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
      next = appendMessage(next, threadId, { id: newId("tool"), role: "assistant", text: event.summary, runId, tool: { name: event.tool, ...(event.label ? { label: event.label } : {}) } });
      return appendTrace(next, threadId, {
        id: newId("trace"),
        tool: event.tool,
        ...(event.label ? { label: event.label } : {}),
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
    case "context_usage":
    case "context_suspended":
    case "background_tasks_changed": return state;
    case "steered": {
      // Text streamed so far came before the steer; keep the order the model saw.
      let next = state;
      const text = state.streams[threadId];
      if (text) next = appendMessage(next, threadId, { id: newId("msg"), role: "assistant", text, runId });
      next = { ...next, streams: { ...next.streams, [threadId]: "" } };
      return appendMessage(next, threadId, { id: newId("msg"), role: "user", text: event.text, runId, steered: true });
    }
    case "user_message": {
      // This window's own send is already on screen without a run: adopt it.
      const rows = state.messages[threadId] ?? [];
      if (runId && rows.some(row => row.role === "user" && row.runId === runId)) return state;
      let own = rows.length - 1;
      while (own >= 0 && !(rows[own].role === "user" && !rows[own].runId && !rows[own].steered && rows[own].text === event.text)) own -= 1;
      if (own < 0) return appendMessage(state, threadId, userMessage(event.text, runId || undefined, event.attachments));
      const adopted = rows.map((row, index) => index === own ? { ...row, runId } : row);
      return { ...state, messages: { ...state.messages, [threadId]: adopted } };
    }
    case "queue_changed":
      return { ...state, queues: { ...state.queues, [threadId]: event.queue } };
    case "goal_changed":
      return { ...state, goals: { ...state.goals, [threadId]: event.goal } };
    case "goal_note":
      return appendMessage(state, threadId, { id: newId("msg"), role: "system", text: event.reason, runId, goalNote: { outcome: event.outcome, reason: event.reason } });
    case "question_asked": {
      // A question asked again on its open topic updates its box where it was first asked.
      const rows = state.messages[threadId] ?? [];
      if (rows.some(row => row.question?.id === event.question.id)) {
        const updated = rows.map(row => row.question?.id === event.question.id ? { ...row, text: event.question.question, question: { ...event.question, answer: row.question.answer ?? event.question.answer } } : row);
        return { ...state, messages: { ...state.messages, [threadId]: updated } };
      }
      return appendMessage(state, threadId, { id: `question-${event.question.id}`, role: "system", text: event.question.question, runId: event.question.run_id, question: event.question });
    }
    case "question_answered": {
      const rows = state.messages[threadId] ?? [];
      const updated = rows.map(row => row.question?.id === event.id ? { ...row, question: { ...row.question, answer: event.answer } } : row);
      return { ...state, messages: { ...state.messages, [threadId]: updated } };
    }
    case "context_budget": {
      const rows = state.messages[threadId] ?? [];
      const last = rows[rows.length - 1];
      const percent = usedPercent(event.budget);
      // The first budget after a checkpoint completes its divider.
      const messages = last?.checkpoint && last.checkpoint.after === undefined
        ? { ...state.messages, [threadId]: [...rows.slice(0, -1), { ...last, checkpoint: { ...last.checkpoint, after: percent } }] }
        : state.messages;
      return { ...state, messages, contextBudgets: { ...state.contextBudgets, [threadId]: event.budget } };
    }
    case "context_compacting": return { ...state, compacting: { ...state.compacting, [threadId]: true } };
    case "context_checkpoint": {
      const before = state.contextBudgets[threadId];
      const next = appendMessage(state, threadId, {
        id: newId("checkpoint"), role: "system", text: "Context compacted", runId,
        checkpoint: before ? { before: usedPercent(before) } : {},
      });
      return { ...next, compacting: { ...next.compacting, [threadId]: false } };
    }
    case "files_changed":
      return { ...state, runChanges: { ...state.runChanges, [runId]: event.files } };
    case "browser_changed":
      return { ...state, browsers: { ...state.browsers, [threadId]: { controller: event.controller, recording: event.recording, at: Date.now() } } };
    case "media_attached": {
      const previous = state.runMedia[runId] ?? [];
      const added = event.files.filter(file => !previous.some(seen => seen.path === file.path));
      return { ...state, runMedia: { ...state.runMedia, [runId]: [...previous, ...added] } };
    }
    case "incomplete":
    case "finished": {
      // Text streamed before the final answer (an answer the runtime then had checked or
      // continued, without a tool call between) stays as a progress line: dropping it made
      // it flash on screen and vanish.
      const streamed = (state.streams[threadId] ?? "").trim();
      const result = event.result.trim();
      const earlier = result && streamed !== result && streamed.endsWith(result) ? streamed.slice(0, -result.length).trim() : "";
      let next = earlier ? appendMessage(state, threadId, { id: newId("msg"), role: "assistant", text: earlier, runId }) : state;
      next = appendMessage(next, threadId, {
        id: newId("msg"),
        role: "assistant",
        text: event.result,
        model: event.model,
        runId,
        final: true,
        incomplete: event.kind === "incomplete",
        ...(state.runChanges[runId] ? { changedFiles: state.runChanges[runId] } : {}),
        ...(state.runMedia[runId] ? { media: state.runMedia[runId] } : {}),
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
      // Keep what was already streamed: the user saw it, so Stop or a failure must not discard it.
      const partial = state.streams[threadId];
      let next = partial?.trim()
        ? appendMessage(state, threadId, {
          id: newId("msg"), role: "assistant", text: partial, runId, final: true, incomplete: true,
          interrupted: stopped ? "stopped" : "failed",
        })
        : state;
      next = appendMessage(next, threadId, {
        id: newId("msg"),
        role: "system",
        text: stopped ? event.error : `Run failed: ${event.error}`,
        runId,
        ...(state.runChanges[runId] ? { changedFiles: state.runChanges[runId] } : {}),
        ...(state.runMedia[runId] ? { media: state.runMedia[runId] } : {}),
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
  const collectionState = reduceCollectionActions(state, action);
  if (collectionState !== null) return collectionState;
  const uiState = reduceUiActions(state, action);
  if (uiState !== null) return uiState;
  switch (action.type) {
    case "mcp/status": return { ...state, mcpConnections: action.connections };
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
        activeProjectRoot: action.select === false ? state.activeProjectRoot : action.project.root,
        mainView: action.select === false ? state.mainView : "thread",
      };
    }
    case "project/removed": {
      const projects = state.projects.filter((p) => p.root !== action.root);
      const threadsByProject = { ...state.threadsByProject };
      delete threadsByProject[action.root];
      const automations = state.automations.filter((a) => a.project_root !== action.root);
      if (state.activeProjectRoot !== action.root) return { ...state, projects, threadsByProject, automations };
      // Fall back to the built-in project, else the first remaining one.
      const next = projects.find((p) => p.is_default) ?? projects[0] ?? null;
      const threads = [...(next ? threadsByProject[next.root] ?? [] : [])].sort((a, b) => (b.last_activity_seq ?? 0) - (a.last_activity_seq ?? 0));
      return { ...state, projects, threadsByProject, automations, activeProjectRoot: next?.root ?? null, activeThreadId: threads[0]?.id ?? null, mainView: "thread" };
    }
    case "project/selected": {
      // Without a selection in this project, open its most recently active chat.
      const threads = [...(state.threadsByProject[action.root] ?? [])].sort((a, b) => (b.last_activity_seq ?? 0) - (a.last_activity_seq ?? 0));
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
      // The server's own `created` event can arrive before this reply: upsert, never append twice.
      const synced = reducer(state, { type: "thread/synced", projectRoot: action.projectRoot, thread: action.thread });
      return {
        ...synced,
        activeProjectRoot: action.projectRoot,
        activeThreadId: action.thread.id,
        mainView: "thread",
      };
    }
    case "thread/running-loaded": return setThreadRunning(state, action.threadId, action.running);
    case "approval/loaded": return { ...state, approvals: action.requests };
    case "approval/expired": {
      // The backend already denied it after the wait ran out: drop the stale dialog and say so once.
      if (!state.approvals.some(approval => approval.approval_id === action.approvalId)) return state;
      const next = { ...state, approvals: state.approvals.filter(approval => approval.approval_id !== action.approvalId) };
      return pushToast(next, APPROVAL_EXPIRED_NOTE, "warning");
    }
    case "thread/synced": {
      const threads = state.threadsByProject[action.projectRoot] ?? [];
      return {
        ...state,
        threadsByProject: {
          ...state.threadsByProject,
          [action.projectRoot]: threads.some(thread => thread.id === action.thread.id)
            ? threads.map(thread => thread.id === action.thread.id ? action.thread : thread)
            : [...threads, action.thread],
        },
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
    case "thread/changed": {
      const { change } = action;
      if (change.kind === "removed") return reducer(state, { type: "thread/removed", projectRoot: change.project_root, threadId: change.thread_id });
      const known = state.threadsByProject[change.project_root];
      // Only follow projects already open in this window, and never bring back a thread removed here.
      if (!change.thread || known === undefined || state.removedThreadIds.includes(change.thread_id)) return state;
      const exists = known.some(thread => thread.id === change.thread_id);
      if (change.kind === "updated" && !exists) return state;
      // `created` replaces an existing entry in place, so a replayed event is not duplicated.
      return reducer(state, { type: "thread/synced", projectRoot: change.project_root, thread: change.thread });
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
      const without = <T,>(record: Record<string, T>): Record<string, T> => {
        const { [action.threadId]: _gone, ...rest } = record;
        return rest;
      };
      return {
        ...state,
        running: without(state.running),
        streams: without(state.streams),
        traces: without(state.traces),
        compacting: without(state.compacting),
        approvals: state.approvals.filter(approval => approval.thread_id !== action.threadId),
        removedThreadIds: [...state.removedThreadIds, action.threadId].slice(-200),
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
    case "queue/loaded":
      return { ...state, queues: { ...state.queues, [action.threadId]: action.queue } };
    case "goal/loaded":
      return { ...state, goals: { ...state.goals, [action.threadId]: action.goal } };
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
    case "browser/opens-workspace":
      return { ...state, browserOpensWorkspace: action.value };
    case "thread/event": {
      const { thread_id: threadId, event } = action.envelope;
      const next = applyThreadEvent(state, action.envelope);
      // The agent's first browser step in a run shows the Browser tab of the
      // chat on screen; later steps leave whatever the person chose.
      const firstBrowserStep = state.browserOpensWorkspace && event.kind === "tool_started" && event.tool.startsWith("browser_")
        && threadId === state.activeThreadId && state.mainView === "thread"
        && !(state.traces[threadId] ?? []).some((entry) => entry.tool.startsWith("browser_"));
      return firstBrowserStep
        ? { ...next, diffPanelOpen: true, workspaceTabRequest: { tab: "browser", at: Date.now() } }
        : next;
    }
    case "thread/history-loaded": {
      let next: AppState = {
        ...state,
        messages: { ...state.messages, [action.threadId]: [] },
        streams: { ...state.streams, [action.threadId]: "" },
        traces: { ...state.traces, [action.threadId]: [] },
      };
      for (const item of action.history) {
        if (item.kind === "user") next = appendMessage(next, action.threadId, userMessage(item.text, item.run_id, item.attachments));
        else if (item.kind === "legacy") next = appendMessage(next, action.threadId, item.message);
        else next = applyThreadEvent(next, item.envelope, false);
      }
      if (next.running[action.threadId] && !action.running) {
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
    default:
      return state;

  }
}

/** OpenCode Go can be used: a key is set and OpenCode has not rejected it. */
export function goUsable(state: Pick<AppState, "secretStatus" | "goKeyCheck">): boolean {
  return state.secretStatus.go && state.goKeyCheck !== "rejected";
}
