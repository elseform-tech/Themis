//! Creating, changing, listing and removing automations, and a chat's task list.

use super::*;

impl AppState {
    pub async fn list_automations(&self) -> Vec<Automation> {
        let automations = self.inner.automations.read().await;
        let mut out: Vec<Automation> = automations.values().cloned().collect();
        out.sort_by(|left, right| left.id.cmp(&right.id));
        out
    }

    /// Creates an automation after validating the input. The project root
    /// must exist (stored canonicalized) and `next_run_at` starts at
    /// the next calendar occurrence or now + the legacy interval.
    pub async fn create_automation(&self, input: AutomationInput) -> Result<Automation, String> {
        self.create_automation_from(input, AutomationOrigin::User)
            .await
    }

    /// Like [`Self::create_automation`], recording who created it.
    pub async fn create_automation_from(
        &self,
        input: AutomationInput,
        origin: AutomationOrigin,
    ) -> Result<Automation, String> {
        // A user authoring their own automation (UI, CLI) is the approval; an
        // agent's check command must be approved separately by the caller.
        self.create_automation_checked(
            input,
            origin,
            if origin == AutomationOrigin::User {
                CheckGrant::Approved
            } else {
                CheckGrant::None
            },
        )
        .await
    }

    /// Creates an automation; `check_approved` says the caller obtained an
    /// approval of exactly `input.watch.check_command`.
    pub async fn create_automation_checked(
        &self,
        mut input: AutomationInput,
        origin: AutomationOrigin,
        grant: CheckGrant,
    ) -> Result<Automation, String> {
        normalize_watch(&mut input.watch);
        let now = Utc::now();
        if origin == AutomationOrigin::Agent {
            // Agent automations always end: interval watches in 2 hours with a
            // run limit, calendar automations in 7 days.
            let default_expiry = if input.schedule.is_none() {
                input.watch.max_runs.get_or_insert(WATCH_DEFAULT_MAX_RUNS);
                WATCH_DEFAULT_EXPIRY_MINS
            } else {
                CALENDAR_EXPIRY_MINS
            };
            input
                .watch
                .expires_at
                .get_or_insert_with(|| rfc3339_plus_minutes(now, default_expiry));
        }
        self.check_automation_input(&input, origin, origin, None, grant)
            .await?;
        let root = canonical_project_dir(&input.project_root)?;
        let interval_mins = if input.schedule.is_some() {
            60 // Calendar schedules do not use the legacy interval field.
        } else {
            u32::try_from(input.interval_mins).map_err(|_| {
                format!(
                    "interval_mins is out of range (got {})",
                    input.interval_mins
                )
            })?
        };
        let next_run_at = next_automation_run(input.schedule.as_ref(), interval_mins, now)?;
        let automation = Automation {
            id: uuid::Uuid::new_v4().to_string(),
            name: input.name,
            project_root: root.to_string_lossy().into_owned(),
            target_thread_id: input.target_thread_id.clone(),
            provider: input.provider,
            model: if input.target_thread_id.is_some() {
                String::new()
            } else {
                input.model
            },
            reasoning_effort: if input.target_thread_id.is_some() {
                None
            } else {
                input.reasoning_effort
            },
            skill_ids: if input.target_thread_id.is_some() {
                Vec::new()
            } else {
                dedup_ids(input.skill_ids)
            },
            interval_mins,
            schedule: input.schedule,
            task: input.task,
            enabled: input.enabled,
            last_run_at: None,
            next_run_at,
            run_count: 0,
            origin,
            watch_state: WatchState {
                approved_check: input
                    .watch
                    .check_command
                    .clone()
                    .filter(|_| grant == CheckGrant::Approved),
                ..WatchState::default()
            },
            watch: input.watch,
        };
        self.inner
            .automations
            .write()
            .await
            .insert(automation.id.clone(), automation.clone());
        self.persist_automations().await;
        Ok(automation)
    }

    /// Replaces the automation `automation_id` with `input`, keeping its id,
    /// history (`last_run_at`, `run_count`), and rescheduling `next_run_at`
    /// to the next calendar occurrence or new interval.
    pub async fn update_automation(
        &self,
        automation_id: String,
        input: AutomationInput,
    ) -> Result<Automation, String> {
        // The user typing a new command is its approval; renaming leaves the
        // agent's stored approval (or lack of one) as it was.
        let changed = self
            .inner
            .automations
            .read()
            .await
            .get(&automation_id)
            .is_some_and(|item| item.watch.check_command != input.watch.check_command);
        self.update_automation_checked(
            automation_id,
            input,
            if changed {
                CheckGrant::Approved
            } else {
                CheckGrant::None
            },
            AutomationOrigin::User,
        )
        .await
    }

