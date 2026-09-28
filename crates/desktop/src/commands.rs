//! Thin `#[tauri::command]` wrappers over [`AppState`](crate::state::AppState).
//!
//! Command names and argument shapes are the web contract (`web/src/lib/tauri.ts`
//! is the caller): Tauri maps the JS `camelCase` args to these `snake_case`
//! params automatically.
//!
//! Every fallible command wraps its handler call in the `record_cmd` macro,
//! which reports the `Err` to the diagnostics ring ([`AppState::record_error`])
//! attributed with the command's own name before returning it. The only
//! commands that never fail — and therefore legitimately skip the macro —
//! are `ping`, `get_settings`, `get_secret_status`, `list_skills`,
//! `list_automations`, `list_review_items`, `get_diagnostics`, and
//! `check_for_updates` (the last reports update failures in-band via
//! [`UpdateStatus`](crate::types::UpdateStatus), never as `Err`). The
//! `tests/command_coverage.rs` test enforces this enumeration structurally.

use std::sync::Arc;

use tauri::{AppHandle, State};
use tauri_plugin_updater::UpdaterExt;

use crate::sink::{EventSink, TauriSink};
use crate::state::AppState;
use crate::transcript::HistoryItem;
use crate::types::{
    ApprovalDecision, Automation, AutomationInput, Diagnostics, DiffState, MergeResult,
    ProjectInfo, ProviderKind, ReviewItem, ReviewStatus, RunAutomationNow, RunHandle, SecretStatus,
    Settings, SettingsPatch, Skill, SkillInput, ThreadComment, ThreadInfo, UpdateState,
    UpdateStatus,
};

/// Runs a fallible handler call, recording any `Err` in the diagnostics ring
/// under the invoking command's name before returning it.
macro_rules! record_cmd {
    ($state:expr, $command:literal, $call:expr) => {
        match $call {
            Ok(value) => Ok(value),
            Err(message) => {
                $state.record_error($command, message.clone());
                Err(message)
            }
        }
    };
}

/// Health check: returns the `themis-core` version.
#[tauri::command]
pub async fn ping(state: State<'_, AppState>) -> Result<String, String> {
    Ok(state.ping().await)
}

/// Opens a project directory by typed path.
#[tauri::command]
pub async fn open_project(state: State<'_, AppState>, path: String) -> Result<ProjectInfo, String> {
    record_cmd!(state, "open_project", state.open_project(path).await)
}

/// Creates a project in the configured projects folder.
#[tauri::command]
pub async fn create_project(
    state: State<'_, AppState>,
    name: String,
    directory: Option<String>,
) -> Result<ProjectInfo, String> {
    record_cmd!(
        state,
        "create_project",
        state.create_project(name, directory).await
    )
}

/// Renames an existing thread.
#[tauri::command]
pub async fn rename_thread(
    state: State<'_, AppState>,
    thread_id: String,
    title: String,
) -> Result<ThreadInfo, String> {
    record_cmd!(
        state,
        "rename_thread",
        state.rename_thread(thread_id, title).await
    )
}

/// Requests a stop after the current action.
#[tauri::command]
pub async fn stop_thread(state: State<'_, AppState>, thread_id: String) -> Result<(), String> {
    record_cmd!(state, "stop_thread", state.stop_thread(thread_id).await)
}

/// Loads the OpenCode Go model catalog using the native credential.
#[tauri::command]
pub async fn list_go_models(
    state: State<'_, AppState>,
) -> Result<Vec<themis_core::providers::GoModel>, String> {
    record_cmd!(state, "list_go_models", state.list_go_models().await)
}

/// Creates a thread on a project directory.
#[tauri::command]
pub async fn create_thread(
    state: State<'_, AppState>,
    project_root: String,
    provider: ProviderKind,
    model: Option<String>,
) -> Result<ThreadInfo, String> {
    record_cmd!(
        state,
        "create_thread",
        state.create_thread(project_root, provider, model).await
    )
}

