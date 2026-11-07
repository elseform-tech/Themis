//! The shared command dispatcher: one method name and its arguments in, one JSON value out.

use super::*;

/// Runs one command, then tells every subscribed window about thread
/// creation, metadata changes and removal, so an open app follows changes
/// made through the shared server (for example `themis call create_thread`).
pub async fn dispatch(
    state: &AppState,
    sink: Arc<dyn EventSink>,
    method: &str,
    args: Value,
) -> Result<Value, String> {
    let thread_id = args
        .get("threadId")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let removed_root = match (method, &thread_id) {
        ("discard_thread", Some(id)) => state.thread_project_root(id).await,
        _ => None,
    };
    let result = dispatch_command(state, Arc::clone(&sink), method, args).await;
    if let Ok(value) = &result {
        announce_thread_change(state, sink.as_ref(), method, value, thread_id, removed_root).await;
    }
    result
}

async fn announce_thread_change(
    state: &AppState,
    sink: &dyn EventSink,
    method: &str,
    value: &Value,
    thread_id: Option<String>,
    removed_root: Option<String>,
) {
    let (kind, id) = match method {
        "create_thread" => (
            ThreadChangeKind::Created,
            value.get("id").and_then(Value::as_str).map(str::to_owned),
        ),
        "rename_thread"
        | "set_thread_approval_mode"
        | "set_thread_effort"
        | "set_thread_skills" => (ThreadChangeKind::Updated, thread_id),
        "discard_thread" => {
            if let (Some(thread_id), Some(project_root)) = (thread_id, removed_root) {
                sink.emit_thread_changed(&ThreadChange {
                    kind: ThreadChangeKind::Removed,
                    project_root,
                    thread_id,
                    thread: None,
                });
            }
            return;
        }
        _ => return,
    };
    let Some(id) = id else { return };
    let (Some(project_root), Ok(thread)) = (
        state.thread_project_root(&id).await,
        state.get_thread(&id).await,
    ) else {
        return;
    };
    sink.emit_thread_changed(&ThreadChange {
        kind,
        project_root,
        thread_id: id,
        thread: Some(thread),
    });
}

