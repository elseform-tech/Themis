// Source-of-truth mirror of the Tauri bridge contract (Rust: crates/desktop).
// The desktop agent implements EXACTLY these command names, payloads, and
// event shapes. UI agents consume them via ./tauri.ts only.

/** OpenCode Go, plus the providers in runtime.jsonc, whose models are named `<provider>/<model>`. */
export type ProviderKind = 'go';

export interface GoModel {
  id: string;
  effort_levels: string[];
  /** Set when Themis does not speak the model's protocol; such a model cannot be chosen. */
  unavailable_reason?: string | null;
  /** The runtime.jsonc provider serving the model (`id` is then `<provider>/<model>`); absent for OpenCode Go. */
  provider?: string | null;
}

/** A provider from `providers` in runtime.jsonc (`list_model_providers`); never key values. */
export interface ModelProvider {
  id: string;
  name: string;
  /** The Settings › Secrets secret holding its key. */
  secret: string;
  /** `unchecked`: the key is set and the models are listed in the file. */
  status: "works" | "rejected" | "unavailable" | "missing" | "unchecked";
  models: number;
}
export type RiskLevel = 'read' | 'write' | 'execute' | 'network' | 'destructive';
/** `chat` is "Allow in this chat": allow now and for every later run of this chat. */
export type ApprovalDecision = 'once' | 'chat' | 'deny';
export type ThemeMode = 'dark' | 'light' | 'system';

export interface ProjectInfo {
  root: string;
  name: string;
  is_git: boolean;
  is_default?: boolean;
}