/// Returns the current info for a thread.
#[tauri::command]
pub async fn get_thread(
    state: State<'_, AppState>,
    thread_id: String,
) -> Result<ThreadInfo, String> {
    record_cmd!(state, "get_thread", state.get_thread(&thread_id).await)
}

#[tauri::command]
pub async fn get_thread_history(
    state: State<'_, AppState>,
    thread_id: String,
) -> Result<Vec<HistoryItem>, String> {
    record_cmd!(
        state,
        "get_thread_history",
        state.get_thread_history(&thread_id).await
    )
}

#[tauri::command]
pub async fn import_legacy_history(
    state: State<'_, AppState>,
    thread_id: String,
    messages: Vec<serde_json::Value>,
) -> Result<(), String> {
    record_cmd!(
        state,
        "import_legacy_history",
        state.import_legacy_history(&thread_id, &messages).await
    )
}

/// Lists the threads on a project directory.
#[tauri::command]
pub async fn list_threads(
    state: State<'_, AppState>,
    project_root: String,
) -> Result<Vec<ThreadInfo>, String> {
    record_cmd!(
        state,
        "list_threads",
        state.list_threads(project_root).await
    )
}

/// Merges a thread's worktree changes into the user's checkout.
#[tauri::command]
pub async fn merge_thread(
    state: State<'_, AppState>,
    thread_id: String,
) -> Result<MergeResult, String> {
    record_cmd!(state, "merge_thread", state.merge_thread(thread_id).await)
}

/// Discards a thread, removing its worktree and branch.
#[tauri::command]
pub async fn discard_thread(state: State<'_, AppState>, thread_id: String) -> Result<(), String> {
    record_cmd!(
        state,
        "discard_thread",
        state.discard_thread(thread_id).await
    )
}

/// Sends a message and spawns the run, returning its handle immediately.
#[tauri::command]
pub async fn send_message(
    app: AppHandle,
    state: State<'_, AppState>,
    thread_id: String,
    text: String,
    reasoning_effort: Option<String>,
) -> Result<RunHandle, String> {
    let sink: Arc<dyn EventSink> = Arc::new(TauriSink::new(app));
    record_cmd!(
        state,
        "send_message",
        state
            .send_message_with_effort(sink, thread_id, text, reasoning_effort)
            .await
    )
}

/// Lists the working-tree diff for a thread's project.
#[tauri::command]
pub async fn list_diff(state: State<'_, AppState>, thread_id: String) -> Result<DiffState, String> {
    record_cmd!(state, "list_diff", state.list_diff(thread_id).await)
}

/// Records acceptance of a changed file.
#[tauri::command]
pub async fn accept_file(
    state: State<'_, AppState>,
    thread_id: String,
    path: String,
) -> Result<(), String> {
    record_cmd!(
        state,
        "accept_file",
        state.accept_file(thread_id, path).await
    )
}

/// Discards a thread-made change.
#[tauri::command]
pub async fn discard_file(
    state: State<'_, AppState>,
    thread_id: String,
    path: String,
) -> Result<(), String> {
    record_cmd!(
        state,
        "discard_file",
        state.discard_file(thread_id, path).await
    )
}

/// Stores a comment pinned to a file.
#[tauri::command]
pub async fn add_comment(
    state: State<'_, AppState>,
    thread_id: String,
    path: String,
    comment: String,
) -> Result<ThreadComment, String> {
    record_cmd!(
        state,
        "add_comment",
        state.add_comment(thread_id, path, comment).await
    )
}

/// Delivers a UI decision to a pending approval dialog.
#[tauri::command]
pub async fn approve_action(
    state: State<'_, AppState>,
    thread_id: String,
    approval_id: String,
    decision: ApprovalDecision,
) -> Result<(), String> {
    record_cmd!(
        state,
        "approve_action",
        state.approve_action(thread_id, approval_id, decision).await
    )
}