/// The command names and camelCase arguments match the existing Tauri bridge.
async fn dispatch_command(
    state: &AppState,
    sink: Arc<dyn EventSink>,
    method: &str,
    args: Value,
) -> Result<Value, String> {
    use crate::types::{
        ApprovalDecision, AutomationInput, ProviderKind, ReviewStatus, SettingsPatch, SkillInput,
    };
    macro_rules! output {
        ($expr:expr) => {
            serde_json::to_value($expr).map_err(|err| err.to_string())
        };
    }
    macro_rules! done {
        ($expr:expr) => {{
            $expr?;
            Ok(Value::Null)
        }};
    }
    match method {
        "get_runtime_configuration" => state.get_runtime_configuration(arg(&args, "projectRoot")?),
        "get_effective_permissions" => {
            state
                .get_effective_permissions(arg(&args, "projectRoot")?, arg(&args, "threadId")?)
                .await
        }
        "save_runtime_configuration" => state.save_runtime_configuration(
            &arg::<String>(&args, "scope")?,
            arg(&args, "projectRoot")?,
            &arg::<String>(&args, "json")?,
        ),
        "query_diagnostic_logs" => output!(state
            .query_diagnostic_logs(serde_json::from_value(args).map_err(|e| e.to_string())?)?),
        "set_thread_approval_mode" => output!(
            state
                .set_thread_approval_mode(arg(&args, "threadId")?, arg(&args, "mode")?)
                .await?
        ),
        "plugin_action" => output!(state.plugin_action(args).await?),
        "list_prompt_skills" => output!(state.prompt_skills(arg(&args, "projectRoot")?).await?),
        "ping" => output!(state.ping().await),
        "get_browser_settings" => output!({
            let (settings, revision) = state.user_browser_view()?;
            json!({
            "settings": settings,
            "revision": revision,
            "browsers": themis_core::browser::launch::installed_browsers(),
            "built_in": state.browser_built_in(),
            "default_downloads_directory": themis_core::browser::session::default_downloads_directory(),
            "starter_sites": themis_core::configuration::STARTER_BLOCKED_SITES,
            })
        }),
        "update_browser_settings" => {
            let changes: serde_json::Map<String, Value> = arg(&args, "changes")?;
            let revision: Option<String> = arg(&args, "revision")?;
            let (settings, revision) =
                state.patch_user_browser_config(changes, revision.as_deref())?;
            output!(json!({"settings": settings, "revision": revision}))
        }
        "get_memory_settings" => output!({
            let (settings, revision) = state.user_memory_view()?;
            json!({"settings": settings, "revision": revision, "waiting": state.memory_waiting()})
        }),
        "update_memory_settings" => {
            let enabled: Option<bool> = arg(&args, "enabled")?;
            let models: Option<Vec<String>> = arg(&args, "models")?;
            let revision: Option<String> = arg(&args, "revision")?;
            let (settings, revision) =
                state.set_user_memory_settings(enabled, models, revision.as_deref())?;
            output!(json!({"settings": settings, "revision": revision}))
        }
        "auto_try_review" => {
            let project_root: String = arg(&args, "projectRoot")?;
            output!(state.auto_try_review(&project_root, &args).await?)
        }
        "memory_try_pass" => {
            let project_root: String = arg(&args, "projectRoot")?;
            output!(state.memory_try_pass(&project_root, &args).await?)
        }
        "list_memories" => {
            let scope: String = arg(&args, "scope")?;
            let project_root: Option<String> = arg(&args, "projectRoot")?;
            output!(state.list_memories(&scope, project_root.as_deref())?)
        }
        "delete_memory" => {
            let scope: String = arg(&args, "scope")?;
            let project_root: Option<String> = arg(&args, "projectRoot")?;
            let name: String = arg(&args, "name")?;
            let revision: Option<String> = arg(&args, "revision")?;
            output!(state.delete_memory(
                &scope,
                project_root.as_deref(),
                &name,
                revision.as_deref()
            )?)
        }
        "get_subagent_settings" => output!({
            let (settings, revision) = state.user_subagents_view()?;
            json!({"settings": settings, "revision": revision})
        }),
        "update_subagent_settings" => {
            let changes: serde_json::Map<String, Value> = arg(&args, "changes")?;
            let revision: Option<String> = arg(&args, "revision")?;
            let (settings, revision) =
                state.patch_user_subagents_config(changes, revision.as_deref())?;
            output!(json!({"settings": settings, "revision": revision}))
        }
        "get_web_settings" => output!(json!({
            "search_url": state.user_web_config()?.search_url,
            "built_in": crate::state::searxng::status(),
        })),
        "update_web_settings" => {
            let search_url: Option<String> = arg(&args, "searchUrl")?;
            let search_url = search_url
                .map(|url| url.trim().to_owned())
                .filter(|url| !url.is_empty());
            let web = state.update_user_web_config(|web| {
                web.search_url = search_url;
                Ok(())
            })?;
            output!(json!({"search_url": web.search_url}))
        }
        "check_search_server" => {
            let url = match arg::<Option<String>>(&args, "url")? {
                Some(url) => url,
                None => match state.user_web_config()?.search_url {
                    Some(url) => url,
                    // Yours unset: the one built into Themis (started if needed).
                    None => crate::state::searxng::url(&state.app_data_dir()).await?,
                },
            };
            output!(themis_core::tools::web::check_search_server(&url).await)
        }
        "browser_history" => output!(
            state
                .browser_history(
                    arg::<Option<String>>(&args, "projectRoot")?,
                    arg::<Option<String>>(&args, "query")?.unwrap_or_default()
                )
                .await?
        ),
        "remove_browser_history" => done!(
            state
                .remove_browser_history(arg(&args, "projectRoot")?, arg(&args, "url")?)
                .await
        ),
        "clear_browsing_data" => output!(
            state
                .clear_browsing_data(
                    arg::<Option<String>>(&args, "projectRoot")?,
                    arg(&args, "range")?,
                    arg(&args, "types")?
                )
                .await?
        ),
        "open_project" => output!(state.open_project(arg(&args, "path")?).await?),
        "rename_project" => output!(
            state
                .rename_project(arg(&args, "path")?, arg(&args, "name")?)
                .await?
        ),
        "delete_project" => done!(state.delete_project(arg(&args, "path")?).await),
        "get_default_project" => output!(state.get_default_project().await?),
        "create_project" => output!(
            state
                .create_project(arg(&args, "name")?, arg(&args, "directory")?)
                .await?
        ),
        "rename_thread" => output!(
            state
                .rename_thread(arg(&args, "threadId")?, arg(&args, "title")?)
                .await?
        ),
        "stop_thread" => done!(state.stop_thread(arg(&args, "threadId")?).await),
        "list_go_models" => output!(state.list_go_models().await?),
        "list_model_providers" => output!(state.list_model_providers().await?),
        "create_thread" => output!(
            state
                .create_thread(
                    arg(&args, "projectRoot")?,
                    arg::<ProviderKind>(&args, "provider")?,
                    arg(&args, "model")?
                )
                .await?
        ),
        "get_thread" => output!(state.get_thread(&arg::<String>(&args, "threadId")?).await?),
        "get_context_usage" => output!(
            state
                .get_context_usage(
                    &arg::<String>(&args, "threadId")?,
                    arg::<Option<String>>(&args, "runId")?.as_deref()
                )
                .await?
        ),
        "get_usage_summary" => output!(state.get_usage_summary().await?),
        "get_chat_usage" => output!(state.get_chat_usage(arg(&args, "threadId")?).await?),
        "get_thread_history" => output!(
            state
                .get_thread_history(&arg::<String>(&args, "threadId")?)
                .await?
        ),
        "get_thread_history_page" => output!(
            state
                .get_thread_history_page(
                    &arg::<String>(&args, "threadId")?,
                    arg::<Option<HistoryCursor>>(&args, "cursor")?.unwrap_or_default(),
                    arg::<Option<i64>>(&args, "throughSeq")?,
                    arg::<Option<usize>>(&args, "maxBytes")?.unwrap_or(MAX_PAGE_BYTES),
                )
                .await?
        ),
        "import_legacy_history" => done!(
            state
                .import_legacy_history(
                    &arg::<String>(&args, "threadId")?,
                    &arg::<Vec<Value>>(&args, "messages")?
                )
                .await
        ),
        "get_pending_approvals" => output!(state.pending_approvals()),
        "list_threads" => output!(state.list_threads(arg(&args, "projectRoot")?).await?),
        "merge_thread" => output!(state.merge_thread(arg(&args, "threadId")?).await?),
        "discard_thread" => done!(state.discard_thread(arg(&args, "threadId")?).await),
        "attachment_path" => output!(
            state
                .attachment_path(arg(&args, "threadId")?, arg(&args, "path")?)
                .await?
        ),
        "attach_files" => output!(
            state
                .attach_files(arg(&args, "threadId")?, arg(&args, "paths")?)
                .await?
        ),
        "send_message" => output!(
            crate::state::configuration::RUN_OVERRIDE
                .scope(
                    arg(&args, "runtimeConfiguration")?,
                    state.send_message_with_attachments(
                        sink,
                        arg(&args, "threadId")?,
                        arg(&args, "text")?,
                        arg(&args, "reasoningEffort")?,
                        serde_json::from_value(
                            args.get("attachments")
                                .cloned()
                                .unwrap_or_else(|| serde_json::json!([]))
                        )
                        .map_err(|e| e.to_string())?
                    )
                )
                .await?
        ),
        "list_diff" => output!(state.list_diff(arg(&args, "threadId")?).await?),
        "branch_diff" => output!(
            state
                .branch_diff(arg(&args, "projectRoot")?, arg(&args, "branch")?)
                .await?
        ),
        "switch_branch" => output!(
            state
                .switch_branch(arg(&args, "projectRoot")?, arg(&args, "branch")?)
                .await?
        ),
        "get_workspace" => output!(
            state
                .get_workspace(
                    arg(&args, "projectRoot")?,
                    args.get("pullRequests")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                )
                .await?
        ),
        "terminal_open" => output!(
            state
                .terminal_open(
                    arg(&args, "projectRoot")?,
                    arg(&args, "threadId").unwrap_or(None),
                    arg(&args, "cols")?,
                    arg(&args, "rows")?
                )
                .await?
        ),
        "terminal_state" => output!(state.terminal_state(&arg::<String>(&args, "root")?)),
        "terminal_input" => done!(state.terminal_input(
            &arg::<String>(&args, "root")?,
            &arg::<String>(&args, "data")?
        )),
        "terminal_resize" => done!(state.terminal_resize(
            &arg::<String>(&args, "root")?,
            arg(&args, "cols")?,
            arg(&args, "rows")?
        )),
        "terminal_close" => done!(Ok::<(), String>(
            state.terminal_close(&arg::<String>(&args, "root")?).await
        )),
        "list_project_dir" => output!(
            state
                .list_project_dir(arg(&args, "projectRoot")?, arg(&args, "path")?)
                .await?
        ),
        "read_project_file" => output!(
            state
                .read_project_file(arg(&args, "projectRoot")?, arg(&args, "path")?)
                .await?
        ),
        "accept_file" => done!(
            state
                .accept_file(arg(&args, "threadId")?, arg(&args, "path")?)
                .await
        ),
        "discard_file" => done!(
            state
                .discard_file(arg(&args, "threadId")?, arg(&args, "path")?)
                .await
        ),
        "add_comment" => output!(
            state
                .add_comment(
                    arg(&args, "threadId")?,
                    arg(&args, "path")?,
                    arg(&args, "comment")?
                )
                .await?
        ),
        "approve_action" => done!(
            state
                .approve_action(
                    arg(&args, "threadId")?,
                    arg(&args, "approvalId")?,
                    arg::<ApprovalDecision>(&args, "decision")?
                )
                .await
        ),
        "set_provider" => {
            let thread_id: String = arg(&args, "threadId")?;
            let info = state
                .set_provider(
                    thread_id.clone(),
                    arg::<ProviderKind>(&args, "provider")?,
                    arg(&args, "model")?,
                )
                .await?;
            state.goal_model_changed(sink, &thread_id).await;
            output!(info)
        }
        "list_queue" => output!(state.list_queue(arg(&args, "threadId")?).await?),
        "enqueue_message" => output!(
            state
                .enqueue_message(
                    sink,
                    arg(&args, "threadId")?,
                    arg(&args, "text")?,
                    arg(&args, "reasoningEffort")?,
                    arg::<Option<Vec<String>>>(&args, "attachments")?.unwrap_or_default(),
                )
                .await?
        ),
        "update_queued_message" => output!(
            state
                .update_queued_message(
                    sink,
                    arg(&args, "threadId")?,
                    arg(&args, "messageId")?,
                    arg(&args, "text")?,
                    arg(&args, "held")?,
                )
                .await?
        ),
        "remove_queued_message" => done!(
            state
                .remove_queued_message(sink, arg(&args, "threadId")?, arg(&args, "messageId")?)
                .await
        ),
        "steer_queued_message" => done!(
            state
                .steer_queued_message(sink, arg(&args, "threadId")?, arg(&args, "messageId")?)
                .await
        ),
        "list_questions" => output!(state.list_questions(arg(&args, "threadId")?).await?),
        "list_subagents" => output!(state.list_subagents(&arg::<String>(&args, "threadId")?)),
        "get_subagent_transcript" => output!(state.get_subagent_transcript(
            &arg::<String>(&args, "threadId")?,
            &arg::<String>(&args, "subagentId")?
        )?),
        "stop_subagent" => done!(state.stop_subagent(
            sink.as_ref(),
            &arg::<String>(&args, "threadId")?,
            &arg::<String>(&args, "subagentId")?
        )),
        "answer_question" => output!(
            state
                .answer_question(
                    sink,
                    arg(&args, "threadId")?,
                    arg(&args, "questionId")?,
                    arg(&args, "answer")?,
                )
                .await?
        ),
        "get_goal" => output!(state.get_goal(arg(&args, "threadId")?).await?),
        "create_goal" => output!(
            state
                .create_goal(sink, arg(&args, "threadId")?, arg(&args, "text")?)
                .await?
        ),
        "edit_goal" => output!(
            state
                .edit_goal(sink, arg(&args, "threadId")?, arg(&args, "text")?)
                .await?
        ),
        "pause_goal" => output!(state.pause_goal(sink, arg(&args, "threadId")?).await?),
        "resume_goal" => output!(state.resume_goal(sink, arg(&args, "threadId")?).await?),
        "remove_goal" => output!(state.remove_goal(sink, arg(&args, "threadId")?).await?),
        "set_thread_effort" => output!(
            state
                .set_thread_effort(arg(&args, "threadId")?, arg(&args, "effort")?)
                .await?
        ),
        "ensure_runtime_config" => output!(state.ensure_runtime_config()?),
        "open_runtime_config" => done!(state.open_runtime_config(arg(&args, "editor")?).await),
        "open_mcp_config" => output!(
            state
                .open_mcp_config(
                    arg(&args, "editor")?,
                    arg::<Option<bool>>(&args, "open")?.unwrap_or(true)
                )
                .await?
        ),
        "apply_mcp_config" => output!(state.apply_mcp_config().await?),
        "mcp_config_status" => output!(state.mcp_config_status()),
        "list_keys" => output!(state.list_keys(arg(&args, "projectRoot")?).await?),
        "set_key" => done!(state.set_key(
            &arg::<String>(&args, "name")?,
            &arg::<String>(&args, "value")?
        )),
        "remove_key" => done!(state.remove_key(&arg::<String>(&args, "name")?)),
        "list_editors" => output!(json!({"editors": crate::state::editors::installed_editors()})),
        "record_notification" => done!(state.record_notification(arg(&args, "record")?)),
        "get_settings" => output!(state.get_settings().await),
        "update_settings" => output!(
            state
                .update_settings(arg::<SettingsPatch>(&args, "patch")?)
                .await?
        ),
        "get_secret_status" => output!(state.get_secret_status().await?),
        "check_go_key" => output!(state.check_go_key().await?),
        "set_secret" => done!(
            state
                .set_secret(arg(&args, "provider")?, arg(&args, "value")?)
                .await
        ),
        "clear_secret" => done!(state.clear_secret(arg(&args, "provider")?).await),
        "list_skills" => output!(state.list_skills().await),
        "create_skill" => output!(
            state
                .create_skill(arg::<SkillInput>(&args, "input")?)
                .await?
        ),
        "update_skill" => output!(
            state
                .update_skill(arg(&args, "skillId")?, arg::<SkillInput>(&args, "input")?)
                .await?
        ),
        "delete_skill" => done!(state.delete_skill(arg(&args, "skillId")?).await),
        "set_thread_skills" => output!(
            state
                .set_thread_skills(arg(&args, "threadId")?, arg(&args, "skillIds")?)
                .await?
        ),
        "list_automations" => output!(state.list_automations().await),
        "browser_state" => {
            state
                .browser_state(&arg::<String>(&args, "threadId")?)
                .await
        }
        "browser_screencast" => done!(
            state
                .browser_screencast(
                    &arg::<String>(&args, "threadId")?,
                    arg(&args, "width")?,
                    arg(&args, "height")?,
                    arg(&args, "scale")?,
                    arg(&args, "resize")?,
                )
                .await
        ),
        "browser_preview" => done!(
            state
                .browser_preview(&arg::<String>(&args, "threadId")?, arg(&args, "width")?)
                .await
        ),
        "browser_screencast_stop" => {
            done!(
                state
                    .browser_screencast_stop(&arg::<String>(&args, "threadId")?)
                    .await
            )
        }
        "browser_input" => done!(
            state
                .browser_input(&arg::<String>(&args, "threadId")?, args["input"].clone())
                .await
        ),
        "browser_set_muted" => done!(
            state
                .browser_set_muted(&arg::<String>(&args, "threadId")?, arg(&args, "muted")?)
                .await
        ),
        "browser_open_url" => done!(
            state
                .browser_open_url(
                    &arg::<String>(&args, "threadId")?,
                    &arg::<String>(&args, "address")?,
                )
                .await
        ),
        "browser_navigate" => done!(
            state
                .browser_navigate(
                    &arg::<String>(&args, "threadId")?,
                    &arg::<String>(&args, "action")?,
                )
                .await
        ),
        "browser_capture" => {
            state
                .browser_capture(&arg::<String>(&args, "threadId")?)
                .await
        }
        "browser_record" => {
            state
                .browser_record(
                    &arg::<String>(&args, "threadId")?,
                    &arg::<String>(&args, "action")?,
                )
                .await
        }
        "browser_devtools_url" => {
            output!(
                state
                    .browser_devtools_url(&arg::<String>(&args, "threadId")?)
                    .await?
            )
        }
        "get_thread_tasks" => output!(
            state
                .get_thread_tasks(&arg::<String>(&args, "threadId")?)
                .await?
        ),
        "stop_background_task" => output!(
            state
                .stop_background_task(arg(&args, "threadId")?, arg(&args, "taskId")?)
                .await?
        ),
        "get_background_task_log" => output!(
            state
                .get_background_task_log(
                    arg(&args, "threadId")?,
                    arg(&args, "taskId")?,
                    arg::<Option<u64>>(&args, "offset")?,
                    arg::<Option<usize>>(&args, "maxBytes")?,
                )
                .await?
        ),
        "create_automation" => output!(
            state
                .create_automation(arg::<AutomationInput>(&args, "input")?)
                .await?
        ),
        "update_automation" => output!(
            state
                .update_automation(
                    arg(&args, "automationId")?,
                    arg::<AutomationInput>(&args, "input")?
                )
                .await?
        ),
        "delete_automation" => done!(state.delete_automation(arg(&args, "automationId")?).await),
        "set_automation_enabled" => output!(
            state
                .set_automation_enabled(arg(&args, "automationId")?, arg(&args, "enabled")?)
                .await?
        ),
        "run_automation_now" => output!(
            state
                .run_automation_now(sink, arg(&args, "automationId")?)
                .await?
        ),
        "list_review_items" => output!(
            state
                .list_review_items(arg::<Option<ReviewStatus>>(&args, "status")?)
                .await
        ),
        "dismiss_review_item" => output!(state.dismiss_review_item(arg(&args, "reviewId")?).await?),
        "continue_review_item" => {
            output!(state.continue_review_item(arg(&args, "reviewId")?).await?)
        }
        "resource_usage" => output!(state.resource_usage()),
        "get_diagnostics" => output!(
            state
                .get_diagnostics_with_depth(
                    args.get("deep").and_then(Value::as_bool).unwrap_or(false)
                )
                .await
        ),
        _ => Err(format!("unknown server method '{method}'")),
    }
}
