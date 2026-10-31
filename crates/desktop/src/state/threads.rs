use super::*;

/// In the error of a send refused because the chat already runs.
pub(super) const BUSY: &str = "is busy";
/// In the error of a send refused by the global concurrency limit.
pub(super) const CONCURRENCY_LIMIT: &str = GATE_SATURATED_MARKER;

/// Whether a send error means "try again later" rather than a failure.
pub(super) fn send_must_wait(error: &str) -> bool {
    error.contains(BUSY) || error.contains(CONCURRENCY_LIMIT)
}

/// The context cap a run applies before clamping to the model's window: the
/// resolved configuration's `context_token_budget` when a file sets it, else
/// the app setting (`0` is Auto).
pub(super) fn context_cap(
    configuration: &themis_core::configuration::RuntimeConfig,
    settings: &crate::types::Settings,
) -> usize {
    use themis_core::configuration::ContextTokenBudget;
    configuration
        .context_token_budget
        .unwrap_or(match settings.context_token_cap {
            0 => ContextTokenBudget::Auto,
            tokens => ContextTokenBudget::Tokens(tokens as usize),
        })
        .cap()
}

impl AppState {
    /// Threads share the project checkout, including existing uncommitted files.
    pub async fn create_thread(
        &self,
        project_root: String,
        provider: ProviderKind,
        model: Option<String>,
    ) -> Result<ThreadInfo, String> {
        let root = canonical_project_dir(&project_root)?;
        let settings = self.inner.settings.get().await;
        let saved_default = settings.default_model;
        let model = match model.filter(|name| !name.trim().is_empty()) {
            // A model chosen for this thread stands; a run on it reports if it is gone.
            Some(chosen) if chosen != saved_default && chosen != GO_DEFAULT_MODEL => chosen,
            // A default from a provider in runtime.jsonc stands (a run reports if it is gone).
            Some(chosen)
                if self
                    .user_providers()
                    .iter()
                    .any(|provider| chosen.starts_with(&format!("{}/", provider.id))) =>
            {
                chosen
            }
            // The saved or built-in default may have been retired by Go since it was saved.
            chosen => usable_model(
                &self.inner.go_catalog.read().await,
                chosen.unwrap_or_else(|| GO_DEFAULT_MODEL.to_owned()),
            ),
        };
        let is_git = is_git_repo(&root);
        let id = uuid::Uuid::new_v4().to_string();
        let preexisting = git_status_names(&root).unwrap_or_default();
        let head = git_head(&root);
        let record = ThreadRecord {
            id,
            title: UNTITLED_THREAD.to_owned(),
            project_root: root.clone(),
            is_git,
            provider,
            model,
            reasoning_effort: None,
            approval_mode: settings.default_approval_mode,
            running: false,
            titled: false,
            preexisting,
            head,
            accepted: HashSet::new(),
            comments: Vec::new(),
            worktree_path: None,
            branch: None,
            base_branch: None,
            recovered: false,
            merged: false,
            broken: false,
            skill_ids: Vec::new(),
        };
        let info = thread_info(&record, &self.inner.transcript)?;
        self.inner
            .threads
            .write()
            .await
            .insert(record.id.clone(), record);
        // Auto-register so a thread can be created on any valid directory,
        // even if `open_project` was skipped.
        let key = root.to_string_lossy().into_owned();
        self.inner
            .projects
            .write()
            .await
            .entry(key.clone())
            .or_insert_with(|| ProjectInfo {
                root: key,
                name: project_name(&root),
                is_git,
                is_default: false,
            });
        self.persist_registry().await;
        Ok(info)
    }

    /// Project root key under which `thread_id` is listed, if the thread exists.
    pub async fn thread_project_root(&self, thread_id: &str) -> Option<String> {
        let threads = self.inner.threads.read().await;
        threads
            .get(thread_id)
            .map(|record| record.project_root.to_string_lossy().into_owned())
    }

    /// Returns the current info for `thread_id`.
    pub async fn get_thread(&self, thread_id: &str) -> Result<ThreadInfo, String> {
        let threads = self.inner.threads.read().await;
        threads
            .get(thread_id)
            .ok_or_else(|| format!("unknown thread '{thread_id}'"))
            .and_then(|record| thread_info(record, &self.inner.transcript))
    }

    pub async fn get_thread_history(&self, thread_id: &str) -> Result<Vec<HistoryItem>, String> {
        self.get_thread(thread_id).await?;
        self.inner.transcript.history(thread_id)
    }