export interface ThreadInfo {
  /** Durable conversation activity order; absent on older servers. */
  last_activity_seq?: number;
  id: string;
  title: string;
  provider: ProviderKind;
  model: string;
  approval_mode?: "custom" | "yolo";
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
  import_issues?: Array<{kind:string;name:string;status:string;field?:string|null;reason?:string|null;remedy?:string|null}>;
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
  files: Record<string,string>; unsupported: string[]; notices?: string[];
}
export interface ImportPreview {
  spec: PluginSpec;
  report: { status: "supported" | "partial" | "unsupported"; components: NonNullable<PluginSpec["import_issues"]> };
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

/** Optional limits and change detection of a watch; absent on ordinary automations. */
export interface WatchSettings {
  /** RFC3339 instant after which the automation is removed. */
  expires_at?: string | null;
  /** Executions allowed, checks without a model run included. */
  max_runs?: number | null;
  /** Short plain-language goal. */
  stop_when?: string | null;
  /** argv run without the model each interval. */
  check_command?: string[] | null;
}

export interface Automation extends WatchSettings {
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
  /** Who created it; absent means the user. */
  origin?: AutomationOrigin;
  /** Why a backstop paused it. */
  stopped_reason?: string | null;
}

export type AutomationOrigin = 'user' | 'agent';

/** Live run state of one chat. */
export interface ThreadRunTask {
  running: boolean;
  awaiting_approval: boolean;
}

/** An automation that continues one chat. */
export interface ThreadTaskAutomation extends WatchSettings {
  id: string;
  stopped_reason?: string | null;
  name: string;
  enabled: boolean;
  origin: AutomationOrigin;
  schedule?: AutomationSchedule | null;
  interval_mins: number;
  next_run_at: string;
  last_run_at: string | null;
  run_count: number;
  running: boolean;
}

/** One finished automation run in a chat; `seen` once reviewed or dismissed. */
export interface ThreadTaskRun {
  review_id: string;
  automation_id: string;
  name: string;
  finished_at: string;
  ok: boolean;
  summary: string;
  seen: boolean;
  /** Kept after the automation is deleted. */
  origin?: AutomationOrigin;
}

/** An automation of this chat that ended, with the reason. */
export interface AutomationRemoval {
  id: string;
  automation_id: string;
  name: string;
  origin?: AutomationOrigin;
  thread_id?: string | null;
  removed_at: string;
  /** "completed by agent", "expired", "run limit", "failed 3 times", "deleted", ... */
  reason: string;
  stop_when?: string | null;
  run_count: number;
}

/** Per-chat task feed behind the header indicator and the Workspace Tasks tab. */
export interface ThreadTasks {
  thread_id: string;
  run: ThreadRunTask;
  automations: ThreadTaskAutomation[];
  recent: ThreadTaskRun[];
  /** Programs the agent started in the background, oldest first. */
  background_tasks?: ThreadTaskBackground[];
  /** Newest first. */
  removed?: AutomationRemoval[];
}

export type BackgroundTaskState = 'running' | 'exited' | 'stopped' | 'timed_out' | 'lost';

/** One background shell task in the chat's task feed. */
export interface ThreadTaskBackground {
  id: string;
  command: string;
  cwd: string;
  state: BackgroundTaskState;
  exit_code: number | null;
  pid: number;
  started_at: string;
  ended_at: string | null;
  timeout_secs: number;
  last_lines: string[];
  /** `last_lines` is the whole output, so the full log would repeat it. */
  last_lines_complete?: boolean;
  log_bytes: number;
}

/** One page of a background task's bounded rolling log. */
export interface BackgroundTaskLog {
  id: string;
  state: BackgroundTaskState;
  text: string;
  offset: number;
  next_offset: number;
  total_bytes: number;
  oldest_retained_offset: number;
}

export interface AutomationInput extends WatchSettings {
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
  /** Stable automation name and origin copied onto the item when the run finished. */
  automation_name?: string;
  automation_origin?: AutomationOrigin;
}

export interface RunHandle {
  run_id: string;
}

/** Mirrors themis-core RunEvent (mapped by the desktop bridge). */
export type ThreadEvent =
  | { kind: 'started'; task: string; max_turns: number }
  | { kind: 'assistant_text'; text: string }
  | { kind: 'tool_started'; tool: string; summary: string; /** Readable name for an MCP tool, "server · tool". */ label?: string }
  | { kind: 'tool_finished'; tool: string; ok: boolean; output?: string }
  | { kind: 'approval_decided'; tool: string; decision: ApprovalDecision }
  | { kind: 'context_compacting' }
  | { kind: 'context_usage'; metrics: Record<string, unknown> }
  | { kind: 'context_budget'; budget: ContextBudget }
  | { kind: 'context_suspended'; progress: string }
  | { kind: 'context_checkpoint'; summary: string }
  | { kind: 'files_changed'; files: ChangedFile[] }
  /** Images, videos or PDFs the agent attached to this run's reply, in order. */
  | { kind: 'media_attached'; files: AttachedMedia[] }
  /** Who drives the chat's built-in browser, and any recording (live only). */
  | { kind: 'browser_changed'; controller: string; recording?: BrowserRecording }
  | { kind: 'incomplete'; result: string; model?: string }
  | { kind: 'finished'; result: string; model?: string }
  | { kind: 'failed'; error: string }
  | { kind: 'background_tasks_changed' }
  /** A queued message the user steered into this run reached the model. */
  | { kind: 'steered'; text: string }
  /** The server started this run with a queued message or a goal turn (live only). */
  | { kind: 'user_message'; text: string; attachments?: string[] }
  | { kind: 'queue_changed'; queue: QueuedMessage[] }
  | { kind: 'goal_changed'; goal: Goal | null }
  | { kind: 'goal_note'; outcome: GoalNoteKind; reason: string }
  /** The agent asked the user a question, or updated one still open (same id). */
  | { kind: 'question_asked'; question: Question }
  | { kind: 'question_answered'; id: string; answer: string };

/** A question the agent asked without stopping its work. */
export interface Question {
  id: string;
  run_id: string;
  question: string;
  choices?: string[];
  /** The agent's recommended answer (one of the choices, or its suggestion), which it works on meanwhile. */
  assumption?: string;
  /** One short line per answer on its tradeoff; the recommended one's says why. */
  reasons?: Record<string, string>;
  /** Nothing more could be done without the answer. */
  blocking: boolean;
  topic?: string;
  asked_at: number;
  answer?: string;
}

/** Starts the message that delivers an answer to the agent (shown as the answered box instead). */
export const ANSWER_MARKER = "[Answer to your question — from the user]";

/** A message waiting for the chat's current run to end. */
export interface QueuedMessage {
  id: string;
  text: string;
  attachments: string[];
  reasoning_effort?: string;
  /** Being edited, or refused (see `note`): the queue skips it until it is saved. */
  held: boolean;
  /** Why the server held it (it could not be sent); an edit clears it. */
  note?: string;
}

export type GoalStatus = 'running' | 'paused' | 'waiting' | 'stuck' | 'quota' | 'reached';
export type GoalNoteKind = 'reached' | 'not_yet' | 'stuck' | 'waiting' | 'quota' | 'failed' | 'small_window';

/** A chat's goal: one sentence the agent keeps working toward. */
export interface Goal {
  text: string;
  status: GoalStatus;
  reason?: string;
  /** Unix milliseconds. */
  started_at: number;
  /** Running time already counted, in milliseconds. */
  active_ms: number;
  /** Unix milliseconds since the goal last started running. */
  running_since?: number;
  turns: number;
  stalls: number;
  progress?: string;
  failures: number;
}

export interface ThreadEventEnvelope {
  thread_id: string;
  run_id: string;
  event: ThreadEvent;
}

/** Token sums the provider reported for requests Themis made. Input includes cached input; output includes reasoning. */
export interface UsageTotals {
  requests: number;
  unreported_requests: number;
  input_tokens: number;
  cached_input_tokens: number;
  output_tokens: number;
  reasoning_tokens: number;
  /** List-price estimate of the priced requests (models.dev); absent when none was priced. */
  estimated_cost_usd?: number;
  unpriced_requests: number;
}

export interface UsageBreakdown {
  /** Provider, model or chat id; null for deleted chats. */
  key: string | null;
  provider?: string;
  title?: string;
  /** The model's published price is zero. */
  free?: boolean;
  totals: UsageTotals;
}

/** How a provider bills; `windows` stays empty until a provider publishes verified limits. */
export interface ProviderBilling {
  provider: string;
  kind: "plan_windows" | "pay_per_token" | "free" | "local";
  windows: Array<{ window: string; limit_usd: number }>;
  price_source?: string;
}

export interface UsageSummary {
  available: boolean;
  scope: "themis_only";
  generated_at_ms: number;
  week_starts: "monday";
  windows: { five_hours: UsageTotals; today: UsageTotals; week: UsageTotals; month: UsageTotals };
  /** The last 30 local days, oldest first: input + output tokens per model. */
  days: Array<{ day: string; models: Record<string, number>; requests: number }>;
  by_provider: UsageBreakdown[];
  by_model: UsageBreakdown[];
  by_chat: UsageBreakdown[];
  billing: ProviderBilling[];
}

/** Byte-estimated context accounting emitted before each bounded model request. */
export interface ContextBudget {
  budget_tokens: number;
  usable_tokens: number;
  used_tokens: number;
  tool_definition_tokens: number;
  output_reserve_tokens: number;
  safety_margin_tokens: number;
  maintenance_at_tokens: number;
  compaction_at_tokens: number;
  compaction_soon: boolean;
  /** Uncorrected byte estimate behind `used_tokens`. */
  raw_estimated_tokens?: number;
  /** Provider-reported input tokens of the previous request. */
  provider_input_tokens?: number;
  /** Estimate correction in thousandths (1000 = exact). */
  calibration_ratio_milli?: number;
  breakdown: { system: number; checkpoint: number; attachments: number; conversation: number; tool_activity: number; reasoning?: number };
}

/** A readable project file one run changed. */
export interface ChangedFile {
  path: string;
  status: DiffStatus;
}

/** A recording of the built-in browser in progress. */
export interface BrowserRecording { by: 'agent' | 'user'; seconds: number; max_seconds: number }

/** A file shown under a reply; `path` is inside the project's `.themis/attachments`. */
export interface AttachedMedia {
  path: string;
  name: string;
  caption?: string;
  kind: 'image' | 'video' | 'pdf';
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
  /** Partial answer kept after the run was stopped by the user or failed. */
  interrupted?: 'stopped' | 'failed';
  tool?: { name: string; label?: string; ok?: boolean; output?: string };
  /** Context checkpoint divider; percentages of usable context before and after. */
  checkpoint?: { before?: number; after?: number };
  /** Files this run changed, shown under its final response. */
  changedFiles?: ChangedFile[];
  /** Media the agent attached, shown under its final response. */
  media?: AttachedMedia[];
  /** A message the user steered into a running run. */
  steered?: boolean;
  /** A runtime message that started goal turn N; shown as a divider, not a bubble. */
  goalTurn?: number;
  /** What the goal check concluded after a turn. */
  goalNote?: { outcome: GoalNoteKind; reason: string };
  /** A question the agent asked the user. */
  question?: Question;
}

export type HistoryItem =
  | { kind: "user"; run_id: string; text: string; attachments?: string[] }
  | { kind: "event"; envelope: ThreadEventEnvelope }
  | { kind: "legacy"; message: PersistedMessage };

/** The dialog for `approval_id` timed out and was denied; the UI removes it. */
export interface ApprovalExpired {
  thread_id: string;
  approval_id: string;
}

export interface ApprovalRequest {
  thread_id: string;
  approval_id: string;
  tool: string;
  summary: string;
  risk: RiskLevel;
  /** What "allow in this chat" covers for shell, as raw argv tokens; render as data only. */
  grant_scope?: { argv: string[]; covers_further_args: boolean };
  /** For web and browser tools: the site "allow in this chat" covers; `tools` is `browser` (every browser action), `fetch_url` or `web_search`. */
  grant_site?: { tools: string; site: string };
  /** False for destructive actions: they ask every time, so only Allow once and Deny are offered. */
  grantable?: boolean;
  /** Structured command for shell/git approvals; show this instead of the joined summary. */
  command?: {
    argv: string[];
    cwd?: string;
    background?: boolean;
    /** Set when the sandbox blocked this command and the request asks to run it outside the sandbox: why (untrusted text). */
    sandbox_blocked?: string;
  };
}

/** Tool name of a request to run a sandbox-blocked command outside the sandbox. */
export const UNSANDBOXED_TOOL = "shell_unsandboxed";

export type RuleSource = "default" | "user" | "project" | "run";

export interface EffectiveRule {
  tool: string;
  action: "allow" | "ask" | "deny";
  risk?: string | null;
  command?: string[] | null;
  source: RuleSource;
}

export interface ConfigFileIssue { message: string; line?: number | null; column?: number | null }

export interface ConfigFileStatus {
  scope: "user" | "project";
  path: string;
  status: "loaded" | "missing" | "invalid" | "unreadable";
  error?: ConfigFileIssue | null;
}

export interface SandboxBackendInfo {
  id: string;
  label: string;
  os: string;
  supported_here: boolean;
  available: boolean;
  detail: string;
}

export interface SandboxStatus {
  enabled: boolean;
  state: "on" | "off" | "unavailable" | "blocked";
  backend?: string | null;
  summary: string;
  reason: string;
  network: "allow" | "block";
  network_enforced: boolean;
  network_note?: string | null;
  writable_paths: string[];
  configured_writable_paths: string[];
  protected_paths: string[];
  secret_paths: string[];
  /** YOLO's open plan: writable everywhere except Themis' own settings. */
  open?: boolean;
  /** Whether commands may use the macOS Keychain. */
  allow_keychain?: boolean;
  backends: SandboxBackendInfo[];
  covers: string[];
  not_covered: string[];
}

export interface ChatGrant { tool: string; command?: string[]; server?: string; risk: string }

export interface EffectivePermissions {
  project_root?: string | null;
  mode?: "custom" | "yolo" | null;
  validation_error?: string | null;
  files: ConfigFileStatus[];
  approval?: {
    default: "allow" | "ask" | "deny";
    default_source: RuleSource;
    rules: EffectiveRule[];
    project_rule_count: number;
  } | null;
  sandbox?: SandboxStatus | null;
  chat?: { thread_id: string; grants_path?: string | null; grants: ChatGrant[]; warning?: string | null } | null;
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
  /** Too large to show line by line (a dataset, a generated file); listed without hunks. */
  too_large?: boolean;
}
export interface DiffState {
  available: boolean;
  reason?: string;
  files: DiffFile[];
}

/** Read-only workspace monitor data (no git writes). */
export interface BranchInfo { name: string; current: boolean; upstream: string | null; track: string | null; updated: string | null }
export interface PullRequestInfo { number: number; title: string; branch: string; state: string; draft: boolean; url: string }
export interface WorkspaceInfo {
  root: string;
  name: string;
  is_git: boolean;
  branch: string | null;
  branches: BranchInfo[];
  pull_requests: PullRequestInfo[];
  pull_requests_unavailable: string | null;
}
export interface DirEntry { name: string; path: string; is_dir: boolean; size: number | null }
export interface DirListing { path: string; entries: DirEntry[]; truncated: boolean }
export interface FileView { path: string; content: string | null; size: number; truncated: boolean; binary: boolean }

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
  max_total_turns: number;
  /** Working-context cap in tokens; 0 is Auto (the model's window, up to the Auto ceiling). */
  context_token_cap: number;
  approval_timeout_seconds: number;
  confirm_reads: boolean;
  recent_roots: string[];
  /** Max parallel agent runs across all threads (1..=16). */
  concurrency_limit: number;
  automations_enabled: boolean;
  completion_sound: boolean;
  completion_haptic: boolean;
  /** A desktop notification when a goal is reached or pauses while the app is in the background. */
  goal_notifications: boolean;
  /** Editor that opens runtime.jsonc ("" = system text editor). */
  config_editor?: string;
  /** The approval mode a new chat starts in. */
  default_approval_mode?: "custom" | "yolo";
  /** The Settings › Secrets secret OpenCode Go uses. */
  go_secret?: string;
  onboarded: boolean;
}

