//! What a goal hands to memory and its plan record: work logs, plan results and mid-goal requests.

use super::*;

impl AppState {
    /// The memory pass for a goal just created: the approved plan is saved now, not after
    /// the goal's first (long) turn. It reads the objective, the person's latest words and
    /// the reply they answered (the plan).
    pub(super) fn remember_goal_created(
        &self,
        sink: &Arc<dyn EventSink>,
        thread_id: &str,
        objective: &str,
    ) {
        let transcript = &self.inner.transcript;
        let mut content = format!("The objective:\n{objective}");
        if let Some(person) = transcript.last_turn_text(thread_id, "user") {
            let person: String = person.chars().take(4000).collect();
            content.push_str(&format!("\nThe person's latest message:\n{person}"));
        }
        // The reply it answered (the plan, when there is one) goes as the reply answered, so a
        // plan record copies it whole.
        let answered = transcript.last_reply_text(thread_id);
        self.spawn_goal_memory_event(sink, thread_id, "a goal was created", (content, answered));
    }

    /// One turn's working log for its memory pass: see [`work_log_of`].
    pub(in crate::state) fn work_log(&self, thread_id: &str, from_seq: i64) -> String {
        work_log_of(&self.events_since(thread_id, from_seq))
    }

    /// The chat's events after `from_seq`, at most [`SNAG_SCAN_ITEMS`].
    fn events_since(&self, thread_id: &str, from_seq: i64) -> Vec<ThreadEvent> {
        let mut events = Vec::new();
        let mut through = from_seq;
        while events.len() < SNAG_SCAN_ITEMS {
            let Ok(page) =
                self.inner
                    .transcript
                    .history_after(thread_id, through, HISTORY_PAGE_ITEMS)
            else {
                break;
            };
            let Some((last, _)) = page.last() else { break };
            through = *last;
            let full = page.len() == HISTORY_PAGE_ITEMS;
            events.extend(page.into_iter().filter_map(|(_, item)| match item {
                crate::transcript::HistoryItem::Event { envelope } => Some(envelope.event),
                _ => None,
            }));
            if !full {
                break;
            }
        }
        events
    }

    /// The name of the plan record this chat's goal builds, if it has one.
    pub(super) async fn plan_name_for_chat(&self, thread_id: &str) -> Option<String> {
        let root = self.thread_project_root(thread_id).await?;
        let store = self.plan_store(std::path::Path::new(&root)).ok()?;
        store.for_chat(thread_id).map(|record| record.name)
    }

    /// Fills the results of the plan record this chat's goal built from the closing reply, if
    /// they are still empty a moment after the goal-complete memory pass had its chance; logs a
    /// record left without results or lessons.
    pub(super) async fn plan_results_from_reply(&self, thread_id: &str, reply: &str) {
        let Some(root) = self.thread_project_root(thread_id).await else {
            return;
        };
        let (state, thread_id, reply) = (self.clone(), thread_id.to_owned(), reply.to_owned());
        tokio::spawn(async move {
            // The goal-complete pass runs on its own task; give it time to write first.
            tokio::time::sleep(PLAN_RESULTS_WAIT).await;
            let _guard = state.inner.memory_job_lock.lock().await;
            let Ok(store) = state.plan_store(std::path::Path::new(&root)) else {
                return;
            };
            let Some(mut record) = store.for_chat(&thread_id) else {
                return;
            };
            let filled = record.results.is_empty() && !reply.trim().is_empty();
            if filled {
                record.done(&reply, &[], &[], &themis_core::memory::today());
                let _ = store.save(&record, false);
            }
            if filled || record.lessons.is_empty() {
                let _ = state.inner.diagnostics.append(
                    "memory",
                    "plan",
                    "info",
                    json!({"thread_id": thread_id, "op": "done", "by": "goal_reply", "outcome": "no_done", "results_filled": filled, "lessons": record.lessons.len()}),
                );
            }
        });
    }

    /// Marks the plan record this chat's goal built as done, with `evidence`.
    pub(super) async fn mark_plan_done(&self, thread_id: &str, evidence: &[String]) {
        let Some(root) = self.thread_project_root(thread_id).await else {
            return;
        };
        let Ok(store) = self.plan_store(std::path::Path::new(&root)) else {
            return;
        };
        let Some(mut record) = store.for_chat(thread_id) else {
            return;
        };
        record.done("", evidence, &[], &themis_core::memory::today());
        let outcome = if store.save(&record, false).is_ok() {
            "updated"
        } else {
            "failed"
        };
        let _ = self.inner.diagnostics.append(
            "memory",
            "plan",
            "info",
            json!({"thread_id": thread_id, "op": "done", "by": "goal", "outcome": outcome, "evidence": evidence.len()}),
        );
    }

