// Typed Tauri bridge client. All frontend code reaches the backend through
// these functions — never raw invoke() with stringly-typed payloads elsewhere.
import { convertFileSrc, invoke as tauriInvoke } from '@tauri-apps/api/core';
import type { BrowserHistoryEntry, BrowserSettings, BrowserSettingsView } from "./types";
import type { PromptSkill } from './prompt';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type {
  ApprovalDecision,
  ApprovalExpired,
  ApprovalRequest,
  Automation,
  AutomationInput,
  Diagnostics,
  DiffState,
  DirListing,
  EffectivePermissions,
  FileView,
  WorkspaceInfo,
  GoModel,
  Goal,
  Question,
  QueuedMessage,
  HistoryItem,
  PersistedMessage,
  UpdateStatus,
  MergeResult,
  ProjectInfo,
  ReviewItem,
  ReviewStatus,
  Skill,
  SkillInput,
  ModelProvider,
  ProviderKind,
  RunHandle,
  SecretStatus,
  Settings,
  ThreadComment,
  ThreadTasks,
  ThreadEventEnvelope,
  ThreadTaskBackground,
  BackgroundTaskLog,
  ThreadInfo,
  ThreadChange,
  UsageSummary,
  UsageTotals,
} from './types';
import {
  APPROVAL_EXPIRED_NAME,
  APPROVAL_REQUEST_NAME,
  REVIEW_ITEM_NAME,
  THREAD_CHANGED_NAME,
  THREAD_EVENT_NAME,
} from './types';

function invoke<T>(command: string, args: Record<string, unknown> = {}): Promise<T> {
  if (command === 'check_for_updates') return tauriInvoke<T>(command, args);
  return tauriInvoke<T>('backend_command', { command, args });
}

export function listPromptSkills(projectRoot?: string | null): Promise<PromptSkill[]> { return invoke("list_prompt_skills", { projectRoot: projectRoot ?? null }); }
export function pluginAction<T>(args: Record<string, unknown>): Promise<T> { return invoke("plugin_action", args); }

/** Read-only effective permissions (rules, sandbox, config files, chat grants), re-read on every call. */
export function openRuntimeConfig(editor?: string): Promise<void> { return invoke("open_runtime_config", editor === undefined ? {} : { editor }); }
export function listEditors(): Promise<{ editors: Array<{ id: string; name: string }> }> { return invoke("list_editors"); }
export function getEffectivePermissions(projectRoot: string | null, threadId: string | null): Promise<EffectivePermissions> { return invoke("get_effective_permissions", { projectRoot, threadId }); }

/** Themis's own CPU and memory use (app, built-in browser, tools), sampled every 5 s; null before the first sample. */
export interface ResourceGroup { group: "app" | "browser" | "tools"; cpu_percent: number; memory_bytes: number; processes: number; growing_bytes?: number }
export interface ResourceUsage { sampled_at_ms: number; cpu_percent: number; memory_bytes: number; cores: number; groups: ResourceGroup[] }
export function resourceUsage(): Promise<ResourceUsage | null> { return invoke("resource_usage"); }
/** Tokens Themis used: windows, the 30-day chart and breakdowns (see `UsageSummary`). */
export function getUsageSummary(): Promise<UsageSummary> { return invoke("get_usage_summary"); }
export function getChatUsage(threadId: string): Promise<UsageTotals> { return invoke("get_chat_usage", { threadId }); }
export function getWorkspace(projectRoot: string, pullRequests = false): Promise<WorkspaceInfo> { return invoke("get_workspace", { projectRoot, pullRequests }); }
export function listProjectDir(projectRoot: string, path: string): Promise<DirListing> { return invoke("list_project_dir", { projectRoot, path }); }
export function readProjectFile(projectRoot: string, path: string): Promise<FileView> { return invoke("read_project_file", { projectRoot, path }); }

export function ping(): Promise<string> {
  return invoke('ping');
}

export function renameThread(threadId: string, title: string): Promise<ThreadInfo> { return invoke("rename_thread", { threadId, title }); }

export function stopThread(threadId: string): Promise<void> { return invoke("stop_thread", { threadId }); }
export function listGoModels(): Promise<GoModel[]> { return invoke("list_go_models"); }
export function listModelProviders(): Promise<ModelProvider[]> { return invoke("list_model_providers"); }

export function createProject(name: string): Promise<ProjectInfo> {
  return invoke("create_project", { name });
}

export function renameProject(path: string, name: string): Promise<ProjectInfo> {
  return invoke("rename_project", { path, name });
}

/** Moves a project folder to the Trash and forgets its chats. */
export function deleteProject(path: string): Promise<void> {
  return invoke("delete_project", { path });
}

