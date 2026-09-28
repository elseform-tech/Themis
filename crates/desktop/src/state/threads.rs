use super::*;

impl AppState {
    /// Threads share the project checkout, including existing uncommitted files.
    pub async fn create_thread(
        &self,
        project_root: String,
        provider: ProviderKind,
        model: Option<String>,
    ) -> Result<ThreadInfo, String> {
        if provider != ProviderKind::Go {
            return Err("Only OpenCode Go is supported".to_owned());
        }
        let root = canonical_project_dir(&project_root)?;
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
            model: model
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| GO_DEFAULT_MODEL.to_owned()),
            reasoning_effort: None,
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
        let info = thread_info(&record);
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
            });
        self.persist_registry().await;
        Ok(info)
    }

    /// Returns the current info for `thread_id`.
    pub async fn get_thread(&self, thread_id: &str) -> Result<ThreadInfo, String> {
        let threads = self.inner.threads.read().await;
        threads
            .get(thread_id)
            .map(thread_info)
            .ok_or_else(|| format!("unknown thread '{thread_id}'"))
    }

    pub async fn get_thread_history(&self, thread_id: &str) -> Result<Vec<HistoryItem>, String> {
        self.get_thread(thread_id).await?;
        self.inner.transcript.history(thread_id)
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
            .map(thread_info)
            .collect();
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
        self.send_message_with_options(sink, thread_id, text, run_id, None)
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
        self.send_message_with_options(
            sink,
            thread_id,
            text,
            uuid::Uuid::new_v4().to_string(),
            effort,
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
    ) -> Result<RunHandle, String> {
        let settings = self.inner.settings.get().await;
        let history = self.inner.transcript.context(&thread_id, usize::MAX)?;
        let snapshot = {
            let mut threads = self.inner.threads.write().await;
            let record = threads
                .get_mut(&thread_id)
                .ok_or_else(|| format!("unknown thread '{thread_id}'"))?;
            if record.running {
                return Err(format!(
                    "thread '{thread_id}' is busy: a run is already in progress"
                ));
            }
            let limit = usize::try_from(settings.concurrency_limit).unwrap_or(usize::MAX);
            // The counter check runs under the state lock, so concurrent sends
            // serialize here and the gate cannot be overrun.
            if self.inner.running_count.load(Ordering::SeqCst) >= limit {
                return Err(format!(
                    "concurrency limit reached ({}): wait for a run to finish and retry",
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
            if !record.titled {
                record.title = title_from_message(&text);
                record.titled = true;
            }
            // Resolve the thread's skills to core skills for the run. This
            // nests the skills read lock under the threads write lock — the
            // only such nesting, and no path nests the reverse, so lock order
            // stays acyclic.
            let skills = self.inner.skills.read().await;
            let resolved: Vec<themis_core::skills::Skill> = record
                .skill_ids
                .iter()
                .filter_map(|id| skills.get(id))
                .map(Skill::core_skill)
                .collect();
            RunSnapshot {
                reasoning_effort: reasoning_effort.or_else(|| record.reasoning_effort.clone()),
                history,
                approval_timeout_seconds: settings.approval_timeout_seconds.clamp(30, 600),
                confirm_reads: settings.confirm_reads,
                work_root: record.project_root.clone(),
                is_git: record.is_git,
                provider: record.provider,
                model: record.model.clone(),
                skills: resolved,
            }
        };
        if let Err(error) = self
            .inner
            .transcript
            .append_user(&thread_id, &run_id, &text)
        {
            self.finish_run(&thread_id).await;
            return Err(error);
        }
        self.persist_registry().await;
        // Clamp: updates are validated, but the file may predate validation.
        let segment_turns = usize::try_from(settings.max_turns)
            .unwrap_or(crate::settings::MAX_TURNS_MAX as usize)
            .clamp(1, crate::settings::MAX_TURNS_MAX as usize);
        let policy = RunPolicy {
            segment_turns,
            total_turns: settings.max_total_turns.clamp(1, 2000) as usize,
            context_token_budget: settings.context_token_budget.clamp(2000, 200000) as usize,
            recent_messages: settings.context_messages.clamp(1, 100) as usize,
        };
        if let Err(error) = self
            .spawn_run(&sink, &thread_id, &run_id, snapshot, text, policy)
            .await
        {
            if let Err(save_error) = self.inner.transcript.append_event(&ThreadEventEnvelope {
                thread_id: thread_id.clone(),
                run_id: run_id.clone(),
                event: ThreadEvent::Failed {
                    error: error.clone(),
                },
            }) {
                self.record_error("save_thread_event", save_error);
            }
            self.finish_run(&thread_id).await;
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
            thread_info(record)
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
        self.persist_registry().await;
        Ok(())
    }

    /// Switches a thread's provider/model (rejected while a run is active).
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
            if record.running {
                return Err(format!(
                    "thread '{thread_id}' is busy: cannot switch provider mid-run"
                ));
            }
            record.provider = provider;
            record.model = model.unwrap_or_default();
            record.reasoning_effort = None;
            thread_info(record)
        };
        self.persist_registry().await;
        Ok(info)
    }

    /// Persists the thread's selected effort for manual and scheduled runs.
    pub async fn set_thread_effort(
        &self,
        thread_id: String,
        effort: Option<String>,
    ) -> Result<ThreadInfo, String> {
        let effort = effort.filter(|value| !value.is_empty());
        let info = {
            let mut threads = self.inner.threads.write().await;
            let record = threads
                .get_mut(&thread_id)
                .ok_or_else(|| format!("unknown thread '{thread_id}'"))?;
            if record.running {
                return Err(format!(
                    "thread '{thread_id}' is busy: cannot change effort mid-run"
                ));
            }
            if let Some(value) = effort.as_deref() {
                let catalog = self.inner.go_catalog.read().await;
                if !catalog.iter().any(|model| {
                    model.id == record.model
                        && model.effort_levels.iter().any(|level| level == value)
                }) {
                    return Err(format!(
                        "Effort '{value}' is unavailable for model '{}'",
                        record.model
                    ));
                }
            }
            record.reasoning_effort = effort;
            thread_info(record)
        };
        self.persist_registry().await;
        Ok(info)
    }
}
