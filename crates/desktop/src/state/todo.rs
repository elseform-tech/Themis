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

/// Most later messages a request chain keeps; the oldest go first.
const MAX_CHAIN_REQUESTS: usize = 8;

/// The person's messages since they last wrote to an idle chat, after that opening message:
/// what they steered in or queued while the chain's runs went on, each with the run it reached.
/// A goal the agent sets in the chain (with `set_goal`, usually on the opening message) takes
/// them on as requests. In memory only, and dropped once a goal took them.
#[derive(Default)]
pub(super) struct RequestChain {
    later: Vec<(String, String)>,
    /// A run of the chain is going, or a queued message of the person's follows one.
    open: bool,
}

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
        let mut list = match self.todo_path(thread_id).map(std::fs::read_to_string) {
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
        // Saved by an earlier start of the app: no turn of this one is working on a request.
        list.end_turn();
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

    /// Notes a message of the person's that reached run `run_id`: one that `continues` the
    /// chain (steered in, or queued while a run went on) joins it; one written to an idle chat
    /// opens a new chain. Answers to the agent's questions and a plugin hook's wake-up turn are
    /// not requests.
    pub(super) fn chain_request(&self, thread_id: &str, run_id: &str, text: &str, continues: bool) {
        let text = text.trim();
        if text.is_empty()
            || text.starts_with(super::questions::ANSWER_MARKER)
            || text.starts_with(themis_core::plugins::hooks::HOOK_WAKE_MARKER)
        {
            return;
        }
        let mut chains = self
            .inner
            .request_chains
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let chain = chains.entry(thread_id.to_owned()).or_default();
        // A message that reaches a chat whose runs had all ended (even one queued there and sent
        // at once) opens a new chain; only one sent while the chain's runs go on continues it.
        if !continues || !chain.open {
            chain.later.clear();
            chain.open = true;
            return;
        }
        if chain.later.len() >= MAX_CHAIN_REQUESTS {
            chain.later.remove(0);
        }
        chain.later.push((run_id.to_owned(), text.to_owned()));
    }

    /// The chat's run ended with nothing of the person's queued after it: the chain is over,
    /// and the next message opens a new one. What it holds waits for that message.
    pub(super) fn close_request_chain(&self, thread_id: &str) {
        if let Some(chain) = self
            .inner
            .request_chains
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_mut(thread_id)
        {
            chain.open = false;
        }
    }

    /// A goal was just set for a chat that had none open: the person's messages after the
    /// one that opened this chain become "you asked" items (the opener asked for the goal);
    /// one that reached the running turn shows in progress for it. Words already on the list
    /// are not added again.
    pub(super) fn requests_join_new_goal(&self, sink: &dyn EventSink, thread_id: &str) {
        let Some(chain) = self
            .inner
            .request_chains
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(thread_id)
        else {
            return;
        };
        let running = self
            .inner
            .steer_inboxes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(thread_id)
            .map(|entry| entry.run_id.clone());
        for (run_id, text) in &chain.later {
            self.note_added_request(sink, thread_id, text);
            if running.as_deref() == Some(run_id.as_str()) {
                self.start_request_on_todo(sink, thread_id, text);
            }
        }
    }

    /// A turn works on the person's message: their pending request in those words shows in
    /// progress until the turn ends (see [`Self::end_todo_turn`]).
    pub(super) fn start_request_on_todo(&self, sink: &dyn EventSink, thread_id: &str, text: &str) {
        let (list, started) =
            self.update_todo(sink, thread_id, |list| list.start_request(text.trim()));
        if started {
            self.log_todo(thread_id, "request_started", "runtime", &list);
        }
    }

    /// The turn ended: requests Themis put in progress for it go back to pending unless the
    /// agent gave them a status itself. Never completes anything.
    pub(super) fn end_todo_turn(&self, sink: &dyn EventSink, thread_id: &str) {
        let (list, settled) = self.update_todo(sink, thread_id, TodoList::end_turn);
        if settled {
            self.log_todo(thread_id, "request_turn_ended", "runtime", &list);
        }
    }

    /// `goal.seed_todo`: a goal that just started on a list with nothing planned gets one item
    /// per thing it asks for (after at most two real preliminary steps) and a final "Verify end
    /// to end", from one bounded request on the summary model. Anything that goes wrong leaves
    /// the list as it was; diagnostics carry the outcome, counts and tokens only.
    pub(super) async fn seed_goal_todo(
        &self,
        sink: &dyn EventSink,
        (thread_id, run_id): (&str, &str),
        goal: &str,
        config: &themis_core::configuration::RuntimeConfig,
        chat_model: &str,
        project_root: &std::path::Path,
    ) {
        if !config.goal.seed_todo || self.todo_for(thread_id).has_planned_items() {
            return;
        }
        let started = std::time::Instant::now();
        let (system, user) = core_todo::seed_request(goal, has_project_files(project_root));
        let answer = self
            .summary_model_request(config, chat_model, (thread_id, run_id), &system, &user)
            .await;
        let mut log = json!({"thread_id": thread_id, "run_id": run_id,
            "input_bytes": system.len() + user.len()});
        let outcome = match answer {
            Err(reason) => reason,
            Ok((text, usage)) => {
                if let Some((prompt, completion)) = usage {
                    log["prompt_tokens"] = prompt.into();
                    log["completion_tokens"] = completion.into();
                }
                match core_todo::parse_seed(&text) {
                    None => "unreadable".to_owned(),
                    Some(items) => {
                        let (list, added) = self.update_todo(sink, thread_id, |list| {
                            // A list planned meanwhile is left alone.
                            if list.has_planned_items() {
                                0
                            } else {
                                list.seed(&items)
                            }
                        });
                        log["items"] = added.into();
                        if added > 0 {
                            self.log_todo(thread_id, "seeded", "goal", &list);
                            "seeded".to_owned()
                        } else {
                            "nothing".to_owned()
                        }
                    }
                }
            }
        };
        log["outcome"] = outcome.clone().into();
        log["elapsed_ms"] = (started.elapsed().as_millis() as u64).into();
        let level = if ["seeded", "nothing"].contains(&outcome.as_str()) {
            "info"
        } else {
            "warn"
        };
        let _ = self.inner.diagnostics.append("todo", "seed", level, log);
    }

    /// Removes the lists of deleted chats.
    pub(super) fn forget_todos(&self, thread_ids: &[String]) {
        let mut todos = self.inner.todos.lock().unwrap_or_else(|e| e.into_inner());
        let mut chains = self
            .inner
            .request_chains
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        for id in thread_ids {
            todos.remove(id);
            chains.remove(id);
            if let Some(path) = self.todo_path(id) {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

/// Whether the project already has files of its own (anything but hidden entries such as
/// `.git` and `.themis`): then a seeded goal starts by reading and planning rather than by
/// setting up. Reads one directory listing, at most 64 entries.
fn has_project_files(root: &std::path::Path) -> bool {
    std::fs::read_dir(root).is_ok_and(|entries| {
        entries
            .flatten()
            .take(64)
            .any(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
    })
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
            TodoOwner::Subagent { agent, .. } => {
                agent.todo.lock().unwrap_or_else(|e| e.into_inner()).clone()
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
            TodoOwner::Subagent { agent, report } => {
                let mut list = agent.todo.lock().unwrap_or_else(|e| e.into_inner());
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
        // Items the check completed or closed that this write gives another status: they keep
        // the check's, and the agent is told why.
        let check_kept: Vec<String> = written
            .iter()
            .filter(|item| item.status != core_todo::TodoStatus::Completed)
            .filter_map(|item| {
                let known = before
                    .items
                    .iter()
                    .find(|known| Some(&known.id) == item.id.as_ref())?;
                let how = match known.closed? {
                    core_todo::CheckClosure::Confirmed => "confirmed",
                    core_todo::CheckClosure::Cancelled => "closed as cancelled",
                    core_todo::CheckClosure::Answered => "closed as answered",
                };
                Some(themis_core::prompts::fill(
                    themis_core::prompts::TOOL_TODO_CHECK_KEPT,
                    &[("id", &known.id), ("how", how)],
                ))
            })
            .collect();
        let (list, summary) = self.apply(written);
        // The ids the agent doesn't know yet: its new items and requests it hadn't seen.
        let new: Vec<Value> = list
            .items
            .iter()
            .filter(|item| !known.contains(&item.id) || !sent.contains(&item.id))
            .map(|item| json!({"id": item.id, "title": item.title, "status": item.status}))
            .collect();
        let mut result = json!({
            "kept": summary.items,
            "open": summary.open,
            "done": summary.completed,
            "new_ids": new,
        });
        if !check_kept.is_empty() {
            result["note"] = check_kept.join(" ").into();
        }
        Ok(result)
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
mod chain_tests {
    use super::*;

    const THREAD: &str = "5d1c7a2e-8f40-4b6a-9e13-2c7b9d0a6f58";

    fn chained(state: &AppState) -> Vec<String> {
        state
            .inner
            .request_chains
            .lock()
            .unwrap()
            .get(THREAD)
            .map(|chain| chain.later.iter().map(|(_, text)| text.clone()).collect())
            .unwrap_or_default()
    }

    #[tokio::test]
    async fn a_chain_keeps_the_latest_requests_restarts_on_an_idle_message_and_joins_a_new_goal_once(
    ) {
        let data = tempfile::tempdir().unwrap();
        let state = AppState::new_for_test(data.path().join("settings.json"));
        let (sink, _rx) = crate::sink::ChannelSink::channel();
        state.chain_request(THREAD, "r1", "Set a goal: build it", false);
        // Answers are not requests; blank messages are nothing.
        let answer = format!("{} yes", super::super::questions::ANSWER_MARKER);
        state.chain_request(THREAD, "r1", &answer, true);
        state.chain_request(THREAD, "r1", "   ", true);
        let wake = format!(
            "{} A plugin hook finished.",
            themis_core::plugins::hooks::HOOK_WAKE_MARKER
        );
        state.chain_request(THREAD, "r1", &wake, true);
        assert!(chained(&state).is_empty());
        for n in 0..12 {
            state.chain_request(THREAD, "r1", &format!("request {n}"), true);
        }
        let kept = chained(&state);
        assert_eq!(kept.len(), MAX_CHAIN_REQUESTS);
        assert_eq!(kept.first().unwrap(), "request 4");
        // A message to an idle chat starts over.
        state.chain_request(THREAD, "r2", "Something else", false);
        assert!(chained(&state).is_empty());
        state.chain_request(THREAD, "r2", "Also this", true);
        // The runs ended with nothing queued: a message queued to the idle chat (sent by the
        // server at once) opens a new chain rather than joining the old one.
        state.close_request_chain(THREAD);
        state.chain_request(THREAD, "r3", "Queued while idle", true);
        assert!(chained(&state).is_empty());
        state.chain_request(THREAD, "r3", "Also this", true);
        state.chain_request(THREAD, "r3", "Also this", true);

        // A new goal takes them on, the same words once, and spends the chain.
        state.begin_goal(&sink, THREAD, "Build it");
        state.requests_join_new_goal(&sink, THREAD);
        let list = state.todo_for(THREAD);
        assert_eq!(
            list.items
                .iter()
                .map(|item| item.details.as_str())
                .collect::<Vec<_>>(),
            ["Also this"]
        );
        assert!(state
            .inner
            .request_chains
            .lock()
            .unwrap()
            .get(THREAD)
            .is_none());
        state.requests_join_new_goal(&sink, THREAD);
        assert_eq!(state.todo_for(THREAD).items.len(), 1);
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