export function openProject(path: string): Promise<ProjectInfo> {
  return invoke('open_project', { path });
}

export function getDefaultProject(): Promise<ProjectInfo> {
  return invoke('get_default_project');
}

export function createThread(
  projectRoot: string,
  provider: ProviderKind,
  model?: string,
): Promise<ThreadInfo> {
  return invoke('create_thread', {
    projectRoot,
    provider,
    model: model ?? null,
  });
}

export interface Attachment { path: string; name: string; size: number }
export function attachFiles(threadId: string, paths: string[]): Promise<Attachment[]> {
  return invoke("attach_files", { threadId, paths });
}

const pendingPermissionChanges = new Map<string, Promise<ThreadInfo>>();

export async function sendMessage(threadId: string, text: string, reasoningEffort?: string, attachments: string[] = []): Promise<RunHandle> {
  while (pendingPermissionChanges.has(threadId)) await pendingPermissionChanges.get(threadId);
  return invoke('send_message', { threadId, text, reasoningEffort: reasoningEffort || null, attachments });
}

export function listQueue(threadId: string): Promise<QueuedMessage[]> {
  return invoke('list_queue', { threadId });
}

/** Queues a message while a run is going; it is sent when the run ends. */
export async function enqueueMessage(threadId: string, text: string, reasoningEffort?: string, attachments: string[] = []): Promise<QueuedMessage> {
  while (pendingPermissionChanges.has(threadId)) await pendingPermissionChanges.get(threadId);
  return invoke('enqueue_message', { threadId, text, reasoningEffort: reasoningEffort || null, attachments });
}

export function updateQueuedMessage(threadId: string, messageId: string, change: { text?: string; held?: boolean }): Promise<QueuedMessage> {
  return invoke('update_queued_message', { threadId, messageId, text: change.text ?? null, held: change.held ?? null });
}

export function removeQueuedMessage(threadId: string, messageId: string): Promise<void> {
  return invoke('remove_queued_message', { threadId, messageId });
}

/** Delivers a queued message into the running run, read before its next step. */
export function steerQueuedMessage(threadId: string, messageId: string): Promise<void> {
  return invoke('steer_queued_message', { threadId, messageId });
}

/** Answers a question the agent asked; the agent reads it now or in its next turn. */
export function answerQuestion(threadId: string, questionId: string, answer: string): Promise<Question> {
  return invoke('answer_question', { threadId, questionId, answer });
}

export function getGoal(threadId: string): Promise<Goal | null> {
  return invoke('get_goal', { threadId });
}

export function editGoal(threadId: string, text: string): Promise<Goal> {
  return invoke('edit_goal', { threadId, text });
}

export function pauseGoal(threadId: string): Promise<Goal> {
  return invoke('pause_goal', { threadId });
}

export function resumeGoal(threadId: string): Promise<Goal> {
  return invoke('resume_goal', { threadId });
}

export function removeGoal(threadId: string): Promise<null> {
  return invoke('remove_goal', { threadId });
}

export function getThreadHistory(threadId: string): Promise<HistoryItem[]> {
  return invoke('get_thread_history', { threadId });
}

export function importLegacyHistory(threadId: string, messages: PersistedMessage[]): Promise<void> {
  return invoke('import_legacy_history', { threadId, messages });
}

export function listDiff(threadId: string): Promise<DiffState> {
  return invoke('list_diff', { threadId });
}

export function acceptFile(threadId: string, path: string): Promise<void> {
  return invoke('accept_file', { threadId, path });
}

export function discardFile(threadId: string, path: string): Promise<void> {
  return invoke('discard_file', { threadId, path });
}

export function addComment(
  threadId: string,
  path: string,
  comment: string,
): Promise<ThreadComment> {
  return invoke('add_comment', { threadId, path, comment });
}

export function approveAction(
  threadId: string,
  approvalId: string,
  decision: ApprovalDecision,
): Promise<void> {
  return invoke('approve_action', { threadId, approvalId, decision });
}

export function setProvider(
  threadId: string,
  provider: ProviderKind,
  model?: string,
): Promise<ThreadInfo> {
  return invoke('set_provider', { threadId, provider, model: model ?? null });
}

export function setThreadEffort(threadId: string, effort: string): Promise<ThreadInfo> {
  return invoke('set_thread_effort', { threadId, effort: effort || null });
}

export function getThread(threadId: string): Promise<ThreadInfo> {
  return invoke('get_thread', { threadId });
}

export function listThreads(projectRoot: string): Promise<ThreadInfo[]> {
  return invoke('list_threads', { projectRoot });
}

export function mergeThread(threadId: string): Promise<MergeResult> {
  return invoke('merge_thread', { threadId });
}

export function discardThread(threadId: string): Promise<void> {
  return invoke('discard_thread', { threadId });
}

