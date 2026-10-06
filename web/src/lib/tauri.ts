// Typed Tauri bridge client. All frontend code reaches the backend through
// these functions — never raw invoke() with stringly-typed payloads elsewhere.
import { invoke as tauriInvoke } from '@tauri-apps/api/core';
import type { PromptSkill } from './prompt';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type {
  ApprovalDecision,
  ApprovalRequest,
  Automation,
  AutomationInput,
  Diagnostics,
  DiffState,
  GoModel,
  HistoryItem,
  PersistedMessage,
  UpdateStatus,
  MergeResult,
  ProjectInfo,
  ReviewItem,
  ReviewStatus,
  Skill,
  SkillInput,
  ProviderKind,
  RunHandle,
  SecretStatus,
  Settings,
  ThreadComment,
  ThreadEventEnvelope,
  ThreadInfo,
} from './types';
import {
  APPROVAL_REQUEST_NAME,
  REVIEW_ITEM_NAME,
  THREAD_EVENT_NAME,
} from './types';

function invoke<T>(command: string, args: Record<string, unknown> = {}): Promise<T> {
  if (command === 'check_for_updates') return tauriInvoke<T>(command, args);
  return tauriInvoke<T>('backend_command', { command, args });
}

export function listPromptSkills(projectRoot?: string | null): Promise<PromptSkill[]> { return invoke("list_prompt_skills", { projectRoot: projectRoot ?? null }); }
export function pluginAction<T>(args: Record<string, unknown>): Promise<T> { return invoke("plugin_action", args); }

export function ping(): Promise<string> {
  return invoke('ping');
}

export function renameThread(threadId: string, title: string): Promise<ThreadInfo> { return invoke("rename_thread", { threadId, title }); }

export function stopThread(threadId: string): Promise<void> { return invoke("stop_thread", { threadId }); }
export function listGoModels(): Promise<GoModel[]> { return invoke("list_go_models"); }

export function createProject(name: string): Promise<ProjectInfo> {
  return invoke("create_project", { name });
}

export function renameProject(path: string, name: string): Promise<ProjectInfo> {
  return invoke("rename_project", { path, name });
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

export function sendMessage(threadId: string, text: string, reasoningEffort?: string, attachments: string[] = []): Promise<RunHandle> {
  return invoke('send_message', { threadId, text, reasoningEffort: reasoningEffort || null, attachments });
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

export function openInEditor(path: string, line?: number): Promise<void> {
  return invoke('open_in_editor', { path, line: line ?? null });
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

export function getSecretStatus(): Promise<SecretStatus> {
  return invoke('get_secret_status');
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

export function onReviewItemAdded(
  cb: (item: ReviewItem) => void,
): Promise<UnlistenFn> {
  return listen<ReviewItem>(REVIEW_ITEM_NAME, (event) => cb(event.payload));
}