    pub async fn get_thread_history_page(
        &self,
        thread_id: &str,
        cursor: crate::transcript::history_pages::HistoryCursor,
        through_seq: Option<i64>,
        max_bytes: usize,
    ) -> Result<crate::transcript::history_pages::HistoryPage, String> {
        self.get_thread(thread_id).await?;
        self.inner
            .transcript
            .history_page(thread_id, cursor, through_seq, max_bytes)
    }

    pub async fn get_context_usage(
        &self,
        thread_id: &str,
        run_id: Option<&str>,
    ) -> Result<serde_json::Value, String> {
        self.get_thread(thread_id).await?;
        self.inner.transcript.context_usage(thread_id, run_id)
    }

    pub async fn import_legacy_history(
        &self,
        thread_id: &str,
        messages: &[serde_json::Value],
    ) -> Result<(), String> {
        self.get_thread(thread_id).await?;
        self.inner.transcript.import_legacy(thread_id, messages)
    }

    /// Lists the threads on `project_root` (empty when the project has none).
    pub async fn list_threads(&self, project_root: String) -> Result<Vec<ThreadInfo>, String> {
        let root = canonical_project_dir(&project_root)?;
        let threads = self.inner.threads.read().await;
        let mut infos: Vec<ThreadInfo> = threads
            .values()
            .filter(|record| record.project_root == root)
            .map(|record| thread_info(record, &self.inner.transcript))
            .collect::<Result<_, _>>()?;
        infos.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(infos)
    }

    /// Sends a message: rejects busy threads and a saturated global run gate,
    /// resolves the provider, and spawns the run on the async runtime,
    /// returning its handle immediately.
    pub async fn send_message(
        &self,
        sink: Arc<dyn EventSink>,
        thread_id: String,
        text: String,
    ) -> Result<RunHandle, String> {
        let run_id = uuid::Uuid::new_v4().to_string();
        self.send_message_with_id(sink, thread_id, text, run_id)
            .await
    }

    /// Sends a message with a caller-chosen `run_id` (the automation
    /// scheduler pre-registers its completion mapping under this id before
    /// spawning, so even an instantly-failing run is attributed).
    pub(super) async fn send_message_with_id(
        &self,
        sink: Arc<dyn EventSink>,
        thread_id: String,
        text: String,
        run_id: String,
    ) -> Result<RunHandle, String> {
        self.send_message_with_options(sink, thread_id, text, run_id, None, Vec::new())
            .await
    }

    /// Sends a user message with an optional provider reasoning effort.
    pub async fn send_message_with_effort(
        &self,
        sink: Arc<dyn EventSink>,
        thread_id: String,
        text: String,
        effort: Option<String>,
    ) -> Result<RunHandle, String> {
        self.send_message_with_attachments(sink, thread_id, text, effort, Vec::new())
            .await
    }

    pub async fn send_message_with_attachments(
        &self,
        sink: Arc<dyn EventSink>,
        thread_id: String,
        text: String,
        effort: Option<String>,
        attachments: Vec<String>,
    ) -> Result<RunHandle, String> {
        self.send_message_with_options(
            sink,
            thread_id,
            text,
            uuid::Uuid::new_v4().to_string(),
            effort,
            attachments,
        )
        .await
    }

