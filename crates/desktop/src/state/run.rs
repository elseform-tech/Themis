//! Provider setup and asynchronous event lifecycle for agent runs.

use super::integration_tools::RunApproval;
use serde_json::json;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use themis_core::runtime::{run_task_with_evidence, RunEvent, RunPolicy};
use themis_core::skills::materialize_scripts;
use themis_core::tools::ApprovalHook;

use crate::approvals::DesktopApprovalHook;
use crate::sink::EventSink;
use crate::types::{ThreadEvent, ThreadEventEnvelope};

use super::{AppState, RunSnapshot};

/// Channel capacity for the run-event pump (runs emit a handful of events).
const EVENT_CHANNEL_CAPACITY: usize = 1024;

/// Logs every gated action's final outcome (`approval/decided`: allowed or denied, by a rule,
/// a chat grant, YOLO or a person). What a person answered in a dialog is the hook's own
/// `approval/answered` event.
pub(super) struct AuditedApprovals {
    pub(super) inner: Arc<dyn ApprovalHook>,
    pub(super) context: themis_core::diagnostics::DiagnosticContext,
}
impl ApprovalHook for AuditedApprovals {
    fn denied_by_rule(&self, action: &themis_core::tools::ToolAction) -> bool {
        self.inner.denied_by_rule(action)
    }

    fn sensitive_access(
        &self,
        tool: &str,
        args: &serde_json::Value,
    ) -> Option<themis_core::tools::sensitive::SensitiveAccess> {
        self.inner.sensitive_access(tool, args)
    }

    fn approve_sensitive(
        &self,
        action: &themis_core::tools::ToolAction,
        access: &themis_core::tools::sensitive::SensitiveAccess,
    ) -> themis_core::tools::Approval {
        self.audited(action, || self.inner.approve_sensitive(action, access))
    }

    fn approve(&self, action: &themis_core::tools::ToolAction) -> themis_core::tools::Approval {
        self.audited(action, || self.inner.approve(action))
    }
}

impl AuditedApprovals {
    fn audited(
        &self,
        action: &themis_core::tools::ToolAction,
        decide: impl FnOnce() -> themis_core::tools::Approval,
    ) -> themis_core::tools::Approval {
        let started = std::time::Instant::now();
        let decision = decide();
        let _ = self.context.log.append_with_level("approval", "decided", "info", json!({
            "thread_id":self.context.thread_id,"run_id":self.context.run_id,"operation_id":self.context.run_id,"tool":action.tool,
            "risk":format!("{:?}",action.risk),"outcome":if decision.is_denied() { "denied" } else { "allowed" },"elapsed_ms":started.elapsed().as_millis()
        }), Some(&self.context.level));
        decision
    }
}

impl AppState {
    /// Warns the agent about secret-looking files the repository could take in or already
    /// holds. One bounded `git` call per run; its cost and counts go to the diagnostics.
    async fn project_secrets_note(
        &self,
        config: &themis_core::configuration::RuntimeConfig,
        root: &std::path::Path,
        thread_id: &str,
        run_id: &str,
    ) -> String {
        use themis_core::tools::sensitive::name_list;
        let matcher = config.approval.sensitive_matcher(Some(root));
        let started = std::time::Instant::now();
        let scan = tokio::task::spawn_blocking(move || matcher.scan_project())
            .await
            .ok()
            .flatten();
        let _ = self.inner.diagnostics.append_with_level(
            "approval",
            "secret_scan",
            "info",
            json!({"thread_id": thread_id, "run_id": run_id,
                "elapsed_ms": started.elapsed().as_millis(), "checked": scan.is_some(),
                "unignored": scan.as_ref().map_or(0, |scan| scan.unignored.len()),
                "tracked": scan.as_ref().map_or(0, |scan| scan.tracked.len())}),
            Some(&config.logging.level),
        );
        let Some(scan) = scan else {
            return String::new();
        };
        let mut note = String::new();
        for (files, prompt) in [
            (
                &scan.unignored,
                themis_core::prompts::DESKTOP_SECRETS_UNIGNORED,
            ),
            (&scan.tracked, themis_core::prompts::DESKTOP_SECRETS_TRACKED),
        ] {
            if !files.is_empty() {
                note.push_str(&themis_core::prompts::fill(
                    prompt,
                    &[("files", &name_list(files))],
                ));
                note.push('\n');
            }
        }
        note
    }

