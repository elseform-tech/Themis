//! Starting a subagent: its slot, approvals, tools and hooks.

use super::*;
use crate::state::{auto_review, integration_tools, question_tool, run, subagent_tools};

impl SubagentKit {
    /// Starts one subagent on `model` (already checked against the allowed ones); it waits
    /// for a free slot before its first step.
    pub(in crate::state) fn start(
        self: &Arc<Self>,
        name: String,
        brief: String,
        model: String,
        write_paths: Vec<String>,
        profile: Option<Arc<NamedProfile>>,
        mcp: McpAccess,
    ) -> Arc<Subagent> {
        let mut web = self.web_access(profile.as_deref());
        // A list naming `Agent` where nesting doesn't reach is told, and it shows in the log.
        if (self.nested.is_some() || !self.config.subagents.allow_nested)
            && profile
                .as_ref()
                .is_some_and(|named| named.profile.names(SPAWN_TOOL))
        {
            web.left_out.push(SPAWN_TOOL.to_owned());
            let _ = self.state.inner.diagnostics.append_with_level(
                "subagents",
                "nesting_left_out",
                "info",
                json!({"thread_id": self.thread_id, "run_id": self.run_id,
                    "reason": if self.nested.is_some() { "one_level" } else { "setting" }}),
                Some(&self.config.logging.level),
            );
        }
        let parent = self.nested.as_ref().map(|nested| nested.parent_id.clone());
        let agent = Arc::new(Subagent {
            parent: parent.clone(),
            id: uuid::Uuid::new_v4().simple().to_string()[..12].to_owned(),
            name,
            task: Mutex::new(brief.clone()),
            brief,
            model,
            earlier: Mutex::new(Vec::new()),
            write_paths,
            profile,
            mcp,
            web,
            confined: self.sandbox.is_active(),
            run_id: uuid::Uuid::new_v4().to_string(),
            started: Mutex::new(Instant::now()),
            stop: Arc::new(AtomicBool::new(false)),
            inbox: SteerInbox::default(),
            tool_calls: AtomicUsize::new(0),
            progress: Mutex::new(Progress::default()),
        });
        self.notes.expect(1);
        self.started
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(Arc::clone(&agent));
        self.state.remember_subagent(&self.thread_id, &agent);
        self.publish(ThreadEvent::SubagentStarted {
            id: agent.id.clone(),
            name: agent.name.clone(),
            brief: agent.brief.clone(),
            model: agent.model.clone(),
            confined: agent.confined,
            write_paths: agent.write_paths.clone(),
            connections: agent.mcp.connections.clone(),
            web: !agent.web.tools.is_empty(),
            shell_scopes: self.shell_limit(&agent),
            started_by: self
                .nested
                .as_ref()
                .map(|nested| nested.parent_name.clone()),
            at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        });
        let _ = self.state.inner.diagnostics.append_with_level(
            "subagents",
            "started",
            "info",
            json!({"thread_id": self.thread_id, "run_id": self.run_id, "subagent_id": agent.id,
                "child_run_id": agent.run_id, "confined": agent.confined,
                "write_paths": agent.write_paths.len(),
                "mcp_tools": agent.mcp.tools.len(),
                "web_tools": agent.web.tools.len(),
                "started_by": parent,
                "profile": agent.profile.as_ref().map(|named| &named.profile.name)}),
            Some(&self.config.logging.level),
        );
        let kit = Arc::clone(self);
        let child = Arc::clone(&agent);
        tauri::async_runtime::spawn(async move { kit.run(child).await });
        agent
    }

    /// Saves and sends an event of the parent chat.
    pub(super) fn publish(&self, event: ThreadEvent) {
        let envelope = ThreadEventEnvelope {
            thread_id: self.thread_id.clone(),
            run_id: self.run_id.clone(),
            event,
        };
        if let Err(error) = self.state.inner.transcript.append_event(&envelope) {
            self.state.record_error("save_thread_event", error);
        }
        self.sink.emit_thread_event(&envelope);
    }

    /// Sends a subagent's live status line (not saved).
    pub(super) fn reporter(&self) -> Reporter {
        let sink = Arc::clone(&self.sink);
        let thread_id = self.thread_id.clone();
        let run_id = self.run_id.clone();
        Arc::new(move |agent: &Subagent| {
            sink.emit_thread_event(&ThreadEventEnvelope {
                thread_id: thread_id.clone(),
                run_id: run_id.clone(),
                event: ThreadEvent::SubagentStatus {
                    id: agent.id.clone(),
                    text: agent.progress().status,
                    tool_calls: agent.tool_calls.load(Ordering::SeqCst),
                    model: agent.progress().ran_on,
                },
            });
        })
    }