    pub(super) async fn send_message_with_options(
        &self,
        sink: Arc<dyn EventSink>,
        thread_id: String,
        text: String,
        run_id: String,
        reasoning_effort: Option<String>,
        attachments: Vec<String>,
    ) -> Result<RunHandle, String> {
        if themis_core::goal::parse_goal_command(&text).is_some_and(|goal| goal.is_empty()) {
            return Err("Add the goal after /goal, for example: /goal speed up p95".to_owned());
        }
        let automation_run = self
            .inner
            .automation_runs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(&run_id);
        let settings = self.inner.settings.get().await;
        let mut history = Vec::new();
        let project_root = {
            let threads = self.inner.threads.read().await;
            threads
                .get(&thread_id)
                .ok_or_else(|| format!("unknown thread '{thread_id}'"))?
                .project_root
                .clone()
        };
        let sources =
            super::attachments::project_storage_directory(&project_root, &["context", &thread_id])?;
        let source_state = self.clone();
        let source_thread = thread_id.clone();
        tokio::task::spawn_blocking(move || {
            source_state
                .inner
                .transcript
                .publish_context_sources(&source_thread, &sources)
        })
        .await
        .map_err(|e| e.to_string())??;
        let explicit = configuration::RUN_OVERRIDE
            .try_with(Clone::clone)
            .ok()
            .flatten();
        let resolved_configuration =
            self.runtime_configuration(Some(&project_root), explicit.as_deref())?;
        let yolo_sandbox = resolved_configuration.yolo_sandbox();
        let mut runtime_configuration = resolved_configuration.config;
        let context_cap = context_cap(&runtime_configuration, &settings);
        history.extend(self.inner.transcript.context_for_request(
            &thread_id,
            &text,
            context_cap / 2,
        )?);
        if settings.confirm_reads
            && !resolved_configuration
                .provenance
                .iter()
                .any(|(key, origin)| key.starts_with("approval") && origin != "default")
        {
            runtime_configuration.approval.rules.clear();
        }
        let source_root = project_root.clone();
        let source_paths = attachments.clone();
        let documents = tokio::task::spawn_blocking(move || {
            super::attachments::document_sources(&source_root, &source_paths)
        })
        .await
        .map_err(|e| e.to_string())??;
        let other_attachments = attachments
            .iter()
            .filter(|path| !documents.iter().any(|source| &source.source_path == *path))
            .cloned()
            .collect::<Vec<_>>();
        let (files, mut manifest) = super::attachments::attachment_context_with_budget(
            &project_root,
            &other_attachments,
            context_cap / 2,
        )?;
        history.extend(files);
        for source in &documents {
            manifest.push_str(&format!("\n- {} ({} bytes; complete document prepared before generation; original preserved)\n", source.source_path, source.text.len()));
        }
        history.extend(
            self.inner.transcript.document_context(
                &thread_id,
                &text,
                context_cap / 2,
                &documents
                    .iter()
                    .map(|source| source.source_path.clone())
                    .collect::<Vec<_>>(),
            )?,
        );
        let legacy: Vec<_> = self
            .inner
            .skills
            .read()
            .await
            .values()
            .map(Skill::core_skill)
            .collect();
        let store = self
            .plugin_store(Some(project_root.clone()))
            .with_skill_paths(runtime_configuration.skills.paths.clone())
            .map_err(|e| e.to_string())?;
        let (resolved_text, inline_skills, selected_plugins) = store
            .resolve_prompt(&text, &legacy)
            .map_err(|e| e.to_string())?;
        let instructions =
            themis_core::configuration::load_instructions(&runtime_configuration, &project_root)
                .map_err(|e| format!("repository instructions: {e:#}"))?;
        let resolved_text = if instructions.is_empty() {
            format!("{resolved_text}{manifest}")
        } else {
            format!(
                "{resolved_text}{manifest}\n\n{}\n{instructions}",
                themis_core::prompts::DESKTOP_REPOSITORY_GUIDANCE
            )
        };
        let display_text = format!("{text}{manifest}");
        let mut plugins: Vec<_> = store
            .list()
            .map_err(|e| e.to_string())?
            .into_iter()
            .filter(|plugin| plugin.enabled)
            .collect();
        // Explicit saved references retain their revision; all other capabilities use current revisions.
        for selected in selected_plugins {
            plugins.retain(|plugin| {
                plugin.scope != selected.scope || plugin.spec.name != selected.spec.name
            });
            plugins.push(selected);
        }
        let mut skill_catalog = legacy.clone();
        let mut catalog_references = String::new();
        for available in store
            .available_skills()
            .map_err(|e| e.to_string())?
            .into_iter()
            .filter(|skill| !skill.retired)
        {
            let parts: Vec<_> = available.id.split("--").collect();
            if parts.last().is_some_and(|id| {
                plugins.iter().any(|plugin| {
                    plugin.scope == available.scope
                        && plugin.spec.name == available.plugin
                        && plugin.spec.manual_skills.iter().any(|manual| manual == id)
                        && !text.contains(&format!(
                            "[[plugin:{}--{}]]",
                            if plugin.scope == "local" { "l" } else { "g" },
                            plugin.spec.name
                        ))
                })
            }) {
                continue;
            }
            if parts.len() == 3
                && !plugins.iter().any(|plugin| {
                    plugin.scope == available.scope
                        && plugin.spec.name == available.plugin
                        && plugin.spec.skills.iter().any(|skill| skill.id == parts[2])
                })
            {
                continue;
            }
            let id = if parts.len() == 3 {
                plugins
                    .iter()
                    .find(|plugin| {
                        plugin.spec.name == available.plugin && plugin.scope == available.scope
                    })
                    .map(|plugin| {
                        if plugin.source.as_deref() == Some("discovered") {
                            available.id.clone()
                        } else {
                            plugin.skill_id(parts[2])
                        }
                    })
                    .unwrap_or(available.id)
            } else {
                available.id
            };
            catalog_references.push_str(&format!("[[skill:{id}]]"));
        }
        let (_, skills, _) = store
            .resolve_prompt(&catalog_references, &legacy)
            .map_err(|e| e.to_string())?;
        for skill in skills {
            if !skill_catalog.iter().any(|known| known.id == skill.id) {
                skill_catalog.push(skill);
            }
        }
        {
            let threads = self.inner.threads.read().await;
            for id in threads
                .get(&thread_id)
                .ok_or("Unknown thread")?
                .skill_ids
                .iter()
                .filter(|_| !automation_run)
            {
                if !legacy.iter().any(|s| s.id == *id) {
                    return Err(format!("Unavailable legacy skill '{id}'"));
                }
            }
        }
        let snapshot = {
            let mut threads = self.inner.threads.write().await;
            let record = threads
                .get_mut(&thread_id)
                .ok_or_else(|| format!("unknown thread '{thread_id}'"))?;
            if record.running {
                return Err(format!(
                    "thread '{thread_id}' {BUSY}: a run is already in progress"
                ));
            }
            let limit = usize::try_from(settings.concurrency_limit).unwrap_or(usize::MAX);
            // The counter check runs under the state lock, so concurrent sends
            // serialize here and the gate cannot be overrun.
            if self.inner.running_count.load(Ordering::SeqCst) >= limit {
                return Err(format!(
                    "{CONCURRENCY_LIMIT} ({}): wait for a run to finish and retry",
                    settings.concurrency_limit
                ));
            }
            self.inner.running_count.fetch_add(1, Ordering::SeqCst);
            record.running = true;
            self.inner
                .stop_flags
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(thread_id.clone(), Arc::new(AtomicBool::new(false)));
            self.open_steer_inbox(&thread_id, &run_id, Arc::clone(&sink));
            if !record.titled {
                record.title = title_from_message(&text);
                record.titled = true;
            }
            // Use the validated snapshot; deletion during preparation cannot leak a run slot.
            let mut resolved: Vec<themis_core::skills::Skill> = record
                .skill_ids
                .iter()
                .filter(|_| !automation_run)
                .filter_map(|id| legacy.iter().find(|s| s.id == *id).cloned())
                .collect();
            for skill in inline_skills {
                if !resolved.iter().any(|s| s.id == skill.id) {
                    resolved.push(skill);
                }
            }
            RunSnapshot {
                reasoning_effort: reasoning_effort.or_else(|| record.reasoning_effort.clone()),
                history,
                documents,
                approval_timeout_seconds: settings.approval_timeout_seconds.clamp(30, 600),
                approval_mode: record.approval_mode,
                runtime_configuration: runtime_configuration.clone(),
                yolo_sandbox,
                work_root: record.project_root.clone(),
                is_git: record.is_git,
                model: record.model.clone(),
                skills: resolved,
                plugins,
                skill_catalog,
                short_reply: themis_core::runtime::is_short_reply(&text, !attachments.is_empty()),
                automation_run,
            }
        };
        // `/goal` sets the goal, a reply resumes a goal waiting for the user;
        // every run while a goal runs is told about it.
        let goal_before = self.goal_for(&thread_id);
        let resolved_text = match self.goal_for_message(sink.as_ref(), &thread_id, &text, &run_id) {
            Ok(context) => format!("{resolved_text}{context}"),
            Err(error) => {
                self.finish_run(&thread_id).await;
                return Err(error);
            }
        };
        // A new goal or an added request changes the checklist.
        let goal_after = self.goal_for(&thread_id);
        if goal_after
            .as_ref()
            .map(|goal| (goal.started_at, goal.added.len()))
            != goal_before
                .as_ref()
                .map(|goal| (goal.started_at, goal.added.len()))
        {
            self.refresh_checklist(Arc::clone(&sink), &thread_id);
        }
        let shown_attachments = attachments.clone();
        if let Err(error) = self.inner.transcript.append_user_with_attachments(
            &thread_id,
            &run_id,
            &display_text,
            attachments,
        ) {
            self.finish_run(&thread_id).await;
            return Err(error);
        }
        if let Some(state) = self
            .inner
            .server_sends
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_mut(&run_id)
        {
            *state = super::queue::ServerSend::Saved;
        }
        // Every open window shows the message now, saved and before any of
        // its run's events: one the server sends itself (queue, goal turn) or
        // another client's (the CLI, a second window). The window that sent it
        // already shows it and adopts this event instead of adding a copy.
        sink.emit_thread_event(&ThreadEventEnvelope {
            thread_id: thread_id.clone(),
            run_id: run_id.clone(),
            event: ThreadEvent::UserMessage {
                text: text.clone(),
                attachments: shown_attachments,
            },
        });
        self.persist_registry().await;
        let policy = RunPolicy {
            total_turns: runtime_configuration
                .total_turns
                .unwrap_or(settings.max_total_turns.clamp(1, 2000) as usize),
            context_token_budget: context_cap,
            recent_messages: runtime_configuration
                .recent_messages
                .unwrap_or(themis_core::configuration::DEFAULT_RECENT_MESSAGES),
        };
        let started = ThreadEventEnvelope {
            thread_id: thread_id.clone(),
            run_id: run_id.clone(),
            event: ThreadEvent::Started {
                task: resolved_text.clone(),
                max_turns: policy.total_turns,
            },
        };
        if let Err(error) = self.inner.transcript.append_event(&started) {
            self.finish_run(&thread_id).await;
            return Err(error);
        }
        sink.emit_thread_event(&started);
        if let Err(error) = self
            .spawn_run(&sink, &thread_id, &run_id, snapshot, resolved_text, policy)
            .await
        {
            let failed = ThreadEventEnvelope {
                thread_id: thread_id.clone(),
                run_id: run_id.clone(),
                event: ThreadEvent::Failed {
                    error: error.clone(),
                },
            };
            if let Err(save_error) = self.inner.transcript.append_event(&failed) {
                self.record_error("save_thread_event", save_error);
            }
            self.finish_run(&thread_id).await;
            sink.emit_thread_event(&failed);
            // A run that failed to start ended like any run: the queue moves on
            // and a goal continues (or retries). Callers see the marker and
            // leave that to this step.
            if let Some(state) = self
                .inner
                .server_sends
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get_mut(&run_id)
            {
                *state = super::queue::ServerSend::SpawnFailed;
            }
            self.spawn_after_run(sink, thread_id, run_id, failed.event);
            return Err(error);
        }
        Ok(RunHandle { run_id })
    }