    /// Resolves the provider, builds tools, and spawns the run task.
    pub(super) async fn spawn_run(
        &self,
        sink: &Arc<dyn EventSink>,
        thread_id: &str,
        run_id: &str,
        snapshot: RunSnapshot,
        task: String,
        policy: RunPolicy,
    ) -> Result<(), String> {
        // What already differs from HEAD before this run, so the end of the run
        // can report only the files this turn changed.
        let changes_before = {
            let root = snapshot.work_root.clone();
            tokio::task::spawn_blocking(move || crate::diff::change_fingerprint(&root))
                .await
                .unwrap_or_default()
        };
        let stopped = self
            .inner
            .stop_flags
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(thread_id)
            .cloned()
            .ok_or_else(|| "Run is no longer active".to_owned())?;
        let run_config = &snapshot.runtime_configuration;
        let summary_model = run_config.compaction.summary_model.clone();
        // A model from `providers` in runtime.jsonc needs neither the Go key nor its catalog.
        let configured = Self::is_configured_model(run_config, &snapshot.model);
        let go_summary = summary_model
            .as_deref()
            .is_some_and(|model| !Self::is_configured_model(run_config, model));
        let endpoint_override = !configured
            && self
                .inner
                .go_base_url_override
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_some();
        let mut config = tokio::select! {
            biased;
            _ = async {
                while !stopped.load(Ordering::SeqCst) {
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
            } => return Err("Stopped by you".to_owned()),
            config = async {
                let config = Box::pin(self.provider_config_for(run_config, &snapshot.model, thread_id)).await?;
                // Unknown limits remain explicit and use conservative admission.
                // Effort selection still requires a successfully loaded catalog.
                let requires_catalog = (!configured && snapshot.reasoning_effort.is_some()) || go_summary;
                if (!configured && !endpoint_override) || requires_catalog {
                    let catalog = self.ensure_go_catalog().await;
                    if requires_catalog {catalog?;}
                }
                Ok::<_, String>(config)
            } => config?,
        };
        let wanted: Vec<&str> = std::iter::once(snapshot.model.as_str())
            .chain(summary_model.as_deref())
            .chain(run_config.subagents.model.as_deref())
            .chain(run_config.subagents.models.iter().map(String::as_str))
            .collect();
        let capacity_catalog = Box::pin(self.run_catalog(run_config, &wanted)).await;
        let entry = capacity_catalog
            .iter()
            .find(|model| model.id == snapshot.model);
        if let Some(reason) = entry.and_then(|model| model.unavailable_reason.clone()) {
            return Err(format!(
                "Model '{}' is unavailable: {reason}",
                snapshot.model
            ));
        }
        if let Some(effort) = &snapshot.reasoning_effort {
            if configured || !self.effort_offered(&snapshot.model, effort).await {
                return Err(format!(
                    "Effort '{effort}' is unavailable for model '{}'",
                    snapshot.model
                ));
            }
        }
        themis_core::providers::check_model_override(
            &capacity_catalog,
            &snapshot.model,
            &run_config.models,
        )?;
        if let Some(model) = &summary_model {
            if !capacity_catalog.iter().any(|entry| entry.id == *model) {
                return Err(format!(
                    "Summary model '{model}' is unavailable: it is not among the models OpenCode Go or the providers in runtime.jsonc offer"
                ));
            }
        }
        // The model's window, lowered by a `models` context cap.
        let context_capacity = entry.and_then(|model| model.context_tokens);
        // The window this run starts with (test endpoints report none, so the
        // configured cap stands); a goal's first turn warns when it is small.
        let window = themis_core::context_window::window(
            &snapshot.model,
            (!endpoint_override).then_some(context_capacity),
        );
        let effective_context =
            themis_core::context_window::effective_budget(policy.context_token_budget, window);
        self.warn_small_goal_window(sink.as_ref(), thread_id, run_id, effective_context);
        config.reasoning_effort = snapshot.reasoning_effort.clone();
        let llm = themis_core::providers::resolve(&config)
            .await
            .map_err(|err| format!("failed to resolve the model provider: {err:#}"))?;
        let diagnostic_context = themis_core::diagnostics::DiagnosticContext {
            log: self.inner.diagnostics.clone(),
            thread_id: thread_id.to_owned(),
            run_id: run_id.to_owned(),
            level: snapshot.runtime_configuration.logging.level.clone(),
        };
        let run_configuration = snapshot.runtime_configuration.clone();
        let short_reply = snapshot.short_reply;
        let yolo = snapshot.approval_mode == themis_core::configuration::ApprovalMode::Yolo;
        let hook = Arc::new(
            DesktopApprovalHook::new(
                thread_id.to_owned(),
                snapshot.is_git,
                Arc::clone(sink),
                Arc::clone(&self.inner.pending),
            )
            .with_timeout(std::time::Duration::from_secs(u64::from(
                snapshot.approval_timeout_seconds,
            )))
            .with_read_approval(true)
            .with_unattended(snapshot.automation_run)
            .with_audit(diagnostic_context.clone())
            .with_steer_waiting(self.steer_waiting_check(thread_id)),
        );
        // "Allow in this chat" grants persist for the chat: loaded now, extended
        // by this run's answers, and inherited by later runs (including the
        // chat's automations).
        let grants = self.chat_grants_for(thread_id);
        let subagent_grants = Arc::clone(&grants);
        // Management tools evaluate with this run's captured mode and policy and
        // ask the user through the raw interactive hook; chat grants are checked
        // and extended explicitly.
        let check_approval = RunApproval {
            mode: snapshot.approval_mode,
            policy: run_configuration.approval.clone(),
            grants: Arc::clone(&grants),
            interactive: Arc::new(AuditedApprovals {
                inner: hook.clone(),
                context: diagnostic_context.clone(),
            }),
        };
        let approvals = themis_core::configuration::configured_hook_for_project(
            snapshot.approval_mode,
            run_configuration.approval.clone(),
            hook,
            grants,
            Some(&snapshot.work_root),
        );
        let approvals: Arc<dyn ApprovalHook> = Arc::new(AuditedApprovals {
            inner: approvals,
            context: diagnostic_context.clone(),
        });
        // Materialize skill scripts into the run workroot BEFORE building
        // tools: a failure aborts the spawn (the caller resets `running`),
        // so a run never starts half-skilled.
        if stopped.load(Ordering::SeqCst) {
            return Err("Stopped by you".to_owned());
        }
        let mut skill_catalog = themis_core::skills::materialize_catalog(
            &snapshot.skill_catalog,
            &snapshot.work_root,
            run_id,
        )
        .map_err(|error| format!("failed to materialize skill catalog: {error:#}"))?;
        let workspace = json!({"project_root": snapshot.work_root}).to_string();
        let secrets_note = self
            .project_secrets_note(&run_configuration, &snapshot.work_root, thread_id, run_id)
            .await;
        skill_catalog = format!(
            "\n{}\n{}{secrets_note}{}",
            themis_core::prompts::fill(
                themis_core::prompts::DESKTOP_RUN_WORKSPACE,
                &[("workspace", &workspace)]
            ),
            // No approvals and no sandbox walls: the caution travels with the run.
            if yolo {
                format!("{}\n", themis_core::prompts::DESKTOP_RUN_YOLO)
            } else {
                String::new()
            },
            skill_catalog
        );
        materialize_scripts(&snapshot.skills, &snapshot.work_root).map_err(|e| e.to_string())?;
        // The sandbox is captured once per run like the rest of the
        // configuration. YOLO gets the open plan: no walls but the protected
        // folders, and no escalation prompt.
        let sandbox = self.sandbox_plan(
            &snapshot.work_root,
            &run_configuration.sandbox,
            yolo.then_some(snapshot.yolo_sandbox),
        );
        let _ = self.inner.diagnostics.append_with_level(
            "sandbox",
            "run_plan",
            "info",
            json!({"thread_id":thread_id,"run_id":run_id,"state":sandbox.status().state,
                "backend":sandbox.status().backend,"network":sandbox.network(),"open":sandbox.open()}),
            Some(&run_configuration.logging.level),
        );
        let mut tools = themis_core::tools::boxed_tools_for_run(
            &snapshot.work_root,
            Arc::clone(&approvals),
            themis_core::tools::ToolOptions {
                yolo,
                background: Some(
                    themis_core::tools::background::BackgroundScope::new(
                        self.background_registry(),
                        thread_id,
                    )
                    .with_run(run_id),
                ),
                cancel: Some(Arc::clone(&stopped)),
                sandbox: Some(Arc::clone(&sandbox)),
                no_escalation: false,
            },
        )
        .map_err(|err| format!("failed to build tools: {err:#}"))?;
        let store = self.plugin_store(Some(snapshot.work_root.clone()));
        store
            .materialize(&snapshot.plugins, &snapshot.work_root)
            .map_err(|e| e.to_string())?;
        let startup_rules = if yolo {
            run_configuration.approval.yolo()
        } else {
            run_configuration.approval.clone()
        };
        let session_dir = self.hook_session_dir(thread_id);
        // A chat with history that hasn't run since the app started is one being continued.
        let resumed = self
            .inner
            .chats_run
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(thread_id.to_owned())
            && !snapshot.history.is_empty();
        let mut plugin_bins = Vec::new();
        let mut hooks = Vec::new();
        let mut setup_events = Vec::new();
        let mut tool_labels = std::collections::HashMap::new();
        let mut mcp_names = std::collections::HashMap::new();
        for plugin in &snapshot.plugins {
            let resource_root = snapshot
                .work_root
                .join(".themis/plugin-files")
                .join(plugin.skill_id("resources"));
            if resource_root.join("bin").is_dir() {
                plugin_bins.push(resource_root.join("bin"));
            }
            for (name, server) in &plugin.spec.mcp {
                if !server.enabled {
                    continue;
                }
                match themis_core::diagnostics::ACTIVE
                    .scope(
                        diagnostic_context.clone(),
                        self.mcp_tools(
                            &snapshot.work_root,
                            plugin,
                            name,
                            approvals.clone(),
                            // YOLO keeps only the deny rules you wrote.
                            Some(&startup_rules),
                        ),
                    )
                    .await
                {
                    Ok((connection_tools, instructions, remote_names)) => {
                        for (wire, remote) in remote_names {
                            mcp_names.insert(
                                wire,
                                vec![
                                    format!("mcp__{name}__{remote}"),
                                    format!("mcp__plugin_{}_{name}__{remote}", plugin.spec.name),
                                ],
                            );
                        }
                        for tool in &connection_tools {
                            if let Some(remote) = mcp_remote_name(tool.name()) {
                                tool_labels
                                    .insert(tool.name().to_owned(), format!("{name} · {remote}"));
                            }
                        }
                        tools.extend(connection_tools);
                        if let Some(instructions) = instructions {
                            skill_catalog.push('\n');
                            skill_catalog.push_str(themis_core::prompts::DESKTOP_MCP_GUIDANCE);
                            skill_catalog.push(' ');
                            skill_catalog.push_str(&json!({"server":format!("{}/{name}",plugin.spec.name),"instructions":instructions}).to_string());
                        }
                    }
                    Err(error) => {
                        // Transport errors may contain remote URLs, arguments or server
                        // responses. Only expose fixed, actionable failure categories.
                        let text = error.to_string();
                        let missing = format!("{error:#}").contains("MCP program not found")
                            || text.starts_with("MCP process could not start: ");
                        let reason = match text.as_str() {
                            _ if missing => {
                                "server process could not start: its program was not found"
                            }
                            "Could not start MCP server"
                            | "MCP process could not start. Check the saved command." => {
                                "server process could not start"
                            }
                            "Required MCP environment variable is unset"
                            | "MCP authentication variable is unset"
                            | "Required MCP environment variable is unavailable." => {
                                "required authentication or environment variable is unavailable"
                            }
                            "MCP startup denied" => "startup approval was denied",
                            "MCP startup needs your approval" => {
                                "it has not been enabled by you since its definition last changed"
                            }
                            "Unsupported MCP protocol version" => "server protocol is unsupported",
                            _ => "connection or tool discovery failed",
                        };
                        let warning = themis_core::prompts::fill(
                            themis_core::prompts::DESKTOP_MCP_UNAVAILABLE,
                            &[
                                ("plugin", &plugin.spec.name),
                                ("name", name),
                                ("reason", reason),
                            ],
                        );
                        skill_catalog.push_str("\nIntegration status: ");
                        skill_catalog.push_str(&warning);
                        setup_events.push(RunEvent::ToolCallFinished {
                            tool: format!("mcp_start_{}", plugin.skill_id(name)),
                            ok: false,
                            output: warning,
                        });
                    }
                }
            }
            for hook in &plugin.spec.hooks {
                let mut hook = hook.clone();
                hook.runtime_identity = Some(plugin.skill_id(&hook.name));
                hook.plugin_root = Some(resource_root.clone());
                hook.plugin_data = Some(store.plugin_data_dir(&plugin.scope, &plugin.spec.name));
                hook.env_file = Some(themis_core::plugins::hooks::PluginEnvFile {
                    path: session_dir.join(format!("env-{}", plugin.skill_id("hooks"))),
                    prefix: themis_core::plugins::hooks::env_prefix(&plugin.spec.name),
                });
                hooks.push(hook);
            }
        }
        let mut hook_names = std::collections::HashSet::new();
        for hook in &hooks {
            if !hook_names.insert(&hook.runtime_identity) {
                return Err("Duplicate plugin hook identity".into());
            }
        }
        let mut names = std::collections::HashSet::new();
        for tool in &tools {
            if !names.insert(tool.name()) {
                return Err("Duplicate plugin tool identity".into());
            }
        }
        tools.push(Box::new(super::goal_tool::GoalTool {
            state: self.clone(),
            sink: Arc::clone(sink),
            thread_id: thread_id.to_owned(),
            run: check_approval.clone(),
        }));
        let end_request = themis_core::runtime::EndRequest::default();
        tools.push(Box::new(super::question_tool::QuestionTool {
            state: self.clone(),
            sink: Arc::clone(sink),
            thread_id: thread_id.to_owned(),
            run_id: run_id.to_owned(),
            run: check_approval.clone(),
            end: Arc::clone(&end_request),
            subagent: None,
        }));
        tools.push(Box::new(super::media_tool::AttachFileTool {
            state: self.clone(),
            sink: Arc::clone(sink),
            thread_id: thread_id.to_owned(),
            run_id: run_id.to_owned(),
            root: snapshot.work_root.clone(),
            attached: std::sync::atomic::AtomicUsize::new(0),
        }));
        // Settings › Browser for this run; "Always allow on this site" is saved for later runs.
        let remember: themis_core::tools::site_permissions::RememberSite = {
            let state = self.clone();
            Arc::new(move |site: &str, permission| {
                let site = site.to_owned();
                state
                    .update_user_browser_config(|browser| {
                        super::browser::allow_site(browser, &site, permission);
                        Ok(())
                    })
                    .map(|_| ())
            })
        };
        let site_permissions = themis_core::tools::site_permissions::SitePermissions::new(
            run_configuration.browser.clone(),
            Some(remember),
        );
        // fetch_url always; web_search through your SearXNG, or the one built into Themis.
        let mut web = run_configuration.web.clone();
        if web.search_url.is_none()
            && self.inner.built_in_search
            && super::searxng::runtime_dir().is_some()
        {
            match super::searxng::url(&self.app_data_dir()).await {
                Ok(url) => web.search_url = Some(url),
                Err(error) => {
                    let _ = self.inner.diagnostics.append(
                        "web",
                        "built_in_search_unavailable",
                        "warn",
                        json!({"reason": error}),
                    );
                }
            }
        }
        tools.extend(themis_core::tools::web::web_tools(
            approvals.clone(),
            &web,
            Some(Arc::clone(&site_permissions)),
        ));
        // The chat's built-in browser, shared with the Workspace Browser tab.
        if run_configuration.browser.enabled {
            skill_catalog.push('\n');
            skill_catalog.push_str(themis_core::prompts::DESKTOP_VERIFY_BROWSER);
            let hub = self.browser_hub();
            hub.set_executable(
                run_configuration
                    .browser
                    .executable
                    .as_ref()
                    .map(std::path::PathBuf::from),
            );
            tools.extend(themis_core::browser::tools::browser_tools(
                approvals.clone(),
                themis_core::browser::tools::BrowserScope {
                    chat: self.browser_of(thread_id, &snapshot.work_root),
                    cancel: Some(Arc::clone(&stopped)),
                    media: Some(Arc::new(super::browser::ProjectMedia {
                        root: snapshot.work_root.clone(),
                    })),
                    permissions: Arc::clone(&site_permissions),
                },
            ));
        }
        tools.extend(self.integration_management_tools_for_run(
            approvals.clone(),
            snapshot.work_root.clone(),
            thread_id.to_owned(),
            check_approval,
        ));
        // Subagents share this run's configuration, rules and grants, and its sandbox made
        // read-only; their results reach this run as notes.
        let mcp_names: themis_core::plugins::hooks::McpNames = Arc::new(mcp_names);
        let run_notes = if run_configuration.subagents.enabled {
            let kit = Arc::new(super::subagents::SubagentKit::new(
                self.clone(),
                Arc::clone(sink),
                (thread_id, run_id),
                &snapshot,
                run_configuration.clone(),
                capacity_catalog.clone(),
                endpoint_override,
                hooks.clone(),
                super::subagents::SubagentMcp {
                    names: Arc::clone(&mcp_names),
                    labels: Arc::new(tool_labels.clone()),
                    startup_rules: startup_rules.clone(),
                },
                super::subagents::SubagentWeb {
                    config: web,
                    permissions: Arc::clone(&site_permissions),
                },
                plugin_bins.clone(),
                Arc::clone(&llm),
                policy,
                &sandbox,
                subagent_grants,
                Arc::clone(&stopped),
            ));
            tools.extend(super::subagent_tools::tools(&kit));
            Arc::clone(&kit.notes)
        } else {
            themis_core::runtime::RunNotes::new()
        };
        let hook_runtime = themis_core::plugins::hooks::HookRuntime {
            hooks,
            run_id: run_id.to_owned(),
            thread_id: thread_id.to_owned(),
            root: snapshot.work_root.clone(),
            approvals: approvals.clone(),
            agent: None,
            mcp_names,
            session: themis_core::plugins::hooks::HookSession {
                transcript_path: Some(session_dir.join("transcript.jsonl")),
                permission_mode: permission_mode(snapshot.approval_mode).to_owned(),
                cancel: Some(Arc::clone(&stopped)),
                source: if resumed { "resume" } else { "startup" }.to_owned(),
                plugin_bins,
                attached_skills: snapshot
                    .skills
                    .iter()
                    .map(|skill| themis_core::plugins::hooks::skill_label(&skill.id))
                    .collect(),
                skills_seen: Arc::default(),
            },
        };
        let skills = snapshot.skills;

        let evidence_directory = super::attachments::project_storage_directory(
            &snapshot.work_root,
            &["context", thread_id],
        )?;
        // Saved originals of this chat only; the tool is read-only and bounded.
        let attachment_root = snapshot.work_root.join(".themis/attachments");
        let attachments = self
            .inner
            .transcript
            .attachment_paths(thread_id)?
            .into_iter()
            .map(std::path::PathBuf::from)
            .collect();
        // A tool every call to which would be denied is not advertised; the prompt then
        // falls back to the read_file pointer. YOLO bypasses rules.
        let recall_probe = themis_core::tools::ToolAction {
            display: None,
            command: Vec::new(),
            tool: "recall".into(),
            summary: String::new(),
            risk: themis_core::tools::RiskLevel::Read,
        };
        let recall_denied = !yolo
            && run_configuration.approval.decision(&recall_probe)
                == themis_core::configuration::PolicyAction::Deny;
        if !recall_denied {
            tools.push(Box::new(themis_core::tools::recall::RecallTool::new(
                themis_core::tools::recall::RecallScope {
                    evidence_dir: evidence_directory.clone(),
                    attachments,
                    attachment_root: Some(attachment_root),
                    project_root: snapshot.work_root.canonicalize().ok(),
                },
            )));
        }
        // Every checkpoint request carries the chat's first message (empty when unreadable).
        let first_request = self
            .inner
            .transcript
            .first_user_request(thread_id)
            .ok()
            .flatten()
            .unwrap_or_default();
        let steer_inbox = self
            .inner
            .steer_inboxes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(thread_id)
            .map(|entry| Arc::clone(&entry.inbox))
            .unwrap_or_default();
        let state = self.clone();
        let thread_id = thread_id.to_owned();
        let run_id = run_id.to_owned();
        let sink = Arc::clone(sink);
        tauri::async_runtime::spawn(async move {
            let (events_tx, mut events_rx) =
                tokio::sync::mpsc::channel::<RunEvent>(EVENT_CHANNEL_CAPACITY);
            let pump_sink = Arc::clone(&sink);
            let pump_thread = thread_id.clone();
            let pump_run = run_id.clone();
            let pump_state = state.clone();
            let pump_thread_id = thread_id.clone();
            let terminated = Arc::new(AtomicBool::new(false));
            let pump_terminated = Arc::clone(&terminated);
            let run_model = snapshot.model.clone();
            let pump_root = snapshot.work_root.clone();
            let pump = tokio::spawn(async move {
                let mut pending_text = String::new();
                while let Some(event) = events_rx.recv().await {
                    // Desktop admission already published Started before setup/preparation.
                    // Keep core's event for standalone callers without resetting live UI state.
                    if matches!(event, RunEvent::Started { .. }) {
                        continue;
                    }
                    if let RunEvent::AssistantText(delta) = &event {
                        pending_text.push_str(delta);
                        pump_sink.emit_thread_event(&ThreadEventEnvelope {
                            thread_id: pump_thread.clone(),
                            run_id: pump_run.clone(),
                            event: ThreadEvent::AssistantText {
                                text: delta.clone(),
                            },
                        });
                        if pending_text.len() < 4096 {
                            continue;
                        }
                    }
                    if !pending_text.is_empty() {
                        let envelope = ThreadEventEnvelope {
                            thread_id: pump_thread.clone(),
                            run_id: pump_run.clone(),
                            event: ThreadEvent::AssistantText {
                                text: std::mem::take(&mut pending_text),
                            },
                        };
                        if let Err(error) = pump_state.inner.transcript.append_event(&envelope) {
                            pump_state.record_error("save_thread_event", error);
                        }
                    }
                    if matches!(event, RunEvent::AssistantText(_)) {
                        continue;
                    }
                    // A terminal event means the run is over: mark the thread
                    // idle BEFORE emitting, so observers (merge/discard,
                    // provider switch) never see `Finished` on a busy thread.
                    let terminal = matches!(
                        event,
                        themis_core::runtime::RunEvent::Finished { .. }
                            | themis_core::runtime::RunEvent::Incomplete { .. }
                            | themis_core::runtime::RunEvent::Failed { .. }
                    );
                    if terminal {
                        // The run is over: its pages stop drawing until something needs them.
                        if let Some(chat) = pump_state.inner.browsers.existing(&pump_thread) {
                            if chat.freeze_pages().await {
                                let _ = pump_state.inner.diagnostics.append(
                                    "browser",
                                    "pages_frozen",
                                    "info",
                                    json!({"thread_id": pump_thread}),
                                );
                            }
                        }
                        let root = pump_root.clone();
                        let before = changes_before.clone();
                        let files = tokio::task::spawn_blocking(move || {
                            crate::diff::changed_between(
                                &root,
                                &before,
                                &crate::diff::change_fingerprint(&root),
                            )
                        })
                        .await
                        .unwrap_or_default();
                        if !files.is_empty() {
                            let changed = ThreadEventEnvelope {
                                thread_id: pump_thread.clone(),
                                run_id: pump_run.clone(),
                                event: ThreadEvent::FilesChanged { files },
                            };
                            if let Err(error) = pump_state.inner.transcript.append_event(&changed) {
                                pump_state.record_error("save_thread_event", error);
                            }
                            pump_sink.emit_thread_event(&changed);
                        }
                    }
                    let mut captured_event = ThreadEvent::from(event.clone());
                    if let ThreadEvent::ToolStarted { tool, label, .. } = &mut captured_event {
                        *label = tool_labels.get(tool.as_str()).cloned();
                    }
                    if let ThreadEvent::Finished { model, .. }
                    | ThreadEvent::Incomplete { model, .. } = &mut captured_event
                    {
                        *model = Some(run_model.clone());
                    }
                    let envelope = ThreadEventEnvelope {
                        thread_id: pump_thread.clone(),
                        run_id: pump_run.clone(),
                        event: captured_event,
                    };
                    if let Err(error) = pump_state.inner.transcript.append_event(&envelope) {
                        pump_state.record_error("save_thread_event", error);
                    }
                    if terminal {
                        pump_state.finish_run(&pump_thread_id).await;
                        pump_terminated.store(true, Ordering::SeqCst);
                        // Automation attribution runs while the thread is
                        // idle but BEFORE the terminal event goes out: the
                        // idle-before-terminal invariant is preserved, and by
                        // the time observers see the terminal event the review
                        // item (if any) already exists.
                        pump_state
                            .complete_automation_run(&pump_sink, &pump_run, &event)
                            .await;
                    }
                    pump_sink.emit_thread_event(&envelope);
                    if terminal {
                        // The next queued message or goal turn starts after
                        // observers saw this run end.
                        pump_state.spawn_after_run(
                            Arc::clone(&pump_sink),
                            pump_thread_id.clone(),
                            pump_run.clone(),
                            envelope.event,
                        );
                    }
                }
                if !pending_text.is_empty() {
                    let envelope = ThreadEventEnvelope {
                        thread_id: pump_thread.clone(),
                        run_id: pump_run.clone(),
                        event: ThreadEvent::AssistantText { text: pending_text },
                    };
                    if let Err(error) = pump_state.inner.transcript.append_event(&envelope) {
                        pump_state.record_error("save_thread_event", error);
                    }
                }
            });
            // A panicking run must neither stick the thread busy nor leave the
            // UI waiting: join the run, then synthesize the terminal event.
            let document_state = state.clone();
            let document_thread = thread_id.clone();
            let usage_recorder = state.usage_recorder(&thread_id, &run_id);
            let run_outcome = tokio::spawn(async move {
                for event in setup_events {
                    let _ = events_tx.send(event).await;
                }
                let hook_task = task.clone();
                themis_core::providers::with_catalog_limits(
                    &capacity_catalog,
                    themis_core::context_metrics::with_usage_recorder(usage_recorder, Box::pin(themis_core::diagnostics::ACTIVE.scope(
                        diagnostic_context,
                        themis_core::runtime::with_short_reply_turn(short_reply, themis_core::runtime::with_model_context_capacity(
                            if endpoint_override {
                                None
                            } else {
                                Some(context_capacity)
                            },
                            themis_core::context_metrics::with_run_policy(
                                &run_configuration.clone(),
                                themis_core::runtime::with_configuration(
                                    run_configuration.compaction,
                                    themis_core::plugins::hooks::with_hooks(
                                        hook_runtime,
                                        &hook_task,
                                        events_tx,
                                        |events_tx| {
                                            Box::pin(themis_core::context_metrics::collect(async move {
                                                themis_core::context_metrics::set_run_metadata(serde_json::json!({"thread_id":document_thread,"run_id":themis_core::diagnostics::ACTIVE.try_with(|context|context.run_id.clone()).ok(),"phase":"document_preparation","requested_context_tokens":policy.context_token_budget,"effective_context_tokens":effective_context,"provider_capacity_tokens":window.tokens,"capacity_source":window.source.as_str(),"configuration_fingerprint":themis_core::context_metrics::config_fingerprint()}));
                                                let mut history = snapshot.history;
                                                match themis_core::runtime::documents::prepare(
                                                    &llm, &snapshot.documents,
                                                    &evidence_directory, &stopped,
                                                    |wanted| document_state.inner.transcript.load_document_capsule(&document_thread,wanted).map_err(anyhow::Error::msg),
                                                    |capsule| document_state.inner.transcript.save_document_capsule(&document_thread,capsule).map_err(anyhow::Error::msg),
                                                ).await {
                                                    Ok(capsules) => history.extend(capsules.into_iter().map(|capsule|themis_core::runtime::ConversationTurn {
                                                        role:themis_core::runtime::ConversationRole::Assistant, text:capsule.context(),
                                                    })),
                                                    Err(error) => {
                                                        if let Some(metrics) = themis_core::context_metrics::snapshot() {
                                                            let _ = events_tx.send(RunEvent::ContextUsage { metrics }).await;
                                                        }
                                                        let failure_message = if stopped.load(Ordering::SeqCst) { "Stopped by you. Completed actions remain available for review.".to_owned() } else {format!("Document preparation failed: {error}; working context unchanged; any completed originals and stages retained")};
                                                        let _ = events_tx.send(RunEvent::Failed {error:failure_message}).await;
                                                        return Err(error);
                                                    }
                                                }
                                                // Boxed: the run's future is large, and the
                                                // steer scope must not copy it on the stack.
                                                themis_core::runtime::with_first_request(first_request, themis_core::runtime::with_end_request(end_request, themis_core::runtime::with_steer_inbox(steer_inbox, themis_core::runtime::with_run_notes(run_notes, Box::pin(run_task_with_evidence(
                                                llm,
                                                if yolo {
                                                    tools
                                                } else {
                                                    themis_core::skills::filter_tools(
                                                        tools, &skills,
                                                    )
                                                },
                                                themis_core::skills::compose_task(&task, &skills),
                                                history,
                                                approvals,
                                                policy,
                                                events_tx,
                                                stopped,
                                                skill_catalog,
                                                Some(evidence_directory),
                                            )))))).await
                                            }))
                                        },
                                    ),
                                ),
                            ),
                        )),
                    ),
                )))
                .await
            })
            .await;
            let _pump = pump.await;
            if !terminated.load(Ordering::SeqCst) {
                state.finish_run(&thread_id).await;
            }
            // Synthesize the terminal event only if the run died without
            // emitting one (a panic after a terminal send must not double it).
            if let Err(join_error) = run_outcome {
                if terminated.load(Ordering::SeqCst) {
                    return;
                }
                let error = format!("agent run panicked: {join_error}");
                // The pump never saw a terminal event, so attribute here; the
                // take-once registry makes a double count impossible.
                state
                    .finish_automation_run(&sink, &run_id, format!("failed: {error}"))
                    .await;
                let envelope = ThreadEventEnvelope {
                    thread_id: thread_id.clone(),
                    run_id: run_id.clone(),
                    event: ThreadEvent::Failed { error },
                };
                if let Err(error) = state.inner.transcript.append_event(&envelope) {
                    state.record_error("save_thread_event", error);
                }
                sink.emit_thread_event(&envelope);
                state.spawn_after_run(sink, thread_id, run_id, envelope.event);
            }
        });
        Ok(())
    }
}

/// App-data folder of what hooks read about a chat (`hook-sessions/<chat id>/`).
const HOOK_SESSIONS_DIR: &str = "hook-sessions";

impl AppState {
    fn hook_session_dir(&self, thread_id: &str) -> std::path::PathBuf {
        self.app_data_dir().join(HOOK_SESSIONS_DIR).join(thread_id)
    }