/** Which providers have a key stored. Values are NEVER exposed. */
export interface SecretStatus {
  go: boolean;
}

export const THREAD_EVENT_NAME = 'thread-event';
export const APPROVAL_REQUEST_NAME = 'approval-request';
export const APPROVAL_EXPIRED_NAME = 'approval-expired';
export const REVIEW_ITEM_NAME = 'review-item-added';
export const THREAD_CHANGED_NAME = 'thread-changed';

/** A thread was created, changed or removed through the shared server (CLI or another client). */
export interface ThreadChange {
  kind: 'created' | 'updated' | 'removed';
  project_root: string;
  thread_id: string;
  thread?: ThreadInfo;
}

export type McpConnections = Record<string, {
  status: "connecting" | "connected" | "failed" | "needs_sign_in" | "needs_key";
  reason?: string;
  /** needs_key: the missing key names, to add in Settings › Keys. */
  keys?: string[];
  /** Remote servers: whether this computer has a sign-in (OAuth) for it, and its address. */
  signed_in?: boolean;
  url?: string;
}>;

/** Settings › Browser: Allow (no prompt), Ask (a prompt) or Block (refused in every mode). */
export type SiteAccess = "allow" | "ask" | "block";

export interface SiteException {
  site: string;
  browsing?: SiteAccess;
  downloads?: SiteAccess;
  debugging?: SiteAccess;
}

/** The user `runtime.jsonc` `browser` block (see docs/web-tools.md). */
export interface BrowserSettings {
  enabled: boolean;
  executable?: string;
  developer_mode: boolean;
  permissions: { browsing: SiteAccess; downloads: SiteAccess; debugging: SiteAccess };
  site_exceptions: SiteException[];
  show_secrets: boolean;
  downloads_directory?: string;
  open_workspace: boolean;
}

export interface BrowserSettingsView {
  settings: BrowserSettings;
  /** What a save must name; a save is refused when runtime.jsonc changed since. */
  revision: string;
  browsers: string[];
  /** The automatic choice is the built-in engine (downloaded on first use). */
  built_in?: boolean;
  default_downloads_directory: string;
  starter_sites: string[];
}

export interface BrowserHistoryEntry {
  url: string;
  title: string;
  /** Unix seconds. */
  last_visit: number;
  visits: number;
  project: string;
}