    /// Renames a thread without changing its files or worktree.
    pub async fn rename_thread(
        &self,
        thread_id: String,
        title: String,
    ) -> Result<ThreadInfo, String> {
        let title = title.trim();
        if title.is_empty() || title.chars().count() > 120 || title.chars().any(char::is_control) {
            return Err("Use a thread title of 1–120 characters on one line".to_owned());
        }
        let info = {
            let mut threads = self.inner.threads.write().await;
            let record = threads
                .get_mut(&thread_id)
                .ok_or_else(|| "Thread not found".to_owned())?;
            record.title = title.to_owned();
            record.titled = true;
            thread_info(record, &self.inner.transcript)?
        };
        self.persist_registry().await;
        Ok(info)
    }

    /// Requests a stop at the next safe boundary; existing actions finish first.
    pub async fn stop_thread(&self, thread_id: String) -> Result<(), String> {
        let flags = self
            .inner
            .stop_flags
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let flag = flags
            .get(&thread_id)
            .ok_or_else(|| "This thread has no active run".to_owned())?;
        flag.store(true, Ordering::SeqCst);
        let mut pending = self.inner.pending.lock().unwrap_or_else(|e| e.into_inner());
        pending.retain(|_, approval| {
            if approval.thread_id == thread_id {
                let _ = approval.sender.send(themis_core::tools::Approval::Deny);
                false
            } else {
                true
            }
        });
        Ok(())
    }

