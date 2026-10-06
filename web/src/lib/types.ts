// Source-of-truth mirror of the Tauri bridge contract (Rust: crates/desktop).
// The desktop agent implements EXACTLY these command names, payloads, and
// event shapes. UI agents consume them via ./tauri.ts only.

export type ProviderKind = 'go' | 'legacy';

export interface GoModel {
  id: string;
  effort_levels: string[];
}
export type RiskLevel = 'read' | 'write' | 'execute' | 'network' | 'destructive';
export type ApprovalDecision = 'once' | 'always' | 'deny';
export type ThemeMode = 'dark' | 'light' | 'system';

export interface ProjectInfo {
  root: string;
  name: string;
  is_git: boolean;
  is_default?: boolean;
}

export interface ThreadInfo {
  id: string;
  title: string;
  provider: ProviderKind;
  model: string;
  reasoning_effort?: string | null;
  running: boolean;
  /** Worktree backing this thread (null for non-git read-only threads). */
  worktree_path: string | null;
  branch: string | null;
  base_branch: string | null;
  /** True when the thread was marked running at shutdown and reconciled on boot. */
  recovered: boolean;
  skill_ids: string[];
}

export interface MergeResult {
  applied: boolean;
  applied_files: string[];
  /** Paths that could not be applied (conflicts); nothing was touched. */
  conflicts: string[];
}

export interface SkillScript {
  name: string;
  content: string;
}

export interface Skill {
  id: string;
  name: string;
  description: string;
  instructions: string;
  /** Tool-name allowlist. Empty means all tools. Unknown names are rejected. */
  allowed_tools: string[];
  scripts: SkillScript[];
}

export interface SkillInput {
  name: string;
  description: string;
  instructions: string;
  allowed_tools: string[];
  scripts: SkillScript[];
}

export interface PluginSpec {
  package_kind?: "plugin" | "skill" | "mcp" | "hook" | null;
  icon?: string | null;
  disabled_skills?: string[];
  manual_skills?: string[];
  origin?: { kind: string; location: string; reference?: string | null; subdirectory?: string | null } | null;
  skill_paths?: Record<string, string>;
  executable_files?: string[];
  name: string; description: string; version: string;
  skills: Array<{ id: string; name: string; description: string; instructions: string; allowedTools: string[]; scripts: SkillScript[] }>;
  mcp: Record<string, { command?: string | null; args: string[]; url?: string | null; env: Record<string,string>; bearer_env?: string | null; enabled: boolean }>;
  hooks: Array<{name:string;event:string;command:string;enabled:boolean;timeout_seconds:number;blocking:boolean}>;
  files: Record<string,string>; unsupported: string[];
}
export interface Plugin { scope: "local"|"global"; revision: string; enabled: boolean; source: string|null; spec: PluginSpec }
export interface Marketplace { name: string; source: string }

export interface AutomationSchedule {
  repeat: "daily" | "weekdays" | "weekly";
  time: string;
  timezone: string;
  /** Monday = 0 through Sunday = 6. */
  weekday: number;
}

export interface Automation {
  id: string;
  name: string;
  project_root: string;
  target_thread_id?: string | null;
  provider: ProviderKind;
  model: string;
  reasoning_effort?: string | null;
  skill_ids: string[];
  /** Fixed interval in minutes, >= 1. */
  interval_mins: number;
  schedule?: AutomationSchedule | null;
  task: string;
  enabled: boolean;
  last_run_at: string | null;
  next_run_at: string;
  run_count: number;
}

export interface AutomationInput {
  name: string;
  project_root: string;
  target_thread_id?: string | null;
  provider: ProviderKind;
  model: string;
  reasoning_effort?: string | null;
  skill_ids: string[];
  interval_mins: number;
  schedule?: AutomationSchedule | null;
  task: string;
  enabled: boolean;
}

export type ReviewStatus = 'pending' | 'continued' | 'dismissed';

export interface DiagnosticsError {
  at: string;
  command: string;
  message: string;
}