export function listSkills(): Promise<Skill[]> {
  return invoke('list_skills');
}

export function createSkill(input: SkillInput): Promise<Skill> {
  return invoke('create_skill', { input });
}

export function updateSkill(skillId: string, input: SkillInput): Promise<Skill> {
  return invoke('update_skill', { skillId, input });
}

export function deleteSkill(skillId: string): Promise<void> {
  return invoke('delete_skill', { skillId });
}

export function setThreadSkills(
  threadId: string,
  skillIds: string[],
): Promise<ThreadInfo> {
  return invoke('set_thread_skills', { threadId, skillIds });
}

export function listAutomations(): Promise<Automation[]> {
  return invoke('list_automations');
}

export function getThreadTasks(threadId: string): Promise<ThreadTasks> {
  return invoke('get_thread_tasks', { threadId });
}

export function stopBackgroundTask(threadId: string, taskId: string): Promise<ThreadTaskBackground> {
  return invoke('stop_background_task', { threadId, taskId });
}

export function getBackgroundTaskLog(threadId: string, taskId: string, offset?: number, maxBytes?: number): Promise<BackgroundTaskLog> {
  return invoke('get_background_task_log', { threadId, taskId, offset: offset ?? null, maxBytes: maxBytes ?? null });
}

export function createAutomation(input: AutomationInput): Promise<Automation> {
  return invoke('create_automation', { input });
}

export function updateAutomation(
  automationId: string,
  input: AutomationInput,
): Promise<Automation> {
  return invoke('update_automation', { automationId, input });
}

export function deleteAutomation(automationId: string): Promise<void> {
  return invoke('delete_automation', { automationId });
}

export function setAutomationEnabled(
  automationId: string,
  enabled: boolean,
): Promise<Automation> {
  return invoke('set_automation_enabled', { automationId, enabled });
}

export function runAutomationNow(
  automationId: string,
): Promise<{ automation_id: string; thread_id: string; run_id: string }> {
  return invoke('run_automation_now', { automationId });
}

export function listReviewItems(status?: ReviewStatus): Promise<ReviewItem[]> {
  return invoke('list_review_items', { status: status ?? null });
}

export function dismissReviewItem(reviewId: string): Promise<ReviewItem> {
  return invoke('dismiss_review_item', { reviewId });
}

export function continueReviewItem(reviewId: string): Promise<ThreadInfo> {
  return invoke('continue_review_item', { reviewId });
}

export function getDiagnostics(): Promise<Diagnostics> {
  return invoke('get_diagnostics');
}

export function checkForUpdates(): Promise<UpdateStatus> {
  return invoke('check_for_updates');
}

export function getSettings(): Promise<Settings> {
  return invoke('get_settings');
}

export function updateSettings(patch: Partial<Settings>): Promise<Settings> {
  return invoke('update_settings', { patch });
}

/** Settings › Browser, read from and saved to the user runtime.jsonc (`browser`). */
export function getBrowserSettings(): Promise<BrowserSettingsView> {
  return invoke('get_browser_settings');
}

/** Saves only `changes` (a cleared field as null), refused when the file changed since `revision`. */
export function updateBrowserSettings(changes: Partial<BrowserSettings>, revision: string): Promise<{ settings: BrowserSettings; revision: string }> {
  const sent = Object.fromEntries(Object.entries(changes).map(([key, value]) => [key, value === undefined ? null : value]));
  return invoke('update_browser_settings', { changes: sent, revision });
}

export function browserHistory(projectRoot: string | null, query: string): Promise<{ entries: BrowserHistoryEntry[] }> {
  return invoke('browser_history', { projectRoot, query });
}

export function removeBrowserHistory(projectRoot: string, url: string): Promise<void> {
  return invoke('remove_browser_history', { projectRoot, url });
}

export function clearBrowsingData(projectRoot: string | null, range: "hour" | "day" | "week" | "all", types: Array<"history" | "cookies" | "cache">): Promise<unknown> {
  return invoke('clear_browsing_data', { projectRoot, range, types });
}

export function getSecretStatus(): Promise<SecretStatus> {
  return invoke('get_secret_status');
}

/** What OpenCode Go says about the key in its chosen secret (nothing is generated). */
export type GoKeyCheck = { state: "works" | "rejected" | "unavailable" | "missing"; secret: string };
export function checkGoKey(): Promise<GoKeyCheck> {
  return invoke('check_go_key');
}

export function setSecret(
  provider: 'go',
  value: string,
): Promise<void> {
  return invoke('set_secret', { provider, value });
}

export function clearSecret(
  provider: 'go',
): Promise<void> {
  return invoke('clear_secret', { provider });
}