    /// Removes an idle conversation, preserving shared and legacy workspace files.
    pub async fn discard_thread(&self, thread_id: String) -> Result<(), String> {
        let record = {
            let mut threads = self.inner.threads.write().await;
            match threads.get(&thread_id) {
                Some(record) if record.running => {
                    return Err(format!(
                        "thread '{thread_id}' is busy: wait for the run to finish \
                         before discarding"
                    ));
                }
                Some(_) => {
                    self.inner.transcript.delete_thread(&thread_id)?;
                    threads.remove(&thread_id).expect("checked above")
                }
                None => return Err(format!("unknown thread '{thread_id}'")),
            }
        };
        drop(record);
        self.forget_chat_grants(std::slice::from_ref(&thread_id));
        self.forget_queues(std::slice::from_ref(&thread_id));
        self.forget_goals(std::slice::from_ref(&thread_id));
        self.forget_questions(std::slice::from_ref(&thread_id));
        self.forget_browsers(std::slice::from_ref(&thread_id)).await;
        self.forget_background_tasks(&[thread_id]).await;
        self.persist_registry().await;
        Ok(())
    }

    /// Switches a thread's provider/model. During a run the switch applies from
    /// the next turn: the running turn keeps the model it started with.
    pub async fn set_provider(
        &self,
        thread_id: String,
        provider: ProviderKind,
        model: Option<String>,
    ) -> Result<ThreadInfo, String> {
        if provider != ProviderKind::Go {
            return Err("Only OpenCode Go is supported".to_owned());
        }
        let info = {
            let mut threads = self.inner.threads.write().await;
            let record = threads
                .get_mut(&thread_id)
                .ok_or_else(|| format!("unknown thread '{thread_id}'"))?;
            record.provider = provider;
            record.model = model.unwrap_or_default();
            record.reasoning_effort = None;
            thread_info(record, &self.inner.transcript)?
        };
        self.persist_registry().await;
        Ok(info)
    }

