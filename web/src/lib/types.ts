// Source-of-truth mirror of the Tauri bridge contract (Rust: crates/desktop).
// The desktop agent implements EXACTLY these command names, payloads, and
// event shapes. UI agents consume them via ./tauri.ts only.

export type ProviderKind = 'go' | 'openai' | 'anthropic' | 'ollama' | 'custom';
export type RiskLevel = 'read' | 'write' | 'execute' | 'network' | 'destructive';
export type ApprovalDecision = 'once' | 'always' | 'deny';
export type ThemeMode = 'dark' | 'light' | 'system';

export interface ProjectInfo {
  root: string;
  name: string;
  is_git: boolean;
}

export interface ThreadInfo {
  id: string;
  title: string;
  provider: ProviderKind;
  model: string;
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

export interface Automation {
  id: string;
  name: string;
  project_root: string;
  provider: ProviderKind;
  model: string;
  skill_ids: string[];
  /** Fixed interval in minutes, >= 1. */
  interval_mins: number;
  task: string;
  enabled: boolean;
  last_run_at: string | null;
  next_run_at: string;
  run_count: number;
}

export interface AutomationInput {
  name: string;
  project_root: string;
  provider: ProviderKind;
  model: string;
  skill_ids: string[];
  interval_mins: number;
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
  | { kind: 'context_checkpoint'; summary: string }
  | { kind: 'finished'; result: string }
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
  runId?: string;
  final?: boolean;
  tool?: { name: string; ok?: boolean; output?: string };
}

export type HistoryItem =
  | { kind: "user"; run_id: string; text: string }
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
  text_size: number;
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
  onboarded: boolean;
}

/** Which providers have a key stored. Values are NEVER exposed. */
export interface SecretStatus {
  go: boolean;
  openai: boolean;
  anthropic: boolean;
}

export const THREAD_EVENT_NAME = 'thread-event';
export const APPROVAL_REQUEST_NAME = 'approval-request';
export const REVIEW_ITEM_NAME = 'review-item-added';