    /// Updates an automation; a changed `check_command` needs `check_approved`
    /// (an approval of exactly that argv) unless policy allows it outright.
    pub async fn update_automation_checked(
        &self,
        automation_id: String,
        mut input: AutomationInput,
        grant: CheckGrant,
        actor: AutomationOrigin,
    ) -> Result<Automation, String> {
        normalize_watch(&mut input.watch);
        let existing = self
            .inner
            .automations
            .read()
            .await
            .get(&automation_id)
            .cloned()
            .ok_or_else(|| format!("unknown automation '{automation_id}'"))?;
        self.check_automation_input(&input, existing.origin, actor, Some(&existing), grant)
            .await?;
        let root = canonical_project_dir(&input.project_root)?;
        let interval_mins = if input.schedule.is_some() {
            60 // Calendar schedules do not use the legacy interval field.
        } else {
            u32::try_from(input.interval_mins).map_err(|_| {
                format!(
                    "interval_mins is out of range (got {})",
                    input.interval_mins
                )
            })?
        };
        let now = Utc::now();
        let next_run_at = next_automation_run(input.schedule.as_ref(), interval_mins, now)?;
        let mut automations = self.inner.automations.write().await;
        let automation = automations
            .get_mut(&automation_id)
            .ok_or_else(|| format!("unknown automation '{automation_id}'"))?;
        automation.name = input.name;
        automation.project_root = root.to_string_lossy().into_owned();
        automation.target_thread_id = input.target_thread_id.clone();
        automation.provider = input.provider;
        automation.model = if input.target_thread_id.is_some() {
            String::new()
        } else {
            input.model
        };
        automation.reasoning_effort = if input.target_thread_id.is_some() {
            None
        } else {
            input.reasoning_effort
        };
        automation.skill_ids = if input.target_thread_id.is_some() {
            Vec::new()
        } else {
            dedup_ids(input.skill_ids)
        };
        automation.interval_mins = interval_mins;
        automation.schedule = input.schedule;
        automation.task = input.task;
        automation.enabled = input.enabled;
        automation.next_run_at = next_run_at;
        if automation.watch.check_command != input.watch.check_command {
            // A different argv is a different command: new baseline, new approval.
            automation.watch_state.last_check_hash = None;
            automation.watch_state.approved_check = input
                .watch
                .check_command
                .clone()
                .filter(|_| grant == CheckGrant::Approved);
        } else if grant == CheckGrant::Approved && input.watch.check_command.is_some() {
            // Same argv, approved again (for example after a pause).
            automation.watch_state.approved_check = input.watch.check_command.clone();
        }
        automation.watch = input.watch;
        // An edit is a fresh start for the failure counter.
        automation.watch_state.consecutive_failures = 0;
        automation.watch_state.stopped_reason = None;
        let updated = automation.clone();
        drop(automations);
        self.persist_automations().await;
        Ok(updated)
    }

