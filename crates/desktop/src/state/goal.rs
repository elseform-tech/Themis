//! Goal mode: one objective per chat, worked on turn after turn.
//!
//! The goal lives in `goals/<chat id>.json` beside `threads.json`. Each goal
//! turn is an ordinary run. When a run ends and nothing is queued, the
//! supervisor here starts the next turn while the goal runs. A goal never gives up: it pauses
//! only when it is waiting for the user or out of model quota (and retries that); turns without
//! progress, errors and restarts never stop it.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use themis_core::goal::{self as core_goal, Failure};

use super::chat_grants::safe_chat_id;
use super::AppState;
use super::{questions, queue, threads};
use crate::sink::EventSink;
use crate::types::{Goal, GoalNoteKind, GoalStatus, ThreadEvent, ThreadEventEnvelope};

mod remember;
mod turns;

#[cfg(test)]
use remember::work_log_of;
#[cfg(test)]
use turns::{backoff, jittered};

/// Directory name under the app data directory.
pub(super) const GOALS_DIR: &str = "goals";
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
/// Longest line of a goal note (an evidence line holds a requirement and its proof).
const MAX_NOTE_LINE_CHARS: usize = 1400;
/// History items read per page while looking for the person's messages.
const HISTORY_PAGE_ITEMS: usize = 2_000;
/// How long after a goal completes Themis waits for the memory pass before filling the plan
/// record's results from the closing reply.
const PLAN_RESULTS_WAIT: Duration = Duration::from_secs(20);
/// Most history items one goal's snag scan reads.
const SNAG_SCAN_ITEMS: usize = 6_000;
/// Most characters of a failed step's output, and of what the agent said next.
const SNAG_CHARS: usize = 300;
/// Most entries of one turn's working log (its failed steps and the agent's notes), the latest.
const MAX_LOG_ENTRIES: usize = 40;
/// Most characters of one note the agent wrote between steps.
const NOTE_CHARS: usize = 400;