    pub(super) fn stopping(&self, agent: &Subagent) -> bool {
        agent.stop.load(Ordering::SeqCst) || self.stopped.load(Ordering::SeqCst)
    }

    pub(super) async fn run(self: Arc<Self>, agent: Arc<Subagent>) {
        let ending = match self.wait_for_slot(&agent).await {
            Some(permit) => {
                let ending = self.run_child(&agent).await;
                drop(permit);
                ending
            }
            None => Ending::stopped("Stopped before it started.".into()),
        };
        self.end(&agent, ending).await;
    }

    pub(super) async fn wait_for_slot(
        &self,
        agent: &Subagent,
    ) -> Option<tokio::sync::OwnedSemaphorePermit> {
        loop {
            if self.stopping(agent) {
                return None;
            }
            tokio::select! {
                permit = Arc::clone(&self.slots).acquire_owned() => return permit.ok(),
                _ = tokio::time::sleep(std::time::Duration::from_millis(200)) => {}
            }
        }
    }

    /// `run` as a subagent of this chat's run: the `themis` credential its commands get is bound
    /// to the project it works in and to its sandbox's network block.
    pub(super) fn bound<F: std::future::Future>(
        &self,
        run: F,
    ) -> impl std::future::Future<Output = F::Output> {
        let scope = themis_core::agent_cli::RunScope::new(&self.work_root, Some(&self.sandbox));
        themis_core::agent_cli::with_run_scope(scope, run)
    }

    pub(super) fn diagnostic_context(
        &self,
        agent: &Subagent,
    ) -> themis_core::diagnostics::DiagnosticContext {
        themis_core::diagnostics::DiagnosticContext {
            log: self.state.inner.diagnostics.clone(),
            thread_id: self.thread_id.clone(),
            run_id: agent.run_id.clone(),
            level: self.config.logging.level.clone(),
        }
    }

    /// Rules first, then the chat's grants, then what was allowed for this subagent alone:
    /// "Allow in this chat" on a subagent's request covers only that subagent.
    pub(super) fn approvals(&self, agent: &Subagent) -> Arc<dyn ApprovalHook> {
        let context = self.diagnostic_context(agent);
        let dialog = DesktopApprovalHook::new(
            self.thread_id.clone(),
            self.is_git,
            Arc::clone(&self.sink),
            Arc::clone(&self.state.inner.pending),
        )
        .with_timeout(self.approval_timeout)
        .with_read_approval(true)
        .with_unattended(self.unattended)
        .with_audit(context.clone())
        .with_subagent(&agent.id, &agent.name);
        let own = Arc::new(OwnGrants {
            inner: Arc::new(dialog),
            grants: ChatGrants::in_memory(),
        });
        // Auto mode reviews a subagent's actions the same way, against the person's words.
        let reviewer = (self.mode == ApprovalMode::Auto).then(|| {
            Arc::new(auto_review::DesktopReviewer {
                state: self.state.clone(),
                sink: Arc::clone(&self.sink),
                thread_id: self.thread_id.clone(),
                run_id: self.run_id.clone(),
                project_root: self.work_root.clone(),
                config: self.config.clone(),
                is_git: self.is_git,
            }) as Arc<dyn themis_core::tools::AutoReviewer>
        });
        let configured = themis_core::configuration::configured_hook_with_reviewer(
            self.mode,
            self.config.approval.clone(),
            own,
            Arc::clone(&self.grants),
            Some(&self.work_root),
            reviewer,
        );
        // A shell held to named commands refuses the others before anything can allow them, in
        // YOLO too: this is the outermost answer, and a call reaches its tool only through it.
        // A subagent another one started is held to both lists.
        let inherited = self
            .nested
            .as_ref()
            .map(|nested| nested.shell_scopes.clone())
            .unwrap_or_default();
        let own = agent.shell_scopes().to_vec();
        let inner = [own, inherited]
            .into_iter()
            .filter(|scopes| !scopes.is_empty())
            .fold(configured, |inner, scopes| -> Arc<dyn ApprovalHook> {
                Arc::new(ScopedShell {
                    inner,
                    scopes,
                    context: context.clone(),
                })
            });
        Arc::new(AuditedApprovals { inner, context })
    }