    /// Forgets what hooks read about deleted chats.
    pub(super) fn forget_hook_sessions(&self, thread_ids: &[String]) {
        for id in thread_ids {
            if super::chat_grants::safe_chat_id(id) {
                let _ = std::fs::remove_dir_all(self.hook_session_dir(id));
            }
        }
    }

    /// Marks a run idle, releases its concurrency permit, and persists.
    pub(super) async fn finish_run(&self, thread_id: &str) {
        self.close_steer_inbox(thread_id);
        self.stop_subagents(thread_id);
        // A recording the agent left running ends with its run, and so do its breakpoints,
        // request blocks and throttling.
        self.stop_agent_recording(thread_id).await;
        if let Some(chat) = self.inner.browsers.existing(thread_id) {
            chat.end_debugging().await;
        }
        self.inner
            .stop_flags
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(thread_id);
        // Both the terminal-event pump and run tail can finish a run; release
        // the permit only for the path that still owns the running state.
        let was_running = self
            .inner
            .threads
            .write()
            .await
            .get_mut(thread_id)
            .map(|record| std::mem::replace(&mut record.running, false))
            .unwrap_or(false);
        if was_running {
            // Keep Rust 1.98 support; the try_update replacement requires Rust 1.99.
            #[allow(deprecated)]
            let _ = self.inner.running_count.fetch_update(
                Ordering::SeqCst,
                Ordering::SeqCst,
                |count| count.checked_sub(1),
            );
        }
        self.persist_registry().await;
    }
}

/// The mode in the words Claude's hooks read.
pub(super) fn permission_mode(mode: themis_core::configuration::ApprovalMode) -> &'static str {
    match mode {
        themis_core::configuration::ApprovalMode::Yolo => "bypassPermissions",
        themis_core::configuration::ApprovalMode::Custom => "default",
    }
}

/// The server's own tool name inside an MCP wire name (`mcp_<24 hex>_<tool>`).
fn mcp_remote_name(wire: &str) -> Option<&str> {
    let rest = wire.strip_prefix("mcp_")?;
    let (identity, remote) = rest.split_at_checked(24)?;
    (identity.bytes().all(|b| b.is_ascii_hexdigit()) && !remote.is_empty())
        .then(|| remote.strip_prefix('_'))
        .flatten()
        .filter(|remote| !remote.is_empty())
}

#[cfg(test)]
mod mcp_label_tests {
    #[test]
    fn mcp_wire_names_yield_the_server_tool_name() {
        assert_eq!(
            super::mcp_remote_name("mcp_932fdceefae46ddd0ad259c8_browser_evaluate"),
            Some("browser_evaluate")
        );
        for other in [
            "shell",
            "mcp_start_docs",
            "mcp_xyz_tool",
            "mcp_932fdceefae46ddd0ad259c8_",
            "mcp_932fdceefae46ddd0ad259c8x",
        ] {
            assert_eq!(super::mcp_remote_name(other), None, "{other}");
        }
    }
}