/// Switches a thread's provider/model.
#[tauri::command]
pub async fn set_provider(
    state: State<'_, AppState>,
    thread_id: String,
    provider: ProviderKind,
    model: Option<String>,
) -> Result<ThreadInfo, String> {
    record_cmd!(
        state,
        "set_provider",
        state.set_provider(thread_id, provider, model).await
    )
}

#[tauri::command]
pub async fn set_thread_effort(
    state: State<'_, AppState>,
    thread_id: String,
    effort: Option<String>,
) -> Result<ThreadInfo, String> {
    record_cmd!(
        state,
        "set_thread_effort",
        state.set_thread_effort(thread_id, effort).await
    )
}

/// Opens a path in the OS default editor.
#[tauri::command]
pub async fn open_in_editor(
    state: State<'_, AppState>,
    path: String,
    line: Option<u32>,
) -> Result<(), String> {
    record_cmd!(
        state,
        "open_in_editor",
        state.open_in_editor(path, line).await
    )
}

/// Returns the current settings.
#[tauri::command]
pub async fn get_settings(state: State<'_, AppState>) -> Result<Settings, String> {
    Ok(state.get_settings().await)
}

/// Applies a settings patch and returns the new settings.
#[tauri::command]
pub async fn update_settings(
    state: State<'_, AppState>,
    patch: SettingsPatch,
) -> Result<Settings, String> {
    record_cmd!(state, "update_settings", state.update_settings(patch).await)
}

/// Reports which providers have a key stored.
#[tauri::command]
pub async fn get_secret_status(state: State<'_, AppState>) -> Result<SecretStatus, String> {
    Ok(state.get_secret_status().await)
}

/// Stores the API key for a provider.
#[tauri::command]
pub async fn set_secret(
    state: State<'_, AppState>,
    provider: String,
    value: String,
) -> Result<(), String> {
    record_cmd!(state, "set_secret", state.set_secret(provider, value).await)
}

/// Deletes the API key for a provider.
#[tauri::command]
pub async fn clear_secret(state: State<'_, AppState>, provider: String) -> Result<(), String> {
    record_cmd!(state, "clear_secret", state.clear_secret(provider).await)
}

/// Lists all skills.
#[tauri::command]
pub async fn list_skills(state: State<'_, AppState>) -> Result<Vec<Skill>, String> {
    Ok(state.list_skills().await)
}

/// Creates a skill.
#[tauri::command]
pub async fn create_skill(state: State<'_, AppState>, input: SkillInput) -> Result<Skill, String> {
    record_cmd!(state, "create_skill", state.create_skill(input).await)
}

/// Replaces a skill.
#[tauri::command]
pub async fn update_skill(
    state: State<'_, AppState>,
    skill_id: String,
    input: SkillInput,
) -> Result<Skill, String> {
    record_cmd!(
        state,
        "update_skill",
        state.update_skill(skill_id, input).await
    )
}

/// Deletes a skill, stripping its id from threads and automations.
#[tauri::command]
pub async fn delete_skill(state: State<'_, AppState>, skill_id: String) -> Result<(), String> {
    record_cmd!(state, "delete_skill", state.delete_skill(skill_id).await)
}

/// Attaches skills to an idle thread.
#[tauri::command]
pub async fn set_thread_skills(
    state: State<'_, AppState>,
    thread_id: String,
    skill_ids: Vec<String>,
) -> Result<ThreadInfo, String> {
    record_cmd!(
        state,
        "set_thread_skills",
        state.set_thread_skills(thread_id, skill_ids).await
    )
}

/// Lists all automations.
#[tauri::command]
pub async fn list_automations(state: State<'_, AppState>) -> Result<Vec<Automation>, String> {
    Ok(state.list_automations().await)
}