export function onThreadEvent(
  cb: (envelope: ThreadEventEnvelope) => void,
): Promise<UnlistenFn> {
  return listen<ThreadEventEnvelope>(THREAD_EVENT_NAME, (event) => cb(event.payload));
}

export function onApprovalRequest(
  cb: (request: ApprovalRequest) => void,
): Promise<UnlistenFn> {
  return listen<ApprovalRequest>(APPROVAL_REQUEST_NAME, (event) => cb(event.payload));
}

export function onApprovalExpired(
  cb: (expired: ApprovalExpired) => void,
): Promise<UnlistenFn> {
  return listen<ApprovalExpired>(APPROVAL_EXPIRED_NAME, (event) => cb(event.payload));
}

export function onReviewItemAdded(
  cb: (item: ReviewItem) => void,
): Promise<UnlistenFn> {
  return listen<ReviewItem>(REVIEW_ITEM_NAME, (event) => cb(event.payload));
}

export function onThreadChanged(
  cb: (change: ThreadChange) => void,
): Promise<UnlistenFn> {
  return listen<ThreadChange>(THREAD_CHANGED_NAME, (event) => cb(event.payload));
}

export async function attachmentFile(threadId: string, path: string): Promise<string> {
  const validated = await tauriInvoke<string>("attachment_file", { threadId, path });
  return convertFileSrc(validated);
}

export function setThreadApprovalMode(threadId: string, mode: "custom" | "yolo"): Promise<ThreadInfo> {
  const previous = pendingPermissionChanges.get(threadId);
  const request = (previous ? previous.catch(() => undefined) : Promise.resolve()).then(() => invoke<ThreadInfo>("set_thread_approval_mode", {threadId, mode}));
  const tracked = request.finally(() => { if (pendingPermissionChanges.get(threadId) === tracked) pendingPermissionChanges.delete(threadId); });
  pendingPermissionChanges.set(threadId, tracked);
  return tracked;
}

/** Whether Themis may show desktop notifications; `request` asks the system when not decided yet. */
export type NotifyPermission = "granted" | "denied" | "undetermined" | "unavailable";
export function notificationPermission(request: boolean): Promise<NotifyPermission> {
  return tauriInvoke<NotifyPermission>("notification_permission", { request });
}
/** A goal notification was clicked: open its chat. */
export function onNotificationOpened(cb: (threadId: string) => void): Promise<UnlistenFn> {
  return listen<string>("notification-opened", (event) => cb(event.payload));
}

export function onBackendResync(handler: () => void): Promise<UnlistenFn> {
  return listen('backend-resync', handler);
}
export function getPendingApprovals(): Promise<ApprovalRequest[]> {
  return invoke('get_pending_approvals');
}

/** Settings › Web: the SearXNG instance web_search uses (user runtime.jsonc). */
export interface WebSettingsView {
  search_url: string | null;
  /** The SearXNG built into Themis: whether this build has it, and its address or error. */
  built_in?: { available: boolean; url?: string | null; error?: string | null };
}
export function getWebSettings(): Promise<WebSettingsView> { return invoke("get_web_settings"); }
export function updateWebSettings(searchUrl: string): Promise<{ search_url: string | null }> { return invoke("update_web_settings", { searchUrl }); }
export interface SearchServerStatus {
  state: "working" | "degraded" | "json_off" | "rate_limited" | "unreachable" | "invalid" | "error";
  results?: number;
  failed_engines?: Array<{ engine: string; reason: string }>;
  message?: string;
}
/** One test search against `url`, or the saved server when omitted. */
export function checkSearchServer(url?: string): Promise<SearchServerStatus> { return invoke("check_search_server", url ? { url } : {}); }

/** Integrations › MCP: the state of the mcp.json copy being edited. */
export interface McpConfigStatus {
  state: "closed" | "open" | "applied" | "invalid" | "conflict" | "error";
  changed?: string[];
  line?: number;
  message?: string;
  servers?: number;
  path?: string;
}
export function openMcpConfig(editor?: string): Promise<McpConfigStatus> { return invoke("open_mcp_config", editor ? { editor } : {}); }
export function mcpConfigStatus(): Promise<McpConfigStatus> { return invoke("mcp_config_status"); }

/** Settings › Keys: a key's name, whether it is saved or found in the environment, and its users (never its value). */
export interface KeyEntry { name: string; saved: boolean; environment: boolean; used_by: string[] }
export function listKeys(projectRoot?: string | null): Promise<KeyEntry[]> { return invoke("list_keys", projectRoot ? { projectRoot } : {}); }
export function setKey(name: string, value: string): Promise<void> { return invoke("set_key", { name, value }); }
export function removeKey(name: string): Promise<void> { return invoke("remove_key", { name }); }
