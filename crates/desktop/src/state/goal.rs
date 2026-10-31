//! Goal mode: one editable sentence per chat, worked on turn after turn
//! until a separate check finds it reached.
//!
//! The goal lives in `goals/<chat id>.json` beside `threads.json`. Each goal
//! turn is an ordinary run. When a run ends and nothing is queued, the
//! supervisor here runs the goal check and either ends the goal, starts the
//! next turn, or pauses it. A goal never gives up: it pauses only when it is
//! reached, waiting for the user, or out of model quota (and retries that);
//! turns without progress, errors and restarts never stop it. The agent says
//! where the goal stands at the start of each turn, and once the check finds
//! it reached, one short turn has the agent tell the user.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use themis_core::goal::{self as core_goal, Failure, Verdict};

use super::chat_grants::safe_chat_id;
use super::AppState;
use crate::sink::EventSink;
use crate::types::{Goal, GoalNoteKind, GoalStatus, ThreadEvent, ThreadEventEnvelope};

/// Directory name under the app data directory.
pub(super) const GOALS_DIR: &str = "goals";
/// Turns without progress before the next turn is told to try a different approach.
const STALL_TURNS: u32 = 3;
/// Failed turns in a row before the goal pauses instead of retrying: a provider's bad
/// spell usually clears within minutes (about 15 of waiting here), other errors won't.
const TRANSIENT_FAILURES_BEFORE_PAUSE: u32 = 6;
const OTHER_FAILURES_BEFORE_PAUSE: u32 = 3;
/// Why a goal paused after failing, shown in its bar.
const KEPT_FAILING: &str = "the model kept failing";
/// How often a goal out of quota tries again.
const QUOTA_RETRY: Duration = Duration::from_secs(15 * 60);
/// Longest wait between attempts after failed turns.
const MAX_BACKOFF: Duration = Duration::from_secs(15 * 60);
/// How often a running goal's time is saved, so a crash or quit mid-turn
/// loses at most this much of the elapsed time.
pub(super) const BANK_INTERVAL: Duration = Duration::from_secs(60);
/// Wait before retrying a goal turn the concurrency limit turned away.
const BUSY_RETRY: Duration = Duration::from_secs(20);
/// Most requests added during a goal the check reads (the oldest are dropped).
const MAX_ADDED_REQUESTS: usize = 20;
/// Longest added request the check reads, in characters.
const MAX_ADDED_CHARS: usize = 2000;
/// Most command evidence the goal check reads, in characters.
const MAX_EVIDENCE_CHARS: usize = 32_000;
/// Most changed files a goal remembers, the latest ones.
const MAX_GOAL_FILES: usize = 30;
/// A long command output keeps its start and its end (where results and
/// summaries are), in characters.
const OUTPUT_HEAD_CHARS: usize = 2_000;
const OUTPUT_TAIL_CHARS: usize = 4_000;
/// History items read per page while gathering evidence (memory stays
/// bounded; the text kept is capped by `MAX_EVIDENCE_CHARS`).
const EVIDENCE_PAGE_ITEMS: usize = 2_000;

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// The wait before the next attempt after `failures` failed turns in a row:
/// 30 s, 1 min, 2 min … capped at 15 min.
pub(super) fn backoff(failures: u32) -> Duration {
    let seconds = 30u64.saturating_mul(1 << failures.saturating_sub(1).min(10));
    Duration::from_secs(seconds).min(MAX_BACKOFF)
}

/// The check's reason, ready to sit inside a sentence.
fn mid_sentence(reason: &str) -> String {
    let reason = reason.trim().trim_end_matches('.');
    let mut chars = reason.chars();
    let first = chars
        .next()
        .map(|c| c.to_lowercase().to_string())
        .unwrap_or_default();
    // An acronym or name keeps its capital.
    let keep = reason.chars().nth(1).is_some_and(char::is_uppercase);
    if keep {
        reason.to_owned()
    } else {
        format!("{first}{}", chars.as_str())
    }
}

/// `wait` plus up to a fifth more, so chats hit by the same outage don't all retry at once.
fn jittered(wait: Duration) -> Duration {
    use std::hash::BuildHasher;
    let room = wait.as_millis() as u64 / 5 + 1;
    let extra = std::collections::hash_map::RandomState::new().hash_one(wait) % room;
    wait + Duration::from_millis(extra)
}

/// The goal note for a small context window, such as "Context window is
/// small (32,768 tokens); long goals may stall."
pub(super) fn small_window_note(tokens: usize) -> String {
    let digits = tokens.to_string();
    let mut grouped = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    format!("Context window is small ({grouped} tokens); long goals may stall.")
}

/// Moves a goal to `status`, counting running time only while it runs.
fn set_status(goal: &mut Goal, status: GoalStatus, reason: Option<String>) {
    let now = now_ms();
    if let Some(since) = goal.running_since.take() {
        goal.active_ms = goal
            .active_ms
            .saturating_add(u64::try_from(now.saturating_sub(since)).unwrap_or(0));
    }
    if status == GoalStatus::Running {
        goal.running_since = Some(now);
    }
    goal.status = status;
    goal.reason = reason;
}

impl AppState {
    fn goal_path(&self, thread_id: &str) -> Option<PathBuf> {
        safe_chat_id(thread_id).then(|| self.inner.goals_dir.join(format!("{thread_id}.json")))
    }