    /// Persists the thread's selected effort for manual and scheduled runs; a
    /// change during a run applies from the next turn.
    pub async fn set_thread_effort(
        &self,
        thread_id: String,
        effort: Option<String>,
    ) -> Result<ThreadInfo, String> {
        let effort = effort.filter(|value| !value.is_empty());
        if effort.is_some() {
            self.ensure_go_catalog().await?;
        }
        let info = {
            let mut threads = self.inner.threads.write().await;
            let record = threads
                .get_mut(&thread_id)
                .ok_or_else(|| format!("unknown thread '{thread_id}'"))?;
            if let Some(value) = effort.as_deref() {
                if !offers_effort(&self.inner.go_catalog.read().await, &record.model, value) {
                    return Err(format!(
                        "Effort '{value}' is unavailable for model '{}'",
                        record.model
                    ));
                }
            }
            record.reasoning_effort = effort;
            thread_info(record, &self.inner.transcript)?
        };
        self.persist_registry().await;
        Ok(info)
    }
}

/// The default model `wanted`, unless the loaded catalog shows it gone or unusable (Go
/// retired it): then Themis's default model, else the first usable one. With no catalog
/// loaded, `wanted` stands.
fn usable_model(catalog: &[themis_core::providers::GoModel], wanted: String) -> String {
    let usable = |id: &str| {
        catalog
            .iter()
            .any(|model| model.id == id && model.unavailable_reason.is_none())
    };
    if catalog.is_empty() || usable(&wanted) {
        return wanted;
    }
    if usable(GO_DEFAULT_MODEL) {
        return GO_DEFAULT_MODEL.to_owned();
    }
    catalog
        .iter()
        .find(|model| model.unavailable_reason.is_none())
        .map_or(wanted, |model| model.id.clone())
}

#[cfg(test)]
mod usable_model_tests {
    use super::*;
    use themis_core::providers::GoModel;

    fn listed(id: &str, unavailable: bool) -> GoModel {
        GoModel {
            id: id.into(),
            unavailable_reason: unavailable.then(|| "unsupported".into()),
            ..Default::default()
        }
    }

    #[test]
    fn a_retired_or_unusable_default_falls_back_to_a_usable_model() {
        let catalog = [
            listed("glm-5.3", false),
            listed(GO_DEFAULT_MODEL, false),
            listed("odd", true),
        ];
        assert_eq!(usable_model(&catalog, "glm-5.3".into()), "glm-5.3");
        assert_eq!(
            usable_model(&catalog, "retired-model".into()),
            GO_DEFAULT_MODEL
        );
        assert_eq!(usable_model(&catalog, "odd".into()), GO_DEFAULT_MODEL);
        let without_default = [listed("odd", true), listed("glm-5.3", false)];
        assert_eq!(usable_model(&without_default, "retired".into()), "glm-5.3");
        assert_eq!(usable_model(&[], "anything".into()), "anything");
    }
}