    pub(super) fn tools(
        &self,
        agent: &Arc<Subagent>,
        approvals: Arc<dyn ApprovalHook>,
        mcp: Vec<Box<dyn ToolT>>,
    ) -> Result<Vec<Box<dyn ToolT>>, String> {
        // Checked again on every start: a path may have become a link since it was given.
        for path in &agent.write_paths {
            themis_core::sandbox::normalize_write_path(
                &self.work_root,
                path,
                self.sandbox.protected(),
            )?;
        }
        let owned: Vec<PathBuf> = agent
            .write_paths
            .iter()
            .map(|path| self.work_root.join(path))
            .collect();
        let approvals_for_web = Arc::clone(&approvals);
        // Folders exist from the start, so an OS sandbox can allow them.
        for path in agent.write_paths.iter().filter(|path| path.ends_with('/')) {
            let _ = std::fs::create_dir_all(self.work_root.join(path));
        }
        let tools = themis_core::tools::boxed_tools_for_run(
            &self.work_root,
            approvals,
            themis_core::tools::ToolOptions {
                yolo: self.mode == ApprovalMode::Yolo,
                // Only when its profile names them, and only its own.
                background: names(agent, "background_task").then(|| {
                    themis_core::tools::background::BackgroundScope::new(
                        self.state.background_registry(),
                        &self.thread_id,
                    )
                    .with_run(&agent.run_id)
                    .own_tasks_only()
                }),
                cancel: Some(Arc::clone(&agent.stop)),
                sandbox: Some(Arc::new(self.sandbox.confined_to(&self.work_root, &owned))),
                no_escalation: true,
            },
        )
        .map_err(|error| format!("failed to build tools: {error:#}"))?;
        let mut web = themis_core::tools::web::web_tools(
            Arc::clone(&approvals_for_web),
            &self.web.config,
            Some(Arc::clone(&self.web.permissions)),
        );
        web.retain(|tool| agent.web.tools.iter().any(|wanted| wanted == tool.name()));
        let mut named: Vec<Box<dyn ToolT>> = Vec::new();
        if names(agent, "ask_user") {
            named.push(Box::new(question_tool::QuestionTool {
                state: self.state.clone(),
                sink: Arc::clone(&self.sink),
                thread_id: self.thread_id.clone(),
                run_id: self.run_id.clone(),
                run: integration_tools::RunApproval {
                    mode: self.mode,
                    policy: self.config.approval.clone(),
                    interactive: Arc::clone(&approvals_for_web),
                    grants: Arc::clone(&self.grants),
                },
                end: themis_core::runtime::EndRequest::default(),
                subagent: Some((agent.id.clone(), agent.name.clone())),
            }));
        }
        Ok(tools
            .into_iter()
            .filter(|tool| {
                READ_ONLY_TOOLS.contains(&tool.name())
                    || (!agent.write_paths.is_empty() && WRITE_TOOLS.contains(&tool.name()))
                    || (tool.name() == "background_task" && names(agent, "background_task"))
            })
            // A profile's list can only take away from the above.
            .filter(|tool| {
                agent
                    .profile
                    .as_ref()
                    .is_none_or(|named| named.profile.allows(tool.name()))
            })
            // Chosen when it started; they are no file permission, so none of the above applies.
            .chain(named)
            .chain(mcp.into_iter().chain(web).map(|tool| {
                Box::new(subagent_tools::Stoppable {
                    inner: tool,
                    stop: Arc::clone(&agent.stop),
                }) as Box<dyn ToolT>
            }))
            // Started by another subagent: nothing that one doesn't have.
            .filter(|tool| {
                self.nested
                    .as_ref()
                    .is_none_or(|nested| nested.tools.iter().any(|name| name == tool.name()))
            })
            .map(|tool| {
                Box::new(subagent_tools::StatusField {
                    inner: tool,
                    agent: Arc::clone(agent),
                    report: self.reporter(),
                }) as Box<dyn ToolT>
            })
            .collect())
    }

