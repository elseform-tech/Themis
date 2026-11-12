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
} from "../../lib/types";
import type { ToastItem, ToastTone } from "../../components";
import { initialVisions, type VisionsAction, type VisionsState } from "../visions";
import { DEFAULT_MODEL } from "../../lib/models";

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

export type MainView = "thread" | "settings" | "queue" | "skills" | "automations" | "visions";

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
  /** Chats whose saved history has been loaded: others load when opened, so memory holds the
   *  chats in use, not every chat ever had. */
  historyLoaded: Record<string, true>;
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
  /** Counts runtime.jsonc and stored-key writes announced by the server, so open Settings
   *  sections read theirs again (settings.json changes arrive as `settings/loaded`). */
  settingsChanges: { runtime: number; secrets: number };
  secretStatus: SecretStatus;
  /** The secret status has loaded once (before that, nothing claims a key is missing). */
  secretStatusLoaded: boolean;
  /** What OpenCode Go said about the key last time it was checked (null = not yet). */
  goKeyCheck: "works" | "rejected" | "unavailable" | "missing" | null;
  toasts: ToastItem[];
  diffPanelOpen: boolean;
  /** A file to show in the Workspace changes view, e.g. from an edited-files list. */
  diffFocus: { path: string; at: number } | null;
  /** A file to show in Workspace › Files. */
  fileFocus: { path: string; at: number } | null;
  /** The subagent Workspace › Tasks shows in detail. */
  subagentFocus: string | null;
  /** Asks the open (or next opened) Workspace panel to show a tab. */
  workspaceTabRequest: { tab: WorkspaceTab; at: number } | null;
  /** Each chat's built-in browser: who drives it and any recording (live only). */
  browsers: Record<string, { controller: string; recording?: BrowserRecording; at: number }>;
  /** Where each chat's memory pass stands while it works (live only). */
  memoryWorking: Record<string, "checking" | "waiting">;
  /** Per-chat task feed behind the header indicator and the Tasks tab. */
  tasks: Record<string, ThreadTasks>;
  /** Messages waiting for each chat's current run to end. */
  queues: Record<string, QueuedMessage[]>;
  /** Each chat's goal (`null` when it has none). */
  goals: Record<string, Goal | null>;
  /** Visions, the open projects' channels and what was read of them. */
  visions: VisionsState;
  /** Each chat's to-do list, kept by its main agent. */
  paletteOpen: boolean;
  projectDialogOpen: boolean;
}

/** The Workspace panel's views. */
export type WorkspaceTab = "changes" | "files" | "pulls" | "tasks" | "browser" | "terminal";

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
  | { type: "approval/expired"; approvalId: string; steered?: boolean }
  | { type: "settings/loaded"; settings: Settings }
  | { type: "settings/patched"; settings: Settings }
  | { type: "settings/changed"; scope: "runtime" | "secrets" }
  | { type: "secrets/loaded"; status: SecretStatus }
  | { type: "provider/checked"; state: NonNullable<AppState["goKeyCheck"]> }
  | { type: "toast/push"; toast: ToastItem }
  | { type: "toast/dismiss"; id: string }
  | { type: "ui/view"; view: MainView; settings?: { section: string; key?: string } }
  | { type: "ui/settings-section"; section: string }
  | { type: "ui/diff-panel"; open: boolean; focus?: string; tab?: WorkspaceTab; file?: string; subagent?: string }
  | { type: "ui/subagent-focus"; id: string | null }
  | { type: "browser/opens-workspace"; value: boolean }
  | { type: "ui/tab-request-consumed" }
  | { type: "tasks/loaded"; threadId: string; tasks: ThreadTasks }
  | { type: "ui/palette"; open: boolean }
  | { type: "ui/project-dialog"; open: boolean }
  | VisionsAction;

export const DEFAULT_SETTINGS: Settings = {
  projects_directory: "",
  text_size: 12,
  theme_palette: "classic",
  font_family: "system",
  sidebar_hover: true,
  check_sources_for_updates: true,
  theme: "dark",
  default_provider: "go",
  default_model: DEFAULT_MODEL,
  max_total_turns: 200,
  context_token_cap: 0,
  approval_timeout_seconds: 600,
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
  historyLoaded: {},
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
  settingsChanges: { runtime: 0, secrets: 0 },
  secretStatus: DEFAULT_SECRET_STATUS,
  secretStatusLoaded: false,
  goKeyCheck: null,
  toasts: [],
  diffPanelOpen: false,
  diffFocus: null,
  fileFocus: null,
  subagentFocus: null,
  workspaceTabRequest: null,
  browsers: {},
  memoryWorking: {},
  tasks: {},
  queues: {},
  goals: {},
  visions: initialVisions,
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