    /// Task feed for one chat: its live run state, the automations that
    /// continue it, and its recent finished automation runs.
    pub async fn get_thread_tasks(&self, thread_id: &str) -> Result<ThreadTasks, String> {
        let running = self
            .inner
            .threads
            .read()
            .await
            .get(thread_id)
            .ok_or_else(|| format!("unknown thread '{thread_id}'"))?
            .running;
        // The run's own approvals; a subagent's are shown on the subagent.
        let awaiting_approval = self
            .pending_approvals()
            .iter()
            .any(|request| request.thread_id == thread_id && request.subagent.is_none());
        let live: HashSet<String> = self
            .inner
            .automation_runs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .values()
            .filter(|context| context.thread_id == thread_id)
            .map(|context| context.automation_id.clone())
            .collect();
        let automations = self.inner.automations.read().await;
        let mut targeting: Vec<ThreadTaskAutomation> = automations
            .values()
            .filter(|automation| automation.target_thread_id.as_deref() == Some(thread_id))
            .map(|automation| ThreadTaskAutomation {
                id: automation.id.clone(),
                name: automation.name.clone(),
                enabled: automation.enabled,
                origin: automation.origin,
                schedule: automation.schedule.clone(),
                interval_mins: automation.interval_mins,
                next_run_at: automation.next_run_at.clone(),
                last_run_at: automation.last_run_at.clone(),
                run_count: automation.run_count,
                running: live.contains(&automation.id),
                watch: automation.watch.clone(),
                stopped_reason: automation.watch_state.stopped_reason.clone(),
            })
            .collect();
        targeting.sort_by(|left, right| left.next_run_at.cmp(&right.next_run_at));
        let mut recent: Vec<ThreadTaskRun> = self
            .list_review_items(None)
            .await
            .into_iter()
            .filter(|item| item.thread_id == thread_id)
            .map(|item| ThreadTaskRun {
                // A deleted automation keeps the name and origin its runs recorded.
                name: automations.get(&item.automation_id).map_or_else(
                    || {
                        item.source
                            .automation_name
                            .clone()
                            .unwrap_or_else(|| title_without_stamp(&item.title))
                    },
                    |automation| automation.name.clone(),
                ),
                origin: automations
                    .get(&item.automation_id)
                    .map_or(item.source.automation_origin, |automation| {
                        automation.origin
                    }),
                // `complete_automation_run` prefixes failed runs with "failed: ";
                // the review item has no separate status (covered by a unit test).
                ok: !item.summary.starts_with("failed:"),
                seen: item.status != ReviewStatus::Pending,
                review_id: item.id,
                automation_id: item.automation_id,
                finished_at: item.created_at,
                summary: item.summary,
            })
            .collect();
        recent.sort_by(|left, right| {
            right
                .finished_at
                .cmp(&left.finished_at)
                .then_with(|| right.review_id.cmp(&left.review_id))
        });
        recent.truncate(THREAD_TASKS_RECENT_LIMIT);
        let mut removed: Vec<AutomationRemoval> = self
            .inner
            .removals
            .read()
            .await
            .values()
            .filter(|item| item.thread_id.as_deref() == Some(thread_id))
            .cloned()
            .collect();
        removed.sort_by(|left, right| {
            right
                .removed_at
                .cmp(&left.removed_at)
                .then_with(|| right.id.cmp(&left.id))
        });
        removed.truncate(THREAD_TASKS_RECENT_LIMIT);
        Ok(ThreadTasks {
            thread_id: thread_id.to_owned(),
            run: ThreadRunTask {
                running,
                awaiting_approval,
            },
            automations: targeting,
            recent,
            background_tasks: self.thread_background_tasks(thread_id),
            removed,
        })
    }

    /// Deletes an automation. Threads it created stay as ordinary threads
    /// and its review items stay as history.
    pub async fn delete_automation(&self, automation_id: String) -> Result<(), String> {
        self.remove_automation(&automation_id, "deleted")
            .await
            .map(|_| ())
    }

    /// Removes an automation and records why, so Tasks can show the reason
    /// after it is gone. Returns the removed record.
    pub async fn remove_automation(
        &self,
        automation_id: &str,
        reason: &str,
    ) -> Result<Automation, String> {
        let removed = self
            .inner
            .automations
            .write()
            .await
            .remove(automation_id)
            .ok_or_else(|| format!("unknown automation '{automation_id}'"))?;
        let now = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
        let record = AutomationRemoval {
            id: uuid::Uuid::new_v4().to_string(),
            automation_id: removed.id.clone(),
            name: removed.name.clone(),
            origin: removed.origin,
            thread_id: removed.target_thread_id.clone(),
            removed_at: now,
            reason: reason.to_owned(),
            stop_when: removed.watch.stop_when.clone(),
            run_count: removed.run_count,
        };
        self.inner
            .removals
            .write()
            .await
            .insert(record.id.clone(), record);
        self.persist_automations().await;
        self.persist_removals().await;
        let _ = self.inner.diagnostics.append(
            "automation",
            "automation_removed",
            "info",
            json!({"automation_id": removed.id, "reason": reason, "runs": removed.run_count}),
        );
        Ok(removed)
    }

    /// Flips an automation's `enabled` flag. Enabling reschedules
    /// `next_run_at` to its next scheduled occurrence.
    pub async fn set_automation_enabled(
        &self,
        automation_id: String,
        enabled: bool,
    ) -> Result<Automation, String> {
        let now = Utc::now();
        let mut automations = self.inner.automations.write().await;
        let automation = automations
            .get_mut(&automation_id)
            .ok_or_else(|| format!("unknown automation '{automation_id}'"))?;
        if enabled {
            automation.next_run_at =
                next_automation_run(automation.schedule.as_ref(), automation.interval_mins, now)?;
            automation.watch_state.consecutive_failures = 0;
            automation.watch_state.stopped_reason = None;
        }
        automation.enabled = enabled;
        let updated = automation.clone();
        drop(automations);
        self.persist_automations().await;
        Ok(updated)
    }

