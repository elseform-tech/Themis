//! Running a goal's turns: scheduling, starting, the after-run step, failures and resuming on start.

use super::*;

impl AppState {
    pub(super) fn mark_goal_turn(&self, run_id: &str) {
        self.inner
            .goal_turn_runs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(run_id.to_owned());
    }

    /// Forgets that `run_id` was a goal turn: a queued message ran after it,
    /// so the goal continues after that message.
    pub(in crate::state) fn goal_turn_interrupted(&self, run_id: &str) {
        self.inner
            .goal_turn_runs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(run_id);
    }

    /// Invalidates any timer waiting to continue this chat's goal.
    pub(super) fn bump_goal_generation(&self, thread_id: &str) -> u64 {
        let mut generations = self
            .inner
            .goal_generations
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let generation = generations.entry(thread_id.to_owned()).or_insert(0);
        *generation += 1;
        *generation
    }

    fn goal_generation(&self, thread_id: &str) -> u64 {
        self.inner
            .goal_generations
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(thread_id)
            .copied()
            .unwrap_or(0)
    }

    /// Starts the next goal turn after `delay`, unless the goal changed in between.
    fn schedule_goal_turn(
        &self,
        sink: Arc<dyn EventSink>,
        thread_id: String,
        delay: Duration,
        note: String,
    ) {
        let generation = self.bump_goal_generation(&thread_id);
        // The chat counts this down; a note would be out of date as soon as it was written.
        let due = now_ms().saturating_add(i64::try_from(delay.as_millis()).unwrap_or(i64::MAX));
        self.update_goal(sink.as_ref(), &thread_id, |goal| goal.retry_at = Some(due));
        let state = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            if state.goal_generation(&thread_id) != generation {
                return;
            }
            let Some(goal) = state.goal_for(&thread_id) else {
                return;
            };
            if goal.status == GoalStatus::Quota {
                state.update_goal(sink.as_ref(), &thread_id, |goal| {
                    set_status(goal, GoalStatus::Running, None);
                });
            } else if goal.status != GoalStatus::Running {
                return;
            }
            state.start_goal_turn(sink, thread_id, note).await;
        });
    }

    /// Starts goal turn N+1 now, if the goal runs and the chat is idle.
    pub(in crate::state) async fn start_goal_turn(
        &self,
        sink: Arc<dyn EventSink>,
        thread_id: String,
        note: String,
    ) {
        if self.require_thread(&thread_id).await.unwrap_or(true) {
            return; // That run's end continues the goal.
        }
        // A chat its Vision drives spends from that Vision's budget: past it, the goal pauses.
        if let Some(over) = self.visions_budget_holds(&thread_id) {
            self.update_goal(sink.as_ref(), &thread_id, |goal| {
                if goal.status == GoalStatus::Running {
                    set_status(goal, GoalStatus::Paused, Some(over.goal_reason().into()));
                }
            });
            return;
        }
        let turn_seq = self
            .inner
            .transcript
            .last_activity_seq(&thread_id)
            .unwrap_or(0);
        let Some(goal) = self.update_goal(sink.as_ref(), &thread_id, |goal| {
            goal.retry_at = None;
            if goal.status == GoalStatus::Running {
                goal.turns = goal.turns.saturating_add(1);
                goal.prev_turn_seq = goal.turn_seq;
                goal.turn_seq = turn_seq;
            }
        }) else {
            return;
        };
        if goal.status != GoalStatus::Running {
            return;
        }
        let note = self.with_open_questions(&thread_id, note);
        let text = core_goal::turn_message(goal.turns, &note);
        let run_id = uuid::Uuid::new_v4().to_string();
        self.mark_goal_turn(&run_id);
        let (sent, progress) = self
            .send_from_server(
                Arc::clone(&sink),
                &thread_id,
                text,
                &run_id,
                None,
                Vec::new(),
            )
            .await;
        if let Err(error) = sent {
            if progress == super::queue::ServerSend::SpawnFailed {
                return; // Its end step counts the failure and retries.
            }
            self.goal_turn_interrupted(&run_id);
            self.update_goal(sink.as_ref(), &thread_id, |goal| {
                goal.turns = goal.turns.saturating_sub(1);
            });
            if error.contains(super::threads::CONCURRENCY_LIMIT) {
                self.schedule_goal_turn(sink, thread_id, BUSY_RETRY, note);
            } else if !super::threads::send_must_wait(&error) {
                // A turn that could not start counts as a failed turn.
                let failures = self
                    .update_goal(sink.as_ref(), &thread_id, |goal| {
                        goal.failures = goal.failures.saturating_add(1);
                    })
                    .map_or(1, |goal| goal.failures);
                let _ = self.inner.diagnostics.append(
                    "goal",
                    "turn_not_started",
                    "warn",
                    json!({"thread_id": thread_id, "failures": failures}),
                );
                self.schedule_goal_turn(
                    sink,
                    thread_id,
                    backoff(failures),
                    "The previous turn could not start; continue toward the goal.".into(),
                );
            }
        }
    }

    /// The supervisor step after a run ended with nothing queued.
    pub(in crate::state) async fn goal_after_run(
        &self,
        sink: Arc<dyn EventSink>,
        thread_id: String,
        run_id: String,
        terminal: ThreadEvent,
    ) {
        self.inner
            .goal_turn_runs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&run_id);
        let Some(goal) = self.goal_for(&thread_id) else {
            return;
        };
        // The turn that completed the goal: its closing reply says what was built and how it was
        // checked, the record's results when the memory pass gives none.
        if goal.status == GoalStatus::Reached {
            if let ThreadEvent::Finished { result, .. } = &terminal {
                self.plan_results_from_reply(&thread_id, result).await;
            }
        }
        if goal.status != GoalStatus::Running {
            return;
        }
        match terminal {
            ThreadEvent::Failed { error } => {
                self.goal_turn_failed(sink, thread_id, run_id, &error);
                return;
            }
            ThreadEvent::Finished { .. } | ThreadEvent::Incomplete { .. } => {}
            _ => return,
        }
        self.update_goal(sink.as_ref(), &thread_id, |goal| goal.failures = 0);
        // A blocking question ended the run: the goal waits for its answer.
        if let Some(question) = self
            .open_questions(&thread_id)
            .into_iter()
            .find(|question| question.blocking && question.run_id == run_id)
        {
            self.update_goal(sink.as_ref(), &thread_id, |goal| {
                set_status(goal, GoalStatus::Waiting, Some(question.question.clone()));
            });
            return;
        }
        self.start_goal_turn(sink, thread_id, String::new()).await;
    }

    /// `note` followed by the chat's open questions, so they survive checkpoints.
    fn with_open_questions(&self, thread_id: &str, note: String) -> String {
        match self.open_questions_note(thread_id) {
            Some(open) if note.is_empty() => open,
            Some(open) => format!("{note}\n{open}"),
            None => note,
        }
    }

    /// A goal turn failed: Stop continues with the next turn, a used-up quota
    /// pauses and retries every 15 minutes, anything else retries after a
    /// growing wait, however often it fails.
    fn goal_turn_failed(
        &self,
        sink: Arc<dyn EventSink>,
        thread_id: String,
        run_id: String,
        error: &str,
    ) {
        let failure = core_goal::classify_failure(error);
        match failure {
            Failure::Stopped => {
                self.schedule_goal_turn(
                    sink,
                    thread_id,
                    Duration::ZERO,
                    themis_core::prompts::GOAL_NOTE_STOPPED.into(),
                );
            }
            Failure::Quota => {
                let reason = "The model quota is used up; trying again every 15 minutes.";
                // Schedule before anyone can see the pause: a model switch
                // that follows it must replace this retry, not be replaced.
                self.schedule_goal_turn(
                    Arc::clone(&sink),
                    thread_id.clone(),
                    QUOTA_RETRY,
                    "Resumed after the model quota pause.".into(),
                );
                self.update_goal(sink.as_ref(), &thread_id, |goal| {
                    set_status(goal, GoalStatus::Quota, Some(reason.into()));
                });
                self.goal_note(
                    sink.as_ref(),
                    &thread_id,
                    &run_id,
                    GoalNoteKind::Quota,
                    reason,
                );
            }
            failure @ (Failure::Transient | Failure::Other) => {
                let limit = if failure == Failure::Transient {
                    TRANSIENT_FAILURES_BEFORE_PAUSE
                } else {
                    OTHER_FAILURES_BEFORE_PAUSE
                };
                let failures = self
                    .update_goal(sink.as_ref(), &thread_id, |goal| {
                        goal.failures = goal.failures.saturating_add(1);
                    })
                    .map_or(1, |goal| goal.failures);
                // A blip gets a quiet retry; a model that keeps failing pauses the goal, and the
                // goal bar says why, until the person resumes it.
                if failures >= limit {
                    self.update_goal(sink.as_ref(), &thread_id, |goal| {
                        goal.failures = 0;
                        set_status(goal, GoalStatus::Paused, Some(KEPT_FAILING.to_owned()));
                    });
                    self.goal_note(
                        sink.as_ref(),
                        &thread_id,
                        &run_id,
                        GoalNoteKind::Status,
                        &format!(
                            "I'm pausing the goal because {KEPT_FAILING}. You can resume it whenever you're ready."
                        ),
                    );
                    let _ = self.inner.diagnostics.append(
                        "goal",
                        "paused_after_failures",
                        "warn",
                        json!({"thread_id": thread_id, "run_id": run_id, "failures": failures}),
                    );
                    return;
                }
                let wait = jittered(backoff(failures));
                self.schedule_goal_turn(
                    sink,
                    thread_id,
                    wait,
                    themis_core::prompts::fill(
                        themis_core::prompts::GOAL_NOTE_FAILED,
                        &[("error", &core_goal::one_line(error, 200))],
                    ),
                );
            }
        }
    }

    /// On start: goals that were running carry on (the cut-off turn is
    /// rechecked by the next one), goals out of quota retry, queued messages
    /// of idle chats go out.
    pub(in crate::state) async fn resume_after_boot(&self, sink: Arc<dyn EventSink>) {
        let thread_ids: Vec<String> = self.inner.threads.read().await.keys().cloned().collect();
        for thread_id in thread_ids {
            if self.dispatch_queue(Arc::clone(&sink), &thread_id).await {
                continue;
            }
            let Some(goal) = self.goal_for(&thread_id) else {
                continue;
            };
            match goal.status {
                GoalStatus::Running => {
                    // Time with the app closed does not count.
                    self.update_goal(sink.as_ref(), &thread_id, |goal| {
                        goal.running_since = Some(now_ms());
                    });
                    self.start_goal_turn(
                        Arc::clone(&sink),
                        thread_id,
                        themis_core::prompts::GOAL_NOTE_INTERRUPTED.into(),
                    )
                    .await;
                }
                GoalStatus::Quota => {
                    self.schedule_goal_turn(
                        Arc::clone(&sink),
                        thread_id,
                        Duration::ZERO,
                        "Resumed after the model quota pause.".into(),
                    );
                }
                _ => {}
            }
        }
    }

    /// A model switch ends a quota pause at once: the new model may have quota.
    pub async fn goal_model_changed(&self, sink: Arc<dyn EventSink>, thread_id: &str) {
        if self.goal_for(thread_id).map(|goal| goal.status) == Some(GoalStatus::Quota) {
            self.schedule_goal_turn(
                sink,
                thread_id.to_owned(),
                Duration::ZERO,
                "The model was switched after a quota pause; continue toward the goal.".into(),
            );
        }
    }
}

/// The wait before the next attempt after `failures` failed turns in a row:
/// 30 s, 1 min, 2 min … capped at 15 min.
pub(in crate::state) fn backoff(failures: u32) -> Duration {
    let seconds = 30u64.saturating_mul(1 << failures.saturating_sub(1).min(10));
    Duration::from_secs(seconds).min(MAX_BACKOFF)
}

/// `wait` plus up to a fifth more, so chats hit by the same outage don't all retry at once.
pub(super) fn jittered(wait: Duration) -> Duration {
    use std::hash::BuildHasher;
    let room = wait.as_millis() as u64 / 5 + 1;
    let extra = std::collections::hash_map::RandomState::new().hash_one(wait) % room;
    wait + Duration::from_millis(extra)
}
