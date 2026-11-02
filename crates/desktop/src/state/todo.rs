//! The agent's to-do list: `todo_write` for the main agent and for subagents, and the main
//! agent's list per chat in `todos/<chat id>.json` beside `goals/`, outside the chat history so
//! compaction and restarts keep it. A subagent's list lives as long as its run.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use serde_json::{json, Value};
use themis_core::todo::{self as core_todo, TodoList, WriteSummary};
use themis_core::tools::{async_trait, ToolCallError, ToolRuntime, ToolT};

use super::chat_grants::safe_chat_id;
use super::subagents::{Reporter, Subagent};
use super::AppState;
use crate::sink::EventSink;
use crate::types::{ThreadEvent, ThreadEventEnvelope};

/// Directory name under the app data directory.
pub(super) const TODOS_DIR: &str = "todos";

fn error(message: impl ToString) -> ToolCallError {
    ToolCallError::RuntimeError(message.to_string().into())
}

impl AppState {
    fn todo_path(&self, thread_id: &str) -> Option<PathBuf> {
        safe_chat_id(thread_id).then(|| self.inner.todos_dir.join(format!("{thread_id}.json")))
    }

    /// The chat's list (empty when it has none). A damaged file counts as an empty list and is
    /// noted in diagnostics with a fixed reason.
    pub fn todo_for(&self, thread_id: &str) -> TodoList {
        let mut todos = self.inner.todos.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(list) = todos.get(thread_id) {
            return list.clone();
        }
        let list = match self.todo_path(thread_id).map(std::fs::read_to_string) {
            Some(Ok(text)) => serde_json::from_str(&text).unwrap_or_else(|_| {
                let _ = self.inner.diagnostics.append(
                    "todo",
                    "todo_ignored",
                    "warn",
                    json!({"thread_id": thread_id, "reason": "unreadable to-do file"}),
                );
                TodoList::default()
            }),
            _ => TodoList::default(),
        };
        todos.insert(thread_id.to_owned(), list.clone());
        list
    }

    /// The chat's list, for clients.
    pub async fn get_todo(&self, thread_id: String) -> Result<TodoList, String> {
        self.require_thread(&thread_id).await?;
        Ok(self.todo_for(&thread_id))
    }

    /// Changes the chat's list with `change`, then saves and announces it when its revision
    /// moved. Changes are serialized per app.
    pub(super) fn update_todo<R>(
        &self,
        sink: &dyn EventSink,
        thread_id: &str,
        change: impl FnOnce(&mut TodoList) -> R,
    ) -> (TodoList, R) {
        let _guard = self
            .inner
            .todo_lock
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut list = self.todo_for(thread_id);
        let before = list.revision;
        let result = change(&mut list);
        if list.revision != before {
            self.inner
                .todos
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(thread_id.to_owned(), list.clone());
            if let Some(path) = self.todo_path(thread_id) {
                if let Err(error) = super::write_json_file(&path, &list) {
                    self.record_error("save_todo", error);
                }
            }
            sink.emit_thread_event(&ThreadEventEnvelope {
                thread_id: thread_id.to_owned(),
                run_id: String::new(),
                event: ThreadEvent::TodoChanged { todo: list.clone() },
            });
        }
        (list, result)
    }

    /// Metadata only: counts, never what the items say.
    pub(super) fn log_todo(&self, thread_id: &str, event: &str, by: &str, list: &TodoList) {
        let open = list.open().count();
        let _ = self.inner.diagnostics.append(
            "todo",
            event,
            "info",
            json!({"thread_id": thread_id, "by": by, "items": list.items.len(), "open": open,
                "done": list.items.len() - open, "removed_requests": list.removed_requests.len()}),
        );
    }

    /// The list as the model reads it at the start of a run; empty when there is none.
    pub(super) fn todo_note(&self, thread_id: &str) -> String {
        self.todo_for(thread_id)
            .note()
            .map_or_else(String::new, |note| format!("\n\n{note}"))
    }