    pub(super) fn hook_runtime(
        &self,
        agent: &Subagent,
        approvals: Arc<dyn ApprovalHook>,
    ) -> themis_core::plugins::hooks::HookRuntime {
        themis_core::plugins::hooks::HookRuntime {
            hooks: self.hooks.clone(),
            run_id: agent.run_id.clone(),
            thread_id: self.thread_id.clone(),
            root: self.work_root.clone(),
            approvals,
            agent: Some(json!({"id": agent.id, "name": agent.name})),
            mcp_names: Arc::clone(&self.mcp.names),
            session: themis_core::plugins::hooks::HookSession {
                transcript_path: self.transcript_path(agent),
                permission_mode: run::permission_mode(self.mode).to_owned(),
                cancel: Some(Arc::clone(&agent.stop)),
                open: None,
                model: agent.model.clone(),
                prompt_model: Some(self.prompt_model.clone()),
                plugin_bins: self.plugin_bins.clone(),
                attached_skills: Vec::new(),
                skills_seen: Arc::default(),
                // The chat's own run picks up the note, or wakes for it when it ends.
                on_rewake: None,
            },
        }
    }
}

/// Refuses a shell call running anything its profile's `Bash(...)` entries don't name.
pub(super) struct ScopedShell {
    pub(super) inner: Arc<dyn ApprovalHook>,
    pub(super) scopes: Vec<String>,
    pub(super) context: themis_core::diagnostics::DiagnosticContext,
}

impl ScopedShell {
    pub(super) fn refuses(&self, action: &ToolAction) -> bool {
        action.tool == "shell"
            && !themis_core::tools::shell_scope::command_in_scope(&action.command, &self.scopes)
    }
}

impl ApprovalHook for ScopedShell {
    fn denied_by_rule(&self, action: &ToolAction) -> bool {
        self.refuses(action) || self.inner.denied_by_rule(action)
    }

    fn sensitive_access(
        &self,
        tool: &str,
        args: &Value,
    ) -> Option<themis_core::tools::sensitive::SensitiveAccess> {
        self.inner.sensitive_access(tool, args)
    }

    fn approve_sensitive(
        &self,
        action: &ToolAction,
        access: &themis_core::tools::sensitive::SensitiveAccess,
    ) -> Approval {
        if self.refuses(action) {
            return self.approve(action);
        }
        self.inner.approve_sensitive(action, access)
    }

    fn would_ask(&self, action: &ToolAction) -> bool {
        !self.refuses(action) && self.inner.would_ask(action)
    }

    fn approve(&self, action: &ToolAction) -> Approval {
        if self.refuses(action) {
            // Metadata only: never the command.
            let _ = self.context.log.append_with_level(
                "subagents",
                "shell_scope_refused",
                "info",
                json!({"thread_id": self.context.thread_id, "run_id": self.context.run_id,
                    "scopes": self.scopes.len()}),
                Some(&self.context.level),
            );
            return Approval::Deny;
        }
        self.inner.approve(action)
    }
}

/// Keeps a subagent's own "allow in this chat" answers to that subagent.
pub(super) struct OwnGrants {
    inner: Arc<dyn ApprovalHook>,
    grants: Arc<ChatGrants>,
}

impl ApprovalHook for OwnGrants {
    fn denied_by_rule(&self, action: &ToolAction) -> bool {
        self.inner.denied_by_rule(action)
    }

    fn would_ask(&self, action: &ToolAction) -> bool {
        !self.grants.covers(action) && self.inner.would_ask(action)
    }

    fn approve(&self, action: &ToolAction) -> Approval {
        if self.grants.covers(action) {
            return Approval::AllowOnce;
        }
        match self.inner.approve(action) {
            Approval::AllowAlways => {
                if action.grantable() {
                    self.grants.remember(action);
                }
                Approval::AllowOnce
            }
            decision => decision,
        }
    }

    /// Asks past a grant this subagent has for the tool or command alone, and remembers each
    /// file with it, in the same place: "allow in this chat" stays its own.
    fn approve_sensitive(
        &self,
        action: &ToolAction,
        access: &themis_core::tools::sensitive::SensitiveAccess,
    ) -> Approval {
        let files = access.file_actions();
        let command = themis_core::tools::is_command_tool(&action.tool);
        if files.iter().all(|file| self.grants.covers(file))
            && (!command || self.grants.covers(action))
        {
            return Approval::AllowOnce;
        }
        match self.inner.approve_sensitive(action, access) {
            Approval::AllowAlways => {
                if action.grantable() {
                    for file in &files {
                        self.grants.remember(file);
                    }
                    if command {
                        self.grants.remember(action);
                    }
                }
                Approval::AllowOnce
            }
            decision => decision,
        }
    }
}