    /// Validates automation input: non-empty name and task, an existing
    /// project dir, a usable Go model, existing skill ids, and
    /// `interval_mins >= 1`.
    pub(super) async fn check_automation_input(
        &self,
        input: &AutomationInput,
        origin: AutomationOrigin,
        actor: AutomationOrigin,
        existing: Option<&Automation>,
        grant: CheckGrant,
    ) -> Result<(), String> {
        if input.name.trim().is_empty() {
            return Err("automation name must not be empty".to_owned());
        }
        if input.task.trim().is_empty() {
            return Err("automation task must not be empty".to_owned());
        }
        if input.schedule.is_none() && input.interval_mins < 1 {
            return Err(format!(
                "interval_mins must be at least 1 (got {})",
                input.interval_mins
            ));
        }
        if input.schedule.is_none()
            && origin == AutomationOrigin::Agent
            && !(WATCH_MIN_INTERVAL_MINS..=WATCH_MAX_INTERVAL_MINS).contains(&input.interval_mins)
        {
            return Err(format!(
                "interval_mins must be between {WATCH_MIN_INTERVAL_MINS} and {WATCH_MAX_INTERVAL_MINS} minutes (got {})",
                input.interval_mins
            ));
        }
        let cap = (actor == AutomationOrigin::Agent).then_some(if input.schedule.is_none() {
            WATCH_MAX_EXPIRY_MINS
        } else {
            CALENDAR_EXPIRY_MINS
        });
        validate_watch(
            &input.watch,
            Utc::now(),
            existing.map(|item| &item.watch),
            cap,
        )?;
        if actor == AutomationOrigin::Agent
            && input.watch.expires_at.is_none()
            && existing.is_some_and(|item| item.watch.expires_at.is_some())
        {
            return Err("The expiry can only be removed by the user".to_owned());
        }
        if let Some(schedule) = &input.schedule {
            next_calendar_run(schedule, Utc::now())?;
        }
        let root = canonical_project_dir(&input.project_root)?;
        if let Some(command) = &input.watch.check_command {
            // An unchanged command keeps its stored approval; the scheduler
            // re-evaluates policy before every execution regardless.
            let unchanged =
                existing.is_some_and(|item| item.watch.check_command.as_ref() == Some(command));
            if !unchanged {
                match self
                    .check_command_policy(&root, input.target_thread_id.as_deref(), command)
                    .await?
                {
                    CheckPolicy::Deny => {
                        return Err("check_command is denied by the approval rules".to_owned())
                    }
                    CheckPolicy::Ask if !grant.permits() => {
                        return Err("check_command needs approval: it runs like the shell tool, and the approval policy asks first. Create the watch without check_command, or have the user approve this exact command.".to_owned())
                    }
                    _ => {}
                }
            }
        }
        let legacy: Vec<_> = self
            .inner
            .skills
            .read()
            .await
            .values()
            .map(Skill::core_skill)
            .collect();
        self.plugin_store(Some(root))
            .resolve_prompt(&input.task, &legacy)
            .map_err(|e| e.to_string())?;
        if input.provider != ProviderKind::Go {
            return Err("Only OpenCode Go is supported".to_owned());
        }
        if let Some(thread_id) = &input.target_thread_id {
            let threads = self.inner.threads.read().await;
            let thread = threads
                .get(thread_id)
                .ok_or_else(|| format!("unknown thread '{thread_id}'"))?;
            if thread.project_root != canonical_project_dir(&input.project_root)? {
                return Err("Selected thread is in a different project".to_owned());
            }
        } else {
            self.check_skill_ids(&input.skill_ids).await?;
        }
        Ok(())
    }

    /// Test hook: forcibly reschedules an automation (lets tick tests make
    /// one due without waiting out its interval).
    pub async fn set_automation_next_run_for_test(
        &self,
        automation_id: &str,
        next_run_at: String,
    ) -> Result<Automation, String> {
        let mut automations = self.inner.automations.write().await;
        let automation = automations
            .get_mut(automation_id)
            .ok_or_else(|| format!("unknown automation '{automation_id}'"))?;
        automation.next_run_at = next_run_at;
        let updated = automation.clone();
        drop(automations);
        self.persist_automations().await;
        Ok(updated)
    }
}