    /// Adds a request the person sent while the chat's goal is open, in their words.
    pub(super) fn add_request_to_todo(&self, sink: &dyn EventSink, thread_id: &str, text: &str) {
        let (list, added) = self.update_todo(sink, thread_id, |list| list.add_request(text));
        if added {
            self.log_todo(thread_id, "request_added", "person", &list);
        }
    }

    /// Removes the lists of deleted chats.
    pub(super) fn forget_todos(&self, thread_ids: &[String]) {
        let mut todos = self.inner.todos.lock().unwrap_or_else(|e| e.into_inner());
        for id in thread_ids {
            todos.remove(id);
            if let Some(path) = self.todo_path(id) {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

/// Whose list a `todo_write` keeps.
pub(super) enum TodoOwner {
    /// The main agent's, saved with the chat. `seen` is the list's revision as the agent last
    /// saw it (the run's opening note, its own writes, a checkpoint's note).
    Chat {
        state: AppState,
        sink: Arc<dyn EventSink>,
        thread_id: String,
        seen: Arc<AtomicU64>,
    },
    /// A subagent's, for its run; the item in progress is its status line.
    Subagent {
        agent: Arc<Subagent>,
        report: Reporter,
        list: std::sync::Mutex<TodoList>,
    },
}

/// `todo_write`: the agent's own to-do list, rewritten whole on each call.
pub(super) struct TodoTool {
    pub(super) owner: TodoOwner,
    /// A deny rule covers it.
    pub(super) denied: bool,
}

impl std::fmt::Debug for TodoTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TodoTool").finish_non_exhaustive()
    }
}

/// Each item's words and status, for plugin `TaskCreated` / `TaskCompleted` hooks.
fn plan(titles: impl Iterator<Item = (String, core_todo::TodoStatus)>) -> Vec<(String, String)> {
    titles
        .map(|(title, status)| (title, status.as_str().to_owned()))
        .collect()
}

impl TodoTool {
    fn current(&self) -> TodoList {
        match &self.owner {
            TodoOwner::Chat {
                state, thread_id, ..
            } => state.todo_for(thread_id),
            TodoOwner::Subagent { list, .. } => {
                list.lock().unwrap_or_else(|e| e.into_inner()).clone()
            }
        }
    }

    /// Applies a validated write; the list afterwards and what changed.
    fn apply(&self, written: Vec<core_todo::WrittenItem>) -> (TodoList, WriteSummary) {
        match &self.owner {
            TodoOwner::Chat {
                state,
                sink,
                thread_id,
                seen,
            } => {
                let seen_at = seen.load(Ordering::SeqCst);
                let (list, summary) = state.update_todo(sink.as_ref(), thread_id, |list| {
                    list.apply_write(written, seen_at)
                });
                seen.store(list.revision, Ordering::SeqCst);
                state.log_todo(thread_id, "updated", "agent", &list);
                if summary.removals_forgotten > 0 {
                    let _ = state.inner.diagnostics.append(
                        "todo",
                        "removed_requests_forgotten",
                        "warn",
                        json!({"thread_id": thread_id, "count": summary.removals_forgotten}),
                    );
                }
                (list, summary)
            }
            TodoOwner::Subagent {
                agent,
                report,
                list,
            } => {
                let mut list = list.lock().unwrap_or_else(|e| e.into_inner());
                let summary = list.apply_write(written, u64::MAX);
                if let Some(current) = list
                    .items
                    .iter()
                    .find(|item| item.status == core_todo::TodoStatus::InProgress)
                {
                    agent.set_status(&current.title);
                    report(agent);
                }
                (list.clone(), summary)
            }
        }
    }
}

#[async_trait]
impl ToolRuntime for TodoTool {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        if self.denied {
            return Err(error(
                "approval denied for 'todo_write': the approval rules deny it",
            ));
        }
        let before = self.current();
        let written = core_todo::parse_write(&args, before.write_limit()).map_err(error)?;
        // A plugin's `TaskCreated` and `TaskCompleted` hooks may refuse: the list stays as it was.
        themis_core::plugins::hooks::plan_changed(
            &plan(
                before
                    .items
                    .iter()
                    .map(|item| (item.title.clone(), item.status)),
            ),
            &plan(written.iter().map(|item| (item.title.clone(), item.status))),
        )
        .await
        .map_err(|refused| error(refused.to_string()))?;
        let known: std::collections::HashSet<String> =
            before.items.iter().map(|item| item.id.clone()).collect();
        let sent: std::collections::HashSet<String> =
            written.iter().filter_map(|item| item.id.clone()).collect();
        let (list, summary) = self.apply(written);
        // The ids the agent doesn't know yet: its new items and requests it hadn't seen.
        let new: Vec<Value> = list
            .items
            .iter()
            .filter(|item| !known.contains(&item.id) || !sent.contains(&item.id))
            .map(|item| json!({"id": item.id, "title": item.title, "status": item.status}))
            .collect();
        Ok(json!({
            "kept": summary.items,
            "open": summary.open,
            "done": summary.completed,
            "new_ids": new,
        }))
    }
}

impl ToolT for TodoTool {
    fn name(&self) -> &str {
        "todo_write"
    }
    fn description(&self) -> &str {
        themis_core::prompts::TODO_TOOL
    }
    fn args_schema(&self) -> Value {
        json!({"type":"object","properties":{"todos":{"type":"array","items":{
            "type":"object","properties":{
                "id":{"type":"string"},
                "title":{"type":"string","maxLength":core_todo::MAX_TITLE_CHARS},
                "details":{"type":"string","maxLength":core_todo::MAX_DETAILS_CHARS},
                "status":{"type":"string","enum":["pending","in_progress","completed"]},
                "outcome":{"type":"string","maxLength":core_todo::MAX_OUTCOME_CHARS}},
            "required":["title","status"]}}},"required":["todos"],"additionalProperties":false})
    }
    fn output_schema(&self) -> Option<Value> {
        None
    }
}

#[cfg(test)]
mod scale_tests {
    use super::*;
    use crate::scale::{growth_rate, measure};
    use themis_core::goal::{self as core_goal, Verdict};
    use themis_core::todo::{TodoSource, TodoStatus, MAX_CHECK_CHARS, MAX_NOTE_CHARS};