    fn spawn_goal_memory_event(
        &self,
        sink: &Arc<dyn EventSink>,
        thread_id: &str,
        what: &'static str,
        content: (String, String),
    ) {
        let (state, sink, thread_id) = (self.clone(), Arc::clone(sink), thread_id.to_owned());
        tokio::spawn(async move {
            state
                .spawn_memory_event(sink, (&thread_id, ""), what, content)
                .await;
        });
    }

    pub(in crate::state) fn add_also_asked(
        &self,
        sink: &Arc<dyn EventSink>,
        thread_id: &str,
        text: &str,
    ) {
        let active = self.goal_for(thread_id).is_some_and(|goal| {
            matches!(
                goal.status,
                GoalStatus::Running | GoalStatus::Waiting | GoalStatus::Quota
            )
        });
        if !active
            || text.starts_with(super::questions::ANSWER_MARKER)
            || core_goal::turn_number(text).is_some()
            || core_goal::parse_goal_command(text).is_some()
        {
            return;
        }
        let Some(line) = core_goal::also_asked(text) else {
            return;
        };
        let mut kept = false;
        let said = line.clone();
        let goal = self.update_goal(sink.as_ref(), thread_id, |goal| {
            if goal.also_asked.len() < core_goal::MAX_ALSO_ASKED {
                goal.also_asked.push(line.clone());
                kept = true;
            }
        });
        let _ = self.inner.diagnostics.append(
            "goal",
            if kept {
                "also_asked"
            } else {
                "also_asked_full"
            },
            "info",
            json!({"thread_id": thread_id, "count": goal.as_ref().map_or(0, |goal| goal.also_asked.len())}),
        );
        // What the person asks mid-goal is remembered now, not after the goal's long turn.
        if let Some(goal) = goal {
            self.spawn_goal_memory_event(
                sink,
                thread_id,
                "the person changed or added to the goal while it ran",
                (
                    format!("The person said:\n{said}\nThe objective:\n{}", goal.text),
                    String::new(),
                ),
            );
        }
    }
}

/// Whether a finished tool step failed: refused, or a command that exited non-zero.
fn step_failed(ok: bool, output: &str) -> bool {
    if !ok {
        return true;
    }
    serde_json::from_str::<serde_json::Value>(output)
        .ok()
        .and_then(|value| value["exit_code"].as_i64())
        .is_some_and(|code| code != 0)
}

/// The end of `text` on one line, at most [`SNAG_CHARS`] characters: an error is at the end.
fn tail(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let count = flat.chars().count();
    flat.chars()
        .skip(count.saturating_sub(SNAG_CHARS))
        .collect()
}

/// One turn's working log for its memory pass, in order: each failed step, and each note the
/// agent wrote between steps (what it tried, found wrong, dropped or thinks could be better),
/// which a failed step alone never shows. The note that ended the turn is its reply, read
/// elsewhere. The latest [`MAX_LOG_ENTRIES`], each bounded.
pub(super) fn work_log_of(events: &[ThreadEvent]) -> String {
    let mut entries: Vec<String> = Vec::new();
    let mut notes: Vec<usize> = Vec::new();
    for event in events {
        match event {
            ThreadEvent::ToolFinished {
                tool, ok, output, ..
            } if step_failed(*ok, output) && !tool.starts_with("hook_") && tool != "Skill" => {
                entries.push(format!("- {tool} failed: {}", tail(output)));
            }
            ThreadEvent::AssistantText { text } if !text.trim().is_empty() => {
                notes.push(entries.len());
                entries.push(format!("- noted: {}", head(text, NOTE_CHARS)));
            }
            _ => {}
        }
    }
    if let Some(&reply) = notes.last() {
        if reply == entries.len() - 1 {
            entries.pop();
        }
    }
    let skip = entries.len().saturating_sub(MAX_LOG_ENTRIES);
    entries
        .into_iter()
        .skip(skip)
        .collect::<Vec<_>>()
        .join("\n")
}

/// The first `limit` characters of `text` on one line, marked when cut.
fn head(text: &str, limit: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= limit {
        return flat;
    }
    let mut cut: String = flat.chars().take(limit).collect();
    cut.push('…');
    cut
}