/// A goal note as shown: its paragraphs and lines kept (a Complete note's evidence is a list,
/// one requirement a line), each line bounded, never the whole list cut as one.
fn note_text(reason: &str) -> String {
    reason
        .split('\n')
        .map(|line| core_goal::one_line(line, MAX_NOTE_LINE_CHARS))
        .collect::<Vec<_>>()
        .join("\n")
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
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
                reason: note_text(reason),
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
            failures: 0,
            prev_turn_seq: self
                .inner
                .transcript
                .last_activity_seq(thread_id)
                .unwrap_or(0),
            turn_seq: self
                .inner
                .transcript
                .last_activity_seq(thread_id)
                .unwrap_or(0),
            start_seq: self
                .inner
                .transcript
                .last_activity_seq(thread_id)
                .unwrap_or(0),
            retry_at: None,
            evidence: Vec::new(),
            also_asked: Vec::new(),
            objective_updated: false,
        };
        self.bump_goal_generation(thread_id);
        self.store_goal(sink, thread_id, Some(goal.clone()));
        goal
    }

    /// Creates the chat's goal (the agent's `create_goal`, or the CLI). Refused while the chat
    /// has an unfinished goal. The run that created it continues; the goal's turns follow when
    /// it ends, or start now when no run is going.
    pub async fn create_goal(
        &self,
        sink: Arc<dyn EventSink>,
        thread_id: String,
        objective: String,
    ) -> Result<Goal, String> {
        let running = self.require_thread(&thread_id).await?;
        let objective = core_goal::normalize_goal(&objective)
            .ok_or("create_goal needs `objective`: the user's objective in their words")?;
        if self
            .goal_for(&thread_id)
            .is_some_and(|goal| goal.status != GoalStatus::Reached)
        {
            return Err("create_goal: this chat already has an unfinished goal; use update_goal only for its status, and the person edits its text on the goal card".to_owned());
        }
        let goal = self.begin_goal(sink.as_ref(), &thread_id, &objective);
        self.remember_goal_created(&sink, &thread_id, &goal.text);
        if !running {
            self.start_goal_turn(sink, thread_id, String::new()).await;
        }
        Ok(goal)
    }

    /// Replaces the goal's text; the next goal run is told the objective changed.
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
                if goal.text != text {
                    goal.text = text;
                    goal.objective_updated = true;
                }
            })
            .ok_or_else(|| "This chat has no goal".to_owned())?;
        let _ = self.inner.diagnostics.append(
            "goal",
            "edited",
            "info",
            json!({"thread_id": thread_id, "status": goal.status}),
        );
        self.spawn_memory_event(
            Arc::clone(&sink),
            (&thread_id, ""),
            "the goal was edited by the person",
            (
                format!("The objective is now:\n{}", goal.text),
                String::new(),
            ),
        )
        .await;
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
                if !matches!(goal.status, GoalStatus::Reached | GoalStatus::Blocked) {
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

    /// The agent's `update_goal`: complete (with its evidence) and blocked end the goal's turns;
    /// paused is accepted only when the person wrote since the current goal turn started.
    pub async fn update_goal_status(
        &self,
        sink: Arc<dyn EventSink>,
        thread_id: String,
        update: core_goal::GoalUpdate,
    ) -> Result<Goal, String> {
        self.require_thread(&thread_id).await?;
        let current = self
            .goal_for(&thread_id)
            .filter(|goal| goal.status != GoalStatus::Reached)
            .ok_or("update_goal: this chat has no unfinished goal")?;
        let (status, reason, evidence) = match update {
            core_goal::GoalUpdate::Paused => {
                if !self.person_spoke_since_goal_turn(&thread_id) {
                    return Err("update_goal: pausing is refused because the user has not asked; keep working toward the goal".to_owned());
                }
                return self.pause_goal(sink, thread_id).await;
            }
            core_goal::GoalUpdate::Blocked(reason) => {
                (GoalStatus::Blocked, Some(reason), Vec::new())
            }
            core_goal::GoalUpdate::Complete(evidence) => (GoalStatus::Reached, None, evidence),
        };
        self.bump_goal_generation(&thread_id);
        let note = reason.clone();
        let count = evidence.len();
        let goal = self
            .update_goal(sink.as_ref(), &thread_id, |goal| {
                set_status(goal, status, reason);
                goal.evidence = evidence;
                goal.retry_at = None;
            })
            .ok_or("update_goal: this chat has no unfinished goal")?;
        let _ = self.inner.diagnostics.append(
            "goal",
            if status == GoalStatus::Reached {
                "completed"
            } else {
                "blocked"
            },
            "info",
            json!({"thread_id": thread_id, "turns": current.turns, "evidence_count": count}),
        );
        match note {
            Some(reason) => self.goal_note(
                sink.as_ref(),
                &thread_id,
                "",
                GoalNoteKind::Blocked,
                &format!("Blocked: {reason}"),
            ),
            None => {
                // The evidence goes in the chat, under the note; the goal bar keeps one line.
                let mut note = "Complete.".to_owned();
                if !goal.evidence.is_empty() {
                    note.push('\n');
                    for item in &goal.evidence {
                        note.push_str(&format!(
                            "\n- {}: {}",
                            core_goal::one_line(&item.requirement, 300),
                            core_goal::one_line(&item.proof, 1000)
                        ));
                    }
                }
                self.goal_note(sink.as_ref(), &thread_id, "", GoalNoteKind::Reached, &note);
                // The work's plan record is done, with the evidence as the agent gave it: no
                // model in between, so none of it is summarized away.
                let evidence: Vec<String> = goal
                    .evidence
                    .iter()
                    .map(|item| format!("{}: {}", item.requirement, item.proof))
                    .collect();
                self.mark_plan_done(&thread_id, &evidence).await;
                let mut content = format!("Objective: {}", goal.text);
                if let Some(name) = self.plan_name_for_chat(&thread_id).await {
                    content.push_str(&format!(
                        "\nThe work's plan record: {name} (write \"done\" for it)"
                    ));
                }
                for asked in &goal.also_asked {
                    content.push_str(&format!("\nAlso asked: {asked}"));
                }
                content.push_str("\nEvidence:");
                for item in &goal.evidence {
                    content.push_str(&format!("\n- {}: {}", item.requirement, item.proof));
                }
                // What the work ran into: the steps that failed and what the agent said next,
                // read from the goal's own history (a short goal has no checkpoint).
                // What the work ran into: its failed steps and the agent's notes along the
                // way (where lessons are: a bug found, an approach dropped, a better way), the
                // latest of the whole goal.
                let snags = self.work_log(&thread_id, goal.start_seq);
                if !snags.is_empty() {
                    content.push_str(&format!(
                        "\nWhat the work ran into (failed steps and the agent's notes along the way):\n{snags}"
                    ));
                }
                // And its latest checkpoint, when there is one.
                if let Some(summary) = self.inner.transcript.checkpoint_summary(&thread_id) {
                    let notes: String = summary.chars().take(5000).collect();
                    content.push_str(&format!(
                        "\nNotes from the work (its latest checkpoint):\n{notes}"
                    ));
                }
                self.spawn_memory_event(
                    Arc::clone(&sink),
                    (&thread_id, ""),
                    "a goal was completed",
                    (content, String::new()),
                )
                .await;
            }
        }
        Ok(goal)
    }

    /// Whether the person wrote in this chat since the current goal turn started: the
    /// agent may pause a goal only because they asked.
    pub(super) fn person_spoke_since_goal_turn(&self, thread_id: &str) -> bool {
        let Some(goal) = self.goal_for(thread_id) else {
            return false;
        };
        // From the turn before: what the person sent between turns was folded into the goal
        // before this turn started, and still counts as their asking.
        let mut through = goal.prev_turn_seq.min(goal.turn_seq);
        while let Ok(page) =
            self.inner
                .transcript
                .history_after(thread_id, through, HISTORY_PAGE_ITEMS)
        {
            let full = page.len() == HISTORY_PAGE_ITEMS;
            for (seq, item) in page {
                through = seq;
                if person_spoke(&item) {
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

    /// Resumes a paused, blocked, waiting or out-of-quota goal; starts a turn
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
                GoalStatus::Blocked => themis_core::prompts::GOAL_NOTE_RESUMED_BLOCKED,
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
        sink: &Arc<dyn EventSink>,
        thread_id: &str,
        text: &str,
        run_id: &str,
        from_person: bool,
    ) -> Result<String, String> {
        let is_turn = core_goal::turn_number(text).is_some();
        if let Some(sentence) = core_goal::parse_goal_command(text) {
            if sentence.is_empty() {
                return Err("Add the goal after /goal, for example: /goal speed up p95".to_owned());
            }
            // Themis creates it from the command; no model decides. This run is its first turn.
            let goal = self.begin_goal(sink.as_ref(), thread_id, &sentence);
            self.remember_goal_created(sink, thread_id, &goal.text);
            self.update_goal(sink.as_ref(), thread_id, |goal| goal.turns = 1);
            self.mark_goal_turn(run_id);
            return Ok(format!(
                "\n\n{}",
                self.take_goal_note(sink.as_ref(), thread_id)
            ));
        } else if !is_turn {
            if from_person {
                self.add_also_asked(sink, thread_id, text);
            }
            let sink = sink.as_ref();
            match self.goal_for(thread_id).map(|goal| goal.status) {
                // An answer to another question leaves the goal waiting on
                // the blocking one; any other reply resumes it.
                Some(GoalStatus::Waiting)
                    if !(super::questions::is_answer(text)
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
        let sink = sink.as_ref();
        Ok(match self.goal_for(thread_id) {
            // A goal turn's saved message is one short line; the note rides along with each run
            // only, so history doesn't grow by a note per turn and every run reads it fresh.
            Some(goal) if goal.status == GoalStatus::Running && is_turn => {
                format!("\n\n{}", self.take_goal_note(sink, thread_id))
            }
            Some(goal) if goal.status == GoalStatus::Running => {
                core_goal::goal_context(&self.take_goal_note(sink, thread_id))
            }
            // Old goal turns in the chat read as unfinished work unless this says otherwise.
            Some(goal) if !is_turn => core_goal::paused_context(&goal.text),
            None if !is_turn && self.inner.transcript.had_goal(thread_id) => {
                core_goal::over_context()
            }
            _ => String::new(),
        })
    }

    /// What the person sent while the goal runs (or waits on them, or on quota) joins it word
    /// for word as an "Also asked" line, by plain code, so no checkpoint can lose it. Answers to
    /// the agent's questions, goal turns and `/goal` are not requests.
    /// Whether the person's message `text` reaches memory through a goal event (the goal it
    /// created, or an "Also asked" line), so the turn's own pass leaves it out. Answers to
    /// the agent's questions still go through the turn's pass, with the question they answer.
    pub(super) fn goal_takes_words(&self, thread_id: &str, text: &str) -> bool {
        self.goal_for(thread_id).is_some_and(|goal| {
            matches!(
                goal.status,
                GoalStatus::Running | GoalStatus::Waiting | GoalStatus::Quota
            )
        }) && !super::questions::is_answer(text)
            && core_goal::turn_number(text).is_none()
    }

    /// The goal note this goal run carries ([`core_goal::goal_note`]): the "objective updated"
    /// note once after an edit, then the continuation with the objective and its "Also asked"
    /// lines.
    fn take_goal_note(&self, sink: &dyn EventSink, thread_id: &str) -> String {
        let mut note = String::new();
        self.update_goal(sink, thread_id, |goal| {
            note = core_goal::goal_note(
                &core_goal::objective(&goal.text, &goal.also_asked),
                goal.objective_updated,
            );
            goal.objective_updated = false;
        });
        note
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

/// Whether a chat history item is the person speaking: a message or a steer, but not a goal
/// turn, a goal command, a Vision's message or steer, or a Vision's answer (queued, it is a
/// message; given while a run was going, it is a steer).
fn person_spoke(item: &crate::transcript::HistoryItem) -> bool {
    match item {
        crate::transcript::HistoryItem::User {
            text, vision_id, ..
        } => {
            vision_id.is_none()
                && !text.starts_with(themis_core::goal::GOAL_TURN_MARKER)
                && !text.starts_with("/goal")
                && !super::questions::is_vision_answer(text)
        }
        crate::transcript::HistoryItem::Event { envelope } => match &envelope.event {
            ThreadEvent::Steered { text, vision_id } => {
                vision_id.is_none() && !super::questions::is_vision_answer(text)
            }
            _ => false,
        },
        crate::transcript::HistoryItem::Legacy { .. } => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Vision's answer is not the person speaking, whether it came as a message (queued) or
    /// was steered into a running turn; the person's answer and words are.
    #[test]
    fn a_visions_answer_never_counts_as_the_person_speaking() {
        use crate::transcript::HistoryItem;
        let vision = format!(
            "{}\nQuestion: Tabs?\nAnswer: Tabs",
            super::super::questions::vision_answer_marker("Wren")
        );
        let person = format!(
            "{}\nQuestion: Tabs?\nAnswer: Tabs",
            super::super::questions::ANSWER_MARKER
        );
        let user = |text: &str| HistoryItem::User {
            run_id: "r".into(),
            text: text.to_owned(),
            attachments: Vec::new(),
            vision_id: None,
        };
        let steered = |text: &str| HistoryItem::Event {
            envelope: ThreadEventEnvelope {
                thread_id: "t".into(),
                run_id: "r".into(),
                event: ThreadEvent::Steered {
                    text: text.to_owned(),
                    vision_id: None,
                },
            },
        };
        for item in [user(&vision), steered(&vision), user("/goal pause")] {
            assert!(!person_spoke(&item));
        }
        for item in [
            user(&person),
            steered(&person),
            user("stop"),
            steered("stop"),
        ] {
            assert!(person_spoke(&item));
        }
        // A message a Vision steered in (it queued it) is its words, however plain.
        let by_vision = HistoryItem::Event {
            envelope: ThreadEventEnvelope {
                thread_id: "t".into(),
                run_id: "r".into(),
                event: ThreadEvent::Steered {
                    text: "stop".into(),
                    vision_id: Some("v1".into()),
                },
            },
        };
        assert!(!person_spoke(&by_vision));
    }

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
    fn jitter_only_adds_up_to_a_fifth_and_varies() {
        let wait = Duration::from_secs(30);
        let waits: std::collections::HashSet<Duration> = (0..50).map(|_| jittered(wait)).collect();
        assert!(waits
            .iter()
            .all(|got| *got >= wait && *got <= wait + wait / 5));
        assert!(waits.len() > 1, "{waits:?}");
    }

    /// A live goal's Complete note lost all but its first evidence lines: the list was one
    /// paragraph cut at 800 characters.
    #[test]
    fn a_complete_note_keeps_every_evidence_line() {
        let lines: Vec<String> = (0..13)
            .map(|n| format!("- requirement {n}: {}", "proof ".repeat(80)))
            .collect();
        let note = note_text(&format!("Complete.\n\n{}", lines.join("\n")));
        assert_eq!(
            note.lines()
                .filter(|line| line.starts_with("- requirement"))
                .count(),
            13
        );
        assert!(note.starts_with("Complete.\n\n- requirement 0: "));
        assert!(note.contains("\n- requirement 12: "));
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
            failures: 0,
            prev_turn_seq: 0,
            turn_seq: 0,
            start_seq: 0,
            retry_at: None,
            evidence: Vec::new(),
            also_asked: Vec::new(),
            objective_updated: false,
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

#[cfg(test)]
mod snag_tests {
    use super::*;

    fn tool(ok: bool, output: &str) -> ThreadEvent {
        ThreadEvent::ToolFinished {
            tool: "shell".into(),
            ok,
            output: output.into(),
            media: None,
        }
    }

    fn said(text: &str) -> ThreadEvent {
        ThreadEvent::AssistantText { text: text.into() }
    }

    /// A turn's log holds what a failed step alone never shows: the agent's notes on what it
    /// found wrong or dropped, in order with the failures, without the reply that ended it.
    #[test]
    fn a_turns_log_keeps_the_agents_notes_with_its_failed_steps() {
        let events = vec![
            said("Starting the asteroid pool."),
            tool(true, r#"{"exit_code":0,"output":"ok"}"#),
            said("The HUD reads visible on the start screen, which doesn't match: show() ran before reset()."),
            tool(true, r#"{"exit_code":1,"output":"1 failing"}"#),
            said("Dropped per-frame Vector3 allocation; one reused vector is enough."),
            said("Done: the game works."),
        ];
        let log = work_log_of(&events);
        let lines: Vec<&str> = log.lines().collect();
        assert_eq!(lines.len(), 4, "{log}");
        assert!(
            lines[1].starts_with("- noted: The HUD reads visible"),
            "{log}"
        );
        assert!(lines[2].starts_with("- shell failed:"), "{log}");
        assert!(lines[3].contains("Dropped per-frame Vector3"), "{log}");
        assert!(!log.contains("Done: the game works"), "{log}");
        // The latest MAX_LOG_ENTRIES, each note bounded.
        let many: Vec<ThreadEvent> = (0..100)
            .map(|n| said(&format!("note {n} {}", "y".repeat(2000))))
            .collect();
        let log = work_log_of(&many);
        assert_eq!(log.lines().count(), MAX_LOG_ENTRIES);
        assert!(
            log.lines().last().unwrap().starts_with("- noted: note 98"),
            "{log}"
        );
        assert!(log
            .lines()
            .all(|line| line.chars().count() <= NOTE_CHARS + 20));
        assert_eq!(work_log_of(&[]), "");
    }
}