    const THREAD: &str = "2b9f0e6a-1c7d-4e55-9a10-6f3d2c8b7a41";

    /// The list the agent read, rewritten: kept whole, with `finish` marked completed and
    /// `drop` taken off, plus `extra`.
    fn rewrite(
        state: &AppState,
        finish: impl Fn(&core_todo::TodoItem) -> bool,
        drop: impl Fn(&core_todo::TodoItem) -> bool,
        extra: Option<Value>,
    ) -> Value {
        let mut todos: Vec<Value> = state
            .todo_for(THREAD)
            .items
            .iter()
            .filter(|item| !drop(item))
            .map(|item| {
                let status = if finish(item) {
                    "completed"
                } else {
                    item.status.as_str()
                };
                json!({"id": item.id, "title": item.title, "status": status})
            })
            .collect();
        todos.extend(extra);
        json!({ "todos": todos })
    }

    /// A goal check that would say reached: the bytes of its prompt, what it decides once it
    /// has refereed the list, and the list afterwards.
    fn check(state: &AppState, sink: &dyn EventSink) -> (usize, Verdict, TodoList) {
        let goal = state.goal_for(THREAD).unwrap();
        let list = state.todo_for(THREAD);
        let (_, user) =
            core_goal::check_prompt(&goal.text, &goal.rules, &list, "answer", "evidence");
        let removals = list.removed_requests.len();
        let mut verdict = core_goal::GoalVerdict::undecided("Done.");
        verdict.verdict = Verdict::Reached;
        let (updated, ()) = state.update_todo(sink, THREAD, |list| {
            verdict.referee(list);
            list.judged_removals(removals);
        });
        (user.len(), verdict.verdict, updated)
    }