    /// The chat's goal, if it has one. A damaged file counts as no goal and is
    /// noted in diagnostics with a fixed reason.
    pub fn goal_for(&self, thread_id: &str) -> Option<Goal> {
        let mut goals = self.inner.goals.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(goal) = goals.get(thread_id) {
            return goal.clone();
        }
        let goal = match self.goal_path(thread_id).map(std::fs::read_to_string) {
            Some(Ok(text)) => serde_json::from_str(&text).map_or_else(
                |_| {
                    let _ = self.inner.diagnostics.append(
                        "goal",
                        "goal_ignored",
                        "warn",
                        json!({"thread_id": thread_id, "reason": "unreadable goal file"}),
                    );
                    None
                },
                Some,
            ),
            _ => None,
        };
        goals.insert(thread_id.to_owned(), goal.clone());
        goal
    }

    /// Saves and announces the chat's goal (`None` removes it). Running time
    /// so far is banked on every save, so a crash loses at most part of a turn.
    fn store_goal(&self, sink: &dyn EventSink, thread_id: &str, mut goal: Option<Goal>) {
        if let Some(goal) = goal
            .as_mut()
            .filter(|goal| goal.status == GoalStatus::Running)
        {
            set_status(goal, GoalStatus::Running, goal.reason.clone());
        }
        let previous = self
            .inner
            .goals
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(thread_id.to_owned(), goal.clone())
            .flatten();
        if let Some(path) = self.goal_path(thread_id) {
            let result = match &goal {
                Some(goal) => super::write_json_file(&path, goal),
                None => match std::fs::remove_file(&path) {
                    Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                        Err(error.to_string())
                    }
                    _ => Ok(()),
                },
            };
            if let Err(error) = result {
                self.record_error("save_goal", error);
            }
        }
        let status = goal.as_ref().map(|goal| goal.status);
        if previous.map(|goal| goal.status) != status {
            let _ = self.inner.diagnostics.append(
                "goal",
                "status",
                "info",
                json!({"thread_id": thread_id, "status": status, "turns": goal.as_ref().map(|goal| goal.turns)}),
            );
        }
        sink.emit_thread_event(&ThreadEventEnvelope {
            thread_id: thread_id.to_owned(),
            run_id: String::new(),
            event: ThreadEvent::GoalChanged { goal },
        });
    }

    /// Changes the chat's goal with `change` and saves it; `None` if it has none.
    fn update_goal(
        &self,
        sink: &dyn EventSink,
        thread_id: &str,
        change: impl FnOnce(&mut Goal),
    ) -> Option<Goal> {
        let _guard = self
            .inner
            .goal_lock
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut goal = self.goal_for(thread_id)?;
        change(&mut goal);
        self.store_goal(sink, thread_id, Some(goal.clone()));
        Some(goal)
    }

    /// Posts a goal note into the chat, attached to the run it concerns.
    fn goal_note(
        &self,
        sink: &dyn EventSink,
        thread_id: &str,
        run_id: &str,
        outcome: GoalNoteKind,
        reason: &str,
    ) {
        let envelope = ThreadEventEnvelope {
            thread_id: thread_id.to_owned(),
            run_id: run_id.to_owned(),
            event: ThreadEvent::GoalNote {
                outcome,
                reason: core_goal::one_line(reason, 400),
            },
        };
        if let Err(error) = self.inner.transcript.append_event(&envelope) {
            self.record_error("save_thread_event", error);
        }
        sink.emit_thread_event(&envelope);
    }

    /// The chat's goal, for clients.
    pub async fn get_goal(&self, thread_id: String) -> Result<Option<Goal>, String> {
        self.require_thread(&thread_id).await?;
        Ok(self.goal_for(&thread_id))
    }

    /// Sets (or replaces) the chat's goal and marks it running. The caller's
    /// run, if any, is its first turn.
    pub(super) fn begin_goal(&self, sink: &dyn EventSink, thread_id: &str, text: &str) -> Goal {
        let _guard = self
            .inner
            .goal_lock
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let now = now_ms();
        let goal = Goal {
            text: text.to_owned(),
            status: GoalStatus::Running,
            reason: None,
            started_at: now,
            active_ms: 0,
            running_since: Some(now),
            turns: 0,
            stalls: 0,
            progress: None,
            failures: 0,
            // Evidence for the first check starts here.
            checked_seq: self
                .inner
                .transcript
                .last_activity_seq(thread_id)
                .unwrap_or(0),
            added: Vec::new(),
            proof: Vec::new(),
            files: Vec::new(),
            checking: false,
        };
        self.bump_goal_generation(thread_id);
        self.store_goal(sink, thread_id, Some(goal.clone()));
        goal
    }

    /// Sets the chat's goal from the agent's `set_goal` tool. The run that
    /// called it continues; the goal's turns follow when it ends.
    pub async fn set_goal(
        &self,
        sink: Arc<dyn EventSink>,
        thread_id: String,
        text: String,
    ) -> Result<Goal, String> {
        let running = self.require_thread(&thread_id).await?;
        let text = core_goal::normalize_goal(&text).ok_or("Describe the goal in one sentence")?;
        let previous = self
            .goal_for(&thread_id)
            .filter(|goal| goal.status != GoalStatus::Reached);
        let mut goal = self.begin_goal(sink.as_ref(), &thread_id, &text);
        // Rewording an open goal continues it: its time, turn count and the
        // requests added to it carry over.
        if let Some(previous) = previous {
            let stretch = previous.running_since.map_or(0, |since| {
                u64::try_from(now_ms().saturating_sub(since)).unwrap_or(0)
            });
            if let Some(updated) = self.update_goal(sink.as_ref(), &thread_id, |goal| {
                goal.active_ms = previous.active_ms.saturating_add(stretch);
                goal.turns = previous.turns;
                goal.added = previous.added;
                goal.proof = previous.proof;
                goal.files = previous.files;
            }) {
                goal = updated;
            }
        }
        self.refresh_checklist(Arc::clone(&sink), &thread_id);
        if !running {
            self.start_goal_turn(
                sink,
                thread_id,
                "The goal was just set; start on it.".into(),
            )
            .await;
        }
        Ok(goal)
    }

    /// Replaces the goal's sentence; the next turn reads the new one.
    pub async fn edit_goal(
        &self,
        sink: Arc<dyn EventSink>,
        thread_id: String,
        text: String,
    ) -> Result<Goal, String> {
        self.require_thread(&thread_id).await?;
        let text = core_goal::normalize_goal(&text).ok_or("The goal cannot be empty")?;
        let goal = self
            .update_goal(sink.as_ref(), &thread_id, |goal| {
                goal.text = text;
                // A changed goal gets a fresh look: old stalls no longer apply.
                goal.stalls = 0;
            })
            .ok_or_else(|| "This chat has no goal".to_owned())?;
        self.refresh_checklist(sink, &thread_id);
        Ok(goal)
    }

    /// Pauses the goal after the current turn; nothing is undone.
    pub async fn pause_goal(
        &self,
        sink: Arc<dyn EventSink>,
        thread_id: String,
    ) -> Result<Goal, String> {
        self.require_thread(&thread_id).await?;
        self.bump_goal_generation(&thread_id);
        let goal = self
            .update_goal(sink.as_ref(), &thread_id, |goal| {
                if goal.status != GoalStatus::Reached {
                    set_status(goal, GoalStatus::Paused, None);
                }
            })
            .ok_or_else(|| "This chat has no goal".to_owned())?;
        self.goal_note(
            sink.as_ref(),
            &thread_id,
            "",
            GoalNoteKind::Status,
            "I've paused the goal as you asked, and nothing more runs until you resume it.",
        );
        Ok(goal)
    }

    /// Whether the person wrote in this chat since the goal's last check: the
    /// agent may pause a goal only because they asked.
    pub(super) fn person_spoke_since_goal_check(&self, thread_id: &str) -> bool {
        let Some(goal) = self.goal_for(thread_id) else {
            return false;
        };
        let mut through = goal.checked_seq;
        while let Ok(page) =
            self.inner
                .transcript
                .history_after(thread_id, through, EVIDENCE_PAGE_ITEMS)
        {
            let full = page.len() == EVIDENCE_PAGE_ITEMS;
            for (seq, item) in page {
                through = seq;
                let spoke = match item {
                    crate::transcript::HistoryItem::User { text, .. } => {
                        !text.starts_with(themis_core::goal::GOAL_TURN_MARKER)
                            && !text.starts_with("/goal")
                    }
                    crate::transcript::HistoryItem::Event { envelope } => {
                        matches!(envelope.event, ThreadEvent::Steered { .. })
                    }
                    crate::transcript::HistoryItem::Legacy { .. } => false,
                };
                if spoke {
                    return true;
                }
            }
            if !full {
                break;
            }
        }
        false
    }

    /// Saves the running time of every running goal (see [`BANK_INTERVAL`]).
    /// Saving banks the time since `running_since` into `active_ms`.
    pub fn bank_running_goals(&self, sink: &dyn EventSink) {
        let running: Vec<String> = self
            .inner
            .goals
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter(|(_, goal)| {
                goal.as_ref()
                    .is_some_and(|goal| goal.status == GoalStatus::Running)
            })
            .map(|(id, _)| id.clone())
            .collect();
        for thread_id in running {
            self.update_goal(sink, &thread_id, |_| {});
        }
    }

    /// Removes the chat's goal. A turn already running finishes as an
    /// ordinary run; no further turn or check starts.
    pub async fn remove_goal(
        &self,
        sink: Arc<dyn EventSink>,
        thread_id: String,
    ) -> Result<(), String> {
        self.require_thread(&thread_id).await?;
        if self.goal_for(&thread_id).is_none() {
            return Err("This chat has no goal".to_owned());
        }
        self.bump_goal_generation(&thread_id);
        self.store_goal(sink.as_ref(), &thread_id, None);
        self.goal_note(
            sink.as_ref(),
            &thread_id,
            "",
            GoalNoteKind::Status,
            "I've removed the goal, so I won't start any more turns for it.",
        );
        Ok(())
    }

    /// Resumes a paused, stuck, waiting or out-of-quota goal; starts a turn
    /// now when no run is going.
    pub async fn resume_goal(
        &self,
        sink: Arc<dyn EventSink>,
        thread_id: String,
    ) -> Result<Goal, String> {
        let running = self.require_thread(&thread_id).await?;
        let previous = self
            .goal_for(&thread_id)
            .ok_or_else(|| "This chat has no goal".to_owned())?;
        if previous.status == GoalStatus::Reached {
            return Err("This goal is already reached; send /goal to start a new one".to_owned());
        }
        let goal = self
            .update_goal(sink.as_ref(), &thread_id, |goal| {
                if goal.status != GoalStatus::Running {
                    set_status(goal, GoalStatus::Running, None);
                }
                goal.stalls = 0;
                goal.failures = 0;
            })
            .ok_or_else(|| "This chat has no goal".to_owned())?;
        self.goal_note(
            sink.as_ref(),
            &thread_id,
            "",
            GoalNoteKind::Status,
            "I'm picking the goal back up.",
        );
        if !running && previous.status != GoalStatus::Running {
            let note = match previous.status {
                GoalStatus::Quota => "Resumed after the model quota pause.",
                GoalStatus::Stuck => themis_core::prompts::GOAL_NOTE_RESUMED_STUCK,
                _ => "The user resumed the goal.",
            };
            self.start_goal_turn(sink, thread_id, note.into()).await;
        }
        Ok(goal)
    }

    /// Called by the send path before a user message starts a run: `/goal`
    /// sets the goal, a reply resumes a goal waiting for the user, and a new
    /// message after a reached goal clears it. Returns the text the run sees
    /// about the goal (empty when there is none running), and whether this
    /// run is a goal turn.
    pub(super) fn goal_for_message(
        &self,
        sink: &dyn EventSink,
        thread_id: &str,
        text: &str,
        run_id: &str,
    ) -> Result<String, String> {
        let is_turn = core_goal::turn_number(text).is_some();
        if let Some(sentence) = core_goal::parse_goal_command(text) {
            if sentence.is_empty() {
                return Err("Add the goal after /goal, for example: /goal speed up p95".to_owned());
            }
            self.begin_goal(sink, thread_id, &sentence);
            self.update_goal(sink, thread_id, |goal| goal.turns = 1);
            self.mark_goal_turn(run_id);
        } else if !is_turn {
            self.note_added_request(sink, thread_id, text);
            match self.goal_for(thread_id).map(|goal| goal.status) {
                // An answer to another question leaves the goal waiting on
                // the blocking one; any other reply resumes it.
                Some(GoalStatus::Waiting)
                    if !(text.starts_with(super::questions::ANSWER_MARKER)
                        && self
                            .open_questions(thread_id)
                            .iter()
                            .any(|question| question.blocking)) =>
                {
                    self.update_goal(sink, thread_id, |goal| {
                        set_status(goal, GoalStatus::Running, None);
                    });
                }
                Some(GoalStatus::Reached) => {
                    let _guard = self
                        .inner
                        .goal_lock
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    self.store_goal(sink, thread_id, None);
                }
                _ => {}
            }
        }
        Ok(match self.goal_for(thread_id) {
            Some(goal) if goal.status == GoalStatus::Running => core_goal::goal_context(&goal.text),
            _ => String::new(),
        })
    }

    /// Remembers a request the user sent while the chat's goal is open (not
    /// reached), so the goal check counts it. Answers to the agent's questions
    /// and runtime goal turns are not requests; a steer that comes back as the
    /// next message is counted once.
    pub(super) fn note_added_request(&self, sink: &dyn EventSink, thread_id: &str, text: &str) {
        let text = text.trim();
        if text.is_empty()
            || text.starts_with(super::questions::ANSWER_MARKER)
            || core_goal::turn_number(text).is_some()
        {
            return;
        }
        let open = self.goal_for(thread_id).is_some_and(|goal| {
            goal.status != GoalStatus::Reached && !goal.added.iter().any(|seen| seen == text)
        });
        if !open {
            return;
        }
        let request: String = text.chars().take(MAX_ADDED_CHARS).collect();
        self.update_goal(sink, thread_id, |goal| {
            let item = core_goal::pending_item(&request);
            if !goal.proof.iter().any(|known| known.what == item.what) {
                goal.proof.push(item);
            }
            goal.added.push(request);
            if goal.added.len() > MAX_ADDED_REQUESTS {
                goal.added.remove(0);
            }
        });
    }

    /// Lists what the goal asks for, in the background, so the checklist follows the goal as it
    /// is set, edited and added to. Dropped if the goal moved on meanwhile; a failure leaves the
    /// list as it was, and the next check fills it anyway.
    pub(super) fn refresh_checklist(&self, sink: Arc<dyn EventSink>, thread_id: &str) {
        let Some(goal) = self.goal_for(thread_id) else {
            return;
        };
        if goal.status == GoalStatus::Reached {
            return;
        }
        let state = self.clone();
        let thread_id = thread_id.to_owned();
        tokio::spawn(async move {
            let planned = match state.goal_check_config(&thread_id).await {
                Ok((config, catalog)) => themis_core::providers::with_catalog_limits(
                    &catalog,
                    themis_core::context_metrics::with_usage_recorder(
                        state.usage_recorder(&thread_id, ""),
                        themis_core::context_metrics::with_usage_kind(
                            themis_core::context_metrics::UsageKind::GoalCheck,
                            core_goal::plan_checklist_with_config(
                                &config,
                                &goal.text,
                                &goal.added,
                                &goal.proof,
                            ),
                        ),
                    ),
                )
                .await
                .map_err(|error| format!("{error:#}")),
                Err(error) => Err(error),
            };
            let Ok(planned) = planned else {
                let _ = state.inner.diagnostics.append(
                    "goal",
                    "checklist_failed",
                    "warn",
                    json!({"thread_id": thread_id}),
                );
                return;
            };
            state.update_goal(sink.as_ref(), &thread_id, |current| {
                if current.started_at == goal.started_at
                    && current.text == goal.text
                    && current.added == goal.added
                    && current.proof == goal.proof
                {
                    current.proof = planned;
                }
            });
        });
    }

    /// Notes once, when a goal's first turn starts, that the context window
    /// is below [`themis_core::context_window::SMALL_WINDOW_TOKENS`]: long
    /// goals may stall there.
    pub(super) fn warn_small_goal_window(
        &self,
        sink: &dyn EventSink,
        thread_id: &str,
        run_id: &str,
        window_tokens: usize,
    ) {
        if window_tokens >= themis_core::context_window::SMALL_WINDOW_TOKENS {
            return;
        }
        let first_turn = self
            .inner
            .goal_turn_runs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(run_id)
            && self
                .goal_for(thread_id)
                .is_some_and(|goal| goal.status == GoalStatus::Running && goal.turns <= 1);
        if !first_turn {
            return;
        }
        let _ = self.inner.diagnostics.append(
            "goal",
            "small_context_window",
            "info",
            json!({"thread_id": thread_id, "run_id": run_id, "window_tokens": window_tokens}),
        );
        self.goal_note(
            sink,
            thread_id,
            run_id,
            GoalNoteKind::SmallWindow,
            &small_window_note(window_tokens),
        );
    }

    fn mark_goal_turn(&self, run_id: &str) {
        self.inner
            .goal_turn_runs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(run_id.to_owned());
    }

    /// Forgets that `run_id` was a goal turn: a queued message ran after it,
    /// so the goal continues after that message without checking this turn.
    pub(super) fn goal_turn_interrupted(&self, run_id: &str) {
        self.inner
            .goal_turn_runs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(run_id);
    }

    /// Invalidates any timer waiting to continue this chat's goal.
    fn bump_goal_generation(&self, thread_id: &str) -> u64 {
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
    pub(super) async fn start_goal_turn(
        &self,
        sink: Arc<dyn EventSink>,
        thread_id: String,
        note: String,
    ) {
        if self.require_thread(&thread_id).await.unwrap_or(true) {
            return; // That run's end continues the goal.
        }
        let Some(goal) = self.update_goal(sink.as_ref(), &thread_id, |goal| {
            if goal.status == GoalStatus::Running {
                goal.turns = goal.turns.saturating_add(1);
            }
        }) else {
            return;
        };
        if goal.status != GoalStatus::Running {
            return;
        }
        let note = self.with_open_questions(&thread_id, note);
        let text = core_goal::turn_message(goal.turns, &goal.text, &note, &goal.proof, &goal.files);
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
    pub(super) async fn goal_after_run(
        &self,
        sink: Arc<dyn EventSink>,
        thread_id: String,
        run_id: String,
        terminal: ThreadEvent,
    ) {
        // Every run is checked, not only goal turns: a message's run that
        // finished the goal would otherwise be followed by a turn redoing it.
        self.inner
            .goal_turn_runs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&run_id);
        let Some(goal) = self.goal_for(&thread_id) else {
            return;
        };
        if goal.status != GoalStatus::Running {
            return;
        }
        let answer = match terminal {
            ThreadEvent::Failed { error } => {
                self.goal_turn_failed(sink, thread_id, run_id, &error);
                return;
            }
            ThreadEvent::Finished { result, .. } | ThreadEvent::Incomplete { result, .. } => result,
            _ => return,
        };
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
        // A passing status line says so while the check reads the turn.
        self.update_goal(sink.as_ref(), &thread_id, |goal| goal.checking = true);
        let (evidence, turn_files, through_seq) = self.turn_evidence(&thread_id, goal.checked_seq);
        let changed_files = !turn_files.is_empty();
        self.update_goal(sink.as_ref(), &thread_id, |goal| {
            for file in &turn_files {
                goal.files.retain(|known| known != file);
                goal.files.push(file.clone());
            }
            let excess = goal.files.len().saturating_sub(MAX_GOAL_FILES);
            goal.files.drain(..excess);
        });
        let checked_goal = goal.started_at;
        let verdict = match self.goal_check_config(&thread_id).await {
            Ok((config, catalog)) => {
                // The same per-model routes and limits real runs use.
                themis_core::providers::with_catalog_limits(
                    &catalog,
                    themis_core::context_metrics::with_usage_recorder(
                        self.usage_recorder(&thread_id, &run_id),
                        themis_core::context_metrics::with_usage_kind(
                            themis_core::context_metrics::UsageKind::GoalCheck,
                            core_goal::check_with_config(
                                &config,
                                &goal.text,
                                &goal.added,
                                &goal.proof,
                                &answer,
                                &evidence,
                            ),
                        ),
                    ),
                )
                .await
                .map_err(|error| format!("{error:#}"))
            }
            Err(error) => Err(error),
        };
        let check_failed = verdict.is_err();
        self.update_goal(sink.as_ref(), &thread_id, |goal| goal.checking = false);
        // The goal may have been paused or edited while the check ran.
        let Some(goal) = self.goal_for(&thread_id) else {
            return;
        };
        if goal.status != GoalStatus::Running {
            return;
        }
        let verdict = match verdict {
            Ok(verdict) => verdict,
            Err(error) => {
                let _ = self.inner.diagnostics.append(
                    "goal",
                    "check_failed",
                    "warn",
                    json!({"thread_id": thread_id, "run_id": run_id}),
                );
                let reason = if matches!(core_goal::classify_failure(&error), Failure::Quota) {
                    // The check itself ran out of quota: so will the next turn.
                    self.goal_turn_failed(sink, thread_id, run_id, &error);
                    return;
                } else {
                    "The goal check could not decide this time; continuing."
                };
                core_goal::GoalVerdict {
                    verdict: Verdict::NotYet,
                    reason: reason.into(),
                    next: None,
                    requirements: Vec::new(),
                    progress: None,
                    progressed: None,
                    told_user: None,
                }
            }
        };
        // "Waiting for you" needs a question to the user in the answer; a check
        // that only lacks evidence means not yet.
        let verdict = if verdict.verdict == Verdict::NeedsUser && !answer.contains('?') {
            core_goal::GoalVerdict {
                verdict: Verdict::NotYet,
                ..verdict
            }
        } else {
            verdict
        };
        let _ = self.inner.diagnostics.append(
            "goal",
            "checked",
            "info",
            json!({"thread_id": thread_id, "run_id": run_id, "verdict": verdict.verdict}),
        );
        // A decided check has judged this evidence; a failed one has not, so
        // the next check reads it again.
        // Only for the goal that was checked: one set while the check ran keeps
        // its own starting point.
        if !check_failed {
            self.update_goal(sink.as_ref(), &thread_id, |goal| {
                if goal.started_at == checked_goal {
                    goal.checked_seq = through_seq;
                    if !verdict.requirements.is_empty() {
                        goal.proof = verdict.requirements.clone();
                    }
                }
            });
        }
        match verdict.verdict {
            Verdict::Reached => {
                let Some(goal) = self.update_goal(sink.as_ref(), &thread_id, |goal| {
                    goal.progress = verdict.progress.clone().or(goal.progress.take());
                    set_status(goal, GoalStatus::Reached, Some(verdict.reason.clone()));
                }) else {
                    return;
                };
                // An answer that already says so needs no second message,
                // unless open questions still have to be named.
                if verdict.told_user == Some(true) && self.open_questions(&thread_id).is_empty() {
                    // The chat last said it was checking, so it says how that came out.
                    self.goal_note(
                        sink.as_ref(),
                        &thread_id,
                        &run_id,
                        GoalNoteKind::Reached,
                        &format!(
                            "That's proven, because {}, so I'm closing the goal.",
                            mid_sentence(&verdict.reason)
                        ),
                    );
                    return;
                }
                self.announce_reached(sink, &thread_id, &goal, &verdict.reason)
                    .await;
            }
            Verdict::NeedsUser => {
                // The answer already asks the user; the goal waits for the reply.
                self.update_goal(sink.as_ref(), &thread_id, |goal| {
                    set_status(goal, GoalStatus::Waiting, Some(verdict.reason.clone()));
                });
            }
            Verdict::NotYet | Verdict::Unreachable => {
                // The check judges progress; a reworded measurement is not.
                // Changed files always are: a checker looking only for proof
                // of completion must not count real work as a stall.
                let progressed = changed_files
                    || verdict.progressed.unwrap_or_else(|| {
                        verdict
                            .progress
                            .as_ref()
                            .is_some_and(|progress| Some(progress) != goal.progress.as_ref())
                    });
                // The checker failing says nothing about the agent's progress.
                let stalls = if progressed {
                    0
                } else if check_failed {
                    goal.stalls
                } else {
                    goal.stalls.saturating_add(1)
                };
                self.update_goal(sink.as_ref(), &thread_id, |goal| {
                    goal.stalls = stalls;
                    if verdict.progress.is_some() {
                        goal.progress = verdict.progress.clone();
                    }
                });
                let shown_reason = verdict.reason.clone();
                // The person reads the reason; only the agent needs the how.
                let agent_note = match &verdict.next {
                    Some(next) => format!("{} {next}", verdict.reason),
                    None => verdict.reason.clone(),
                };
                // Turns without progress never stop the goal; the next one is
                // told to change course.
                let note = if stalls >= STALL_TURNS {
                    let _ = self.inner.diagnostics.append(
                        "goal",
                        "stalled",
                        "info",
                        json!({"thread_id": thread_id, "run_id": run_id, "stalls": stalls}),
                    );
                    format!(
                        "{agent_note} {}",
                        themis_core::prompts::fill(
                            themis_core::prompts::GOAL_NOTE_STALLED,
                            &[("turns", &stalls.to_string())],
                        )
                    )
                } else {
                    agent_note
                };
                // The chat dims the turn on this; the agent says why in its own words next turn.
                if check_failed {
                    self.goal_note(
                        sink.as_ref(),
                        &thread_id,
                        &run_id,
                        GoalNoteKind::Status,
                        "The check couldn't decide this time, so I'm carrying on.",
                    );
                } else {
                    self.goal_note(
                        sink.as_ref(),
                        &thread_id,
                        &run_id,
                        GoalNoteKind::NotYet,
                        &shown_reason,
                    );
                }
                self.start_goal_turn(sink, thread_id, note).await;
            }
        }
    }

    /// `note` followed by the chat's open questions, so they survive checkpoints.
    fn with_open_questions(&self, thread_id: &str, note: String) -> String {
        match self.open_questions_note(thread_id) {
            Some(open) => format!("{note}\n{open}"),
            None => note,
        }
    }

    /// One short turn in which the agent tells the user the goal is reached,
    /// in its own words. The goal is already reached, so no check follows it.
    async fn announce_reached(
        &self,
        sink: Arc<dyn EventSink>,
        thread_id: &str,
        goal: &Goal,
        reason: &str,
    ) {
        let open = self.open_questions_note(thread_id);
        let reason = match &open {
            Some(open) => format!("{reason}\n{open}"),
            None => reason.to_owned(),
        };
        let text = core_goal::reached_message(
            goal.turns.saturating_add(1),
            &goal.text,
            &reason,
            open.is_some(),
        );
        let run_id = uuid::Uuid::new_v4().to_string();
        let (sent, _) = self
            .send_from_server(sink, thread_id, text, &run_id, None, Vec::new())
            .await;
        if sent.is_err() {
            // The goal stays reached; only the agent's sentence is missing.
            let _ = self.inner.diagnostics.append(
                "goal",
                "reached_turn_not_started",
                "warn",
                json!({"thread_id": thread_id}),
            );
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
                self.goal_note(
                    sink.as_ref(),
                    &thread_id,
                    &run_id,
                    GoalNoteKind::Status,
                    &format!(
                        "That turn failed, so I'll try again in {}.",
                        if wait.as_secs() < 60 {
                            format!("{} s", wait.as_secs())
                        } else {
                            format!("{} min", wait.as_secs() / 60)
                        }
                    ),
                );
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

    /// The commands every run since the previous decided goal check ran
    /// (the turn, plus any user message, failed or interrupted turn before
    /// it), with bounded output; whether those runs changed files; and the
    /// history position read through. History after `after_seq` is read in
    /// pages; only the newest evidence text is kept, but every item counts
    /// toward "changed files" and the position.
    fn turn_evidence(&self, thread_id: &str, after_seq: i64) -> (String, Vec<String>, i64) {
        let mut evidence = String::new();
        let mut changed = Vec::new();
        let mut through = after_seq;
        while let Ok(page) =
            self.inner
                .transcript
                .history_after(thread_id, through, EVIDENCE_PAGE_ITEMS)
        {
            let full = page.len() == EVIDENCE_PAGE_ITEMS;
            for (seq, item) in page {
                through = seq;
                let crate::transcript::HistoryItem::Event { envelope } = item else {
                    continue;
                };
                let line = match envelope.event {
                    ThreadEvent::ToolStarted { tool, summary, .. } => {
                        format!("$ {tool}: {summary}\n")
                    }
                    ThreadEvent::ToolFinished { ok, output, .. } => {
                        let output = clip_output(&output);
                        format!("{}{output}\n\n", if ok { "" } else { "(failed) " })
                    }
                    ThreadEvent::FilesChanged { files } => {
                        changed.extend(files.iter().map(|file| file.path.clone()));
                        format!(
                            "Files changed: {}\n",
                            files
                                .iter()
                                .map(|file| file.path.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        )
                    }
                    _ => continue,
                };
                evidence.push_str(&line);
                // Keep the end: the last commands carry the latest measurements.
                let count = evidence.chars().count();
                if count > 2 * MAX_EVIDENCE_CHARS {
                    evidence = evidence.chars().skip(count - MAX_EVIDENCE_CHARS).collect();
                }
            }
            if !full {
                break;
            }
        }
        let count = evidence.chars().count();
        if count > MAX_EVIDENCE_CHARS {
            evidence = format!(
                "…\n{}",
                evidence
                    .chars()
                    .skip(count - MAX_EVIDENCE_CHARS)
                    .collect::<String>()
            );
        }
        (evidence, changed, through)
    }

    /// The provider for the goal check (`goal.check_model`, else the chat's current model,
    /// from OpenCode Go or a provider in runtime.jsonc) and the catalog its routes and
    /// limits come from.
    async fn goal_check_config(
        &self,
        thread_id: &str,
    ) -> Result<
        (
            themis_core::providers::ProviderConfig,
            Vec<themis_core::providers::GoModel>,
        ),
        String,
    > {
        let (chat_model, project_root) = self
            .inner
            .threads
            .read()
            .await
            .get(thread_id)
            .map(|record| (record.model.clone(), record.project_root.clone()))
            .ok_or_else(|| format!("unknown thread '{thread_id}'"))?;
        // The chat's project may set `goal.check_model` too.
        let mut runtime = self
            .runtime_configuration(Some(&project_root), None)?
            .config;
        // One short request on `goal.check_model`, else the chat's model; it writes no summary.
        let model = runtime.goal.check_model.clone().unwrap_or(chat_model);
        runtime.compaction.summary_model = None;
        let config = self
            .provider_config_for(&runtime, &model, thread_id)
            .await?;
        let catalog = self.run_catalog(&runtime, &[model.as_str()]).await;
        Ok((config, catalog))
    }

    /// On start: goals that were running carry on (the cut-off turn is
    /// rechecked by the next one), goals out of quota retry, queued messages
    /// of idle chats go out.
    pub(super) async fn resume_after_boot(&self, sink: Arc<dyn EventSink>) {
        let thread_ids: Vec<String> = self.inner.threads.read().await.keys().cloned().collect();
        for thread_id in thread_ids {
            if self.dispatch_queue(Arc::clone(&sink), &thread_id).await {
                continue;
            }
            let Some(goal) = self.goal_for(&thread_id) else {
                continue;
            };
            // A check the app closed during is not still going.
            if goal.checking {
                self.update_goal(sink.as_ref(), &thread_id, |goal| goal.checking = false);
            }
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

    /// Removes the goals of deleted chats.
    pub(super) fn forget_goals(&self, thread_ids: &[String]) {
        let mut goals = self.inner.goals.lock().unwrap_or_else(|e| e.into_inner());
        for id in thread_ids {
            goals.remove(id);
            if let Some(path) = self.goal_path(id) {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

/// A long command output without its middle, marked where it was cut.
fn clip_output(output: &str) -> String {
    let count = output.chars().count();
    if count <= OUTPUT_HEAD_CHARS + OUTPUT_TAIL_CHARS {
        return output.to_owned();
    }
    let head: String = output.chars().take(OUTPUT_HEAD_CHARS).collect();
    let tail: String = output.chars().skip(count - OUTPUT_TAIL_CHARS).collect();
    let omitted = count - OUTPUT_HEAD_CHARS - OUTPUT_TAIL_CHARS;
    format!("{head}\n[… {omitted} characters clipped …]\n{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_window_note_groups_the_token_count() {
        assert_eq!(
            small_window_note(32_768),
            "Context window is small (32,768 tokens); long goals may stall."
        );
        assert_eq!(
            small_window_note(8_000),
            "Context window is small (8,000 tokens); long goals may stall."
        );
        assert_eq!(
            small_window_note(127_999),
            "Context window is small (127,999 tokens); long goals may stall."
        );
    }

    #[test]
    fn long_outputs_keep_their_start_and_their_results() {
        let table: String = (1..=200)
            .map(|n| format!("| {n} | check number {n} with its details | PASS |\n"))
            .collect();
        let output = format!("# CHECKS\n{table}Summary: 200/200 PASS");
        let clipped = clip_output(&output);
        assert!(clipped.starts_with("# CHECKS\n| 1 |"));
        assert!(clipped.ends_with("Summary: 200/200 PASS"));
        assert!(clipped.contains("characters clipped"));
        assert!(clipped.chars().count() < OUTPUT_HEAD_CHARS + OUTPUT_TAIL_CHARS + 50);
        assert_eq!(clip_output("short"), "short");
    }

    #[test]
    fn the_reason_reads_inside_a_sentence() {
        assert_eq!(
            mid_sentence("Counter is 1; needs 2."),
            "counter is 1; needs 2"
        );
        assert_eq!(mid_sentence("HTTP 500 remains"), "HTTP 500 remains");
        assert_eq!(mid_sentence(""), "");
    }

    #[test]
    fn jitter_only_adds_up_to_a_fifth_and_varies() {
        let wait = Duration::from_secs(30);
        let waits: std::collections::HashSet<Duration> = (0..50).map(|_| jittered(wait)).collect();
        assert!(waits
            .iter()
            .all(|got| *got >= wait && *got <= wait + wait / 5));
        assert!(waits.len() > 1, "{waits:?}");
    }

    #[test]
    fn backoff_doubles_up_to_fifteen_minutes() {
        let waits: Vec<u64> = (1..=8).map(|n| backoff(n).as_secs()).collect();
        assert_eq!(waits, [30, 60, 120, 240, 480, 900, 900, 900]);
        assert_eq!(backoff(0).as_secs(), 30);
        assert_eq!(backoff(u32::MAX), MAX_BACKOFF);
    }

    #[test]
    fn running_time_counts_only_while_running() {
        let mut goal = Goal {
            text: "g".into(),
            status: GoalStatus::Running,
            reason: None,
            started_at: 0,
            active_ms: 1_000,
            running_since: Some(now_ms() - 5_000),
            turns: 1,
            stalls: 0,
            progress: None,
            failures: 0,
            checked_seq: 0,
            added: Vec::new(),
            proof: Vec::new(),
            files: Vec::new(),
            checking: false,
        };
        set_status(&mut goal, GoalStatus::Paused, None);
        assert!(goal.running_since.is_none());
        assert!((6_000..7_000).contains(&goal.active_ms));
        let paused = goal.active_ms;
        set_status(&mut goal, GoalStatus::Running, None);
        assert_eq!(goal.active_ms, paused);
        assert!(goal.running_since.is_some());
    }
}