/// Creates an automation.
#[tauri::command]
pub async fn create_automation(
    state: State<'_, AppState>,
    input: AutomationInput,
) -> Result<Automation, String> {
    record_cmd!(
        state,
        "create_automation",
        state.create_automation(input).await
    )
}

/// Replaces an automation.
#[tauri::command]
pub async fn update_automation(
    state: State<'_, AppState>,
    automation_id: String,
    input: AutomationInput,
) -> Result<Automation, String> {
    record_cmd!(
        state,
        "update_automation",
        state.update_automation(automation_id, input).await
    )
}

/// Deletes an automation.
#[tauri::command]
pub async fn delete_automation(
    state: State<'_, AppState>,
    automation_id: String,
) -> Result<(), String> {
    record_cmd!(
        state,
        "delete_automation",
        state.delete_automation(automation_id).await
    )
}

/// Flips an automation's enabled flag.
#[tauri::command]
pub async fn set_automation_enabled(
    state: State<'_, AppState>,
    automation_id: String,
    enabled: bool,
) -> Result<Automation, String> {
    record_cmd!(
        state,
        "set_automation_enabled",
        state.set_automation_enabled(automation_id, enabled).await
    )
}

/// Manually triggers one automation run.
#[tauri::command]
pub async fn run_automation_now(
    app: AppHandle,
    state: State<'_, AppState>,
    automation_id: String,
) -> Result<RunAutomationNow, String> {
    let sink: Arc<dyn EventSink> = Arc::new(TauriSink::new(app));
    record_cmd!(
        state,
        "run_automation_now",
        state.run_automation_now(sink, automation_id).await
    )
}

/// Lists review items, optionally filtered by status.
#[tauri::command]
pub async fn list_review_items(
    state: State<'_, AppState>,
    status: Option<ReviewStatus>,
) -> Result<Vec<ReviewItem>, String> {
    Ok(state.list_review_items(status).await)
}

/// Dismisses a review item.
#[tauri::command]
pub async fn dismiss_review_item(
    state: State<'_, AppState>,
    review_id: String,
) -> Result<ReviewItem, String> {
    record_cmd!(
        state,
        "dismiss_review_item",
        state.dismiss_review_item(review_id).await
    )
}

/// Marks a review item continued and returns its thread.
#[tauri::command]
pub async fn continue_review_item(
    state: State<'_, AppState>,
    review_id: String,
) -> Result<ThreadInfo, String> {
    record_cmd!(
        state,
        "continue_review_item",
        state.continue_review_item(review_id).await
    )
}

/// Returns the diagnostics snapshot (version, OS, settings, recent errors).
#[tauri::command]
pub async fn get_diagnostics(state: State<'_, AppState>) -> Result<Diagnostics, String> {
    Ok(state.get_diagnostics().await)
}

/// Checks for app updates.
///
/// Returns `disabled` unless the updater plugin has an endpoint+pubkey pair
/// configured; update failures surface in-band as `error`, never as `Err`.
#[tauri::command]
pub async fn check_for_updates(app: AppHandle) -> Result<UpdateStatus, String> {
    if let Some(disabled) = crate::updater::disabled_status(app.config().plugins.0.get("updater")) {
        return Ok(disabled);
    }
    let updater = match app.updater() {
        Ok(updater) => updater,
        Err(err) => {
            return Ok(UpdateStatus {
                state: UpdateState::Error,
                version: None,
                notes: None,
                message: Some(format!("updater unavailable: {err}")),
            });
        }
    };
    match updater.check().await {
        Ok(Some(update)) => Ok(UpdateStatus {
            state: UpdateState::Available,
            version: Some(update.version),
            notes: update.body,
            message: None,
        }),
        Ok(None) => Ok(UpdateStatus {
            state: UpdateState::UpToDate,
            version: None,
            notes: None,
            message: None,
        }),
        Err(err) => Ok(UpdateStatus {
            state: UpdateState::Error,
            version: None,
            notes: None,
            message: Some(err.to_string()),
        }),
    }
}