    /// One goal with detours and `requests` incoming requests (every tenth cancels the one
    /// before it): the largest goal-check prompt and turn note in bytes, and whether the goal
    /// ended reached with the first request still on the list.
    fn goal_with_requests(requests: usize) -> (usize, usize, bool) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async move {
            let data = tempfile::tempdir().unwrap();
            let state = AppState::new_for_test(data.path().join("settings.json"));
            let (sink, _rx) = crate::sink::ChannelSink::channel();
            let sink: Arc<dyn EventSink> = Arc::new(sink);
            state.begin_goal(sink.as_ref(), THREAD, "Build the planner");
            let seen = Arc::new(AtomicU64::new(0));
            let tool = TodoTool {
                owner: TodoOwner::Chat {
                    state: state.clone(),
                    sink: Arc::clone(&sink),
                    thread_id: THREAD.to_owned(),
                    seen: Arc::clone(&seen),
                },
                denied: false,
            };
            let plan: Vec<Value> = (0..5)
                .map(|n| json!({"title": format!("Planned step {n}"), "details": "what done means ".repeat(20), "status": "pending"}))
                .collect();
            tool.execute(json!({ "todos": plan })).await.unwrap();
            let (mut check_max, mut note_max) = (0, 0);
            for n in 0..requests {
                let cancels = n % 10 == 9;
                let words = if cancels {
                    format!("Forget request {}; I don't need it any more.", n - 1)
                } else {
                    format!("request {n}: {}", "please make this part work well ".repeat(12))
                };
                state.note_added_request(sink.as_ref(), THREAD, &words);
                // Each request starts a run, which opens with the list as it stands.
                seen.store(state.todo_for(THREAD).revision, Ordering::SeqCst);
                let mine = format!("request {n}:");
                let cancelled = format!("request {}:", n.saturating_sub(1));
                let write = rewrite(
                    &state,
                    |item| item.source == TodoSource::Agent || item.details.starts_with(&mine),
                    |item| {
                        // The agent lets go of its finished detours to make room.
                        item.title.starts_with("Detour")
                            || cancels
                                && (item.details.starts_with(&cancelled)
                                    || item.details.starts_with("Forget request"))
                    },
                    // A detour every fifth request, finished in the same write.
                    (n % 5 == 0).then(|| {
                        json!({"title": format!("Detour {n}"), "status": "completed", "outcome": "fixed"})
                    }),
                );
                tool.execute(write).await.unwrap();
                note_max = note_max.max(state.todo_note(THREAD).len());
                let (bytes, verdict, _) = check(&state, sink.as_ref());
                check_max = check_max.max(bytes);
                assert_eq!(verdict, Verdict::Reached, "a cancelled request holds nothing: {:?}", state.todo_for(THREAD).open().collect::<Vec<_>>());
            }
            let (_, verdict, list) = check(&state, sink.as_ref());
            assert!(list
                .items
                .iter()
                .all(|item| item.status == TodoStatus::Completed));
            let first = list.items.iter().any(|item| {
                item.source == TodoSource::Person && item.details.starts_with("request 0:")
            });
            (check_max, note_max, verdict == Verdict::Reached && first)
        })
    }

    /// A goal with detours and many requests, some cancelling earlier ones: the first request
    /// is still on the list at the end, cancelled ones don't hold the goal, and what the check
    /// and the turn note read stays bounded however many requests came in.
    #[test]
    fn many_requests_stay_on_the_list_and_what_the_check_reads_stays_bounded() {
        let checks = measure(&[10, 50], |requests| goal_with_requests(requests).0 as u64);
        let notes = measure(&[10, 50], |requests| goal_with_requests(requests).1 as u64);
        assert!(
            goal_with_requests(50).2,
            "reached with the first request kept"
        );
        eprintln!("check prompt bytes {checks:?}; turn note bytes {notes:?}");
        // Characters may take up to 4 bytes; the prompt around the list is a few KB.
        assert!(checks
            .iter()
            .all(|(_, bytes)| *bytes <= (MAX_CHECK_CHARS as u64 + 4_000) * 4));
        assert!(notes
            .iter()
            .all(|(_, bytes)| *bytes <= (MAX_NOTE_CHARS as u64 + 400) * 4));
        assert!(growth_rate(&checks) <= 1.0, "{checks:?}");
        assert!(growth_rate(&notes) <= 1.0, "{notes:?}");
    }
}