export interface Diagnostics {
  app_version: string;
  os: string;
  /** Settings snapshot. Never contains secret values. */
  settings: Settings;
  recent_errors: DiagnosticsError[];
  persistence?: { name: string; status: 'pass' | 'warn' | 'fail' | 'unverified'; detail: string; action: string }[];
  recovery_notes?: number;
}

export type UpdateState = 'disabled' | 'up-to-date' | 'available' | 'error';

export interface UpdateStatus {
  state: UpdateState;
  version?: string;
  notes?: string;
  message?: string;
}

export interface ReviewItem {
  id: string;
  automation_id: string;
  thread_id: string;
  created_at: string;
  title: string;
  summary: string;
  status: ReviewStatus;
}

export interface RunHandle {
  run_id: string;
}

/** Mirrors themis-core RunEvent (mapped by the desktop bridge). */
export type ThreadEvent =
  | { kind: 'started'; task: string; max_turns: number }
  | { kind: 'assistant_text'; text: string }
  | { kind: 'tool_started'; tool: string; summary: string }
  | { kind: 'tool_finished'; tool: string; ok: boolean; output?: string }
  | { kind: 'approval_decided'; tool: string; decision: ApprovalDecision }
  | { kind: 'context_compacting' }
  | { kind: 'context_checkpoint'; summary: string }
  | { kind: 'incomplete'; result: string; model?: string }
  | { kind: 'finished'; result: string; model?: string }
  | { kind: 'failed'; error: string };

export interface ThreadEventEnvelope {
  thread_id: string;
  run_id: string;
  event: ThreadEvent;
}

export interface PersistedMessage {
  id: string;
  role: 'user' | 'assistant' | 'system';
  text: string;
  attachments?: string[];
  runId?: string;
  final?: boolean;
  model?: string;
  /** Internal handoff marker; not shown as a conversation label. */
  incomplete?: boolean;
  tool?: { name: string; ok?: boolean; output?: string };
}

export type HistoryItem =
  | { kind: "user"; run_id: string; text: string; attachments?: string[] }
  | { kind: "event"; envelope: ThreadEventEnvelope }
  | { kind: "legacy"; message: PersistedMessage };

export interface ApprovalRequest {
  thread_id: string;
  approval_id: string;
  tool: string;
  summary: string;
  risk: RiskLevel;
}

export type DiffLineKind = 'context' | 'add' | 'del';
export interface DiffLine {
  kind: DiffLineKind;
  text: string;
}
export interface DiffHunk {
  old_start: number;
  old_lines: number;
  new_start: number;
  new_lines: number;
  lines: DiffLine[];
}
export type DiffStatus = 'modified' | 'added' | 'deleted' | 'renamed';
export interface DiffFile {
  path: string;
  old_path?: string;
  status: DiffStatus;
  /** True when the file already differed from HEAD before the thread started. */
  preexisting: boolean;
  hunks: DiffHunk[];
}
export interface DiffState {
  available: boolean;
  reason?: string;
  files: DiffFile[];
}

export interface ThreadComment {
  id: string;
  path: string;
  comment: string;
}

export interface Settings {
  projects_directory: string;
  project_names?: Record<string, string>;
  text_size: number;
  theme_palette: string;
  font_family: "system" | "sans" | "serif" | "mono" | "rounded" | "avenir" | "helvetica" | "verdana" | "trebuchet" | "palatino" | "charter" | "menlo";
  sidebar_hover: boolean;
  theme: ThemeMode;
  default_provider: ProviderKind;
  default_model: string;
  max_turns: number;
  max_total_turns: number;
  context_token_budget: number;
  context_messages: number;
  approval_timeout_seconds: number;
  confirm_reads: boolean;
  recent_roots: string[];
  /** Max parallel agent runs across all threads (1..=16). */
  concurrency_limit: number;
  automations_enabled: boolean;
  completion_sound: boolean;
  completion_haptic: boolean;
  onboarded: boolean;
}

/** Which providers have a key stored. Values are NEVER exposed. */
export interface SecretStatus {
  go: boolean;
}

export const THREAD_EVENT_NAME = 'thread-event';
export const APPROVAL_REQUEST_NAME = 'approval-request';
export const REVIEW_ITEM_NAME = 'review-item-added';
