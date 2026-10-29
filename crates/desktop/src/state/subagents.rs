//! Subagents: read-only helper runs the agent starts for independent parts of its task.
//!
//! A chat's run captures a [`SubagentKit`] once, like the rest of its configuration. Each
//! subagent then runs on its own task with its own event channel (never the parent's: a
//! terminal event there would end the chat turn), steer inbox, stop flag and approvals,
//! and its result reaches the main agent as a run note.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde_json::{json, Value};
use themis_core::chat_grants::ChatGrants;
use themis_core::configuration::{ApprovalMode, RuntimeConfig};
use themis_core::runtime::{LLMProvider, RunEvent, RunNotes, RunPolicy, SteerInbox};
use themis_core::sandbox::SandboxPlan;
use themis_core::tools::{Approval, ApprovalHook, ToolAction, ToolT};

use super::run::AuditedApprovals;
use super::AppState;
use crate::approvals::DesktopApprovalHook;
use crate::sink::EventSink;
use crate::types::{ApprovalExpired, ThreadEvent, ThreadEventEnvelope};

/// Subagents one `spawn_subagents` call may start.
pub(super) const MAX_PER_CALL: usize = 6;
/// The result a subagent hands back, in bytes.
const RESULT_LIMIT: usize = 8 * 1024;
/// A subagent's saved transcript stops growing at this size.
const TRANSCRIPT_LIMIT: u64 = 4 * 1024 * 1024;
/// Ended subagents kept per chat for status views.
const KEPT_PER_CHAT: usize = 50;
/// App-data folder of subagent transcripts (`subagents/<chat id>/<id>.jsonl`).
pub(super) const SUBAGENTS_DIR: &str = "subagents";
/// The tools a read-only subagent gets.
const READ_ONLY_TOOLS: [&str; 4] = ["read_file", "list_dir", "search_file", "shell"];

/// What a chat's run hands its subagents, captured when the run starts.
pub(super) struct SubagentKit {
    pub(super) state: AppState,
    pub(super) sink: Arc<dyn EventSink>,
    pub(super) thread_id: String,
    pub(super) run_id: String,
    pub(super) work_root: PathBuf,
    pub(super) is_git: bool,
    pub(super) mode: ApprovalMode,
    pub(super) approval_timeout: std::time::Duration,
    pub(super) unattended: bool,
    pub(super) config: RuntimeConfig,
    pub(super) catalog: Vec<themis_core::providers::GoModel>,
    pub(super) endpoint_override: bool,
    pub(super) hooks: Vec<themis_core::plugins::hooks::Hook>,
    /// The chat's model, used when `subagents.model` is unset or the same.
    pub(super) llm: Arc<dyn LLMProvider>,
    pub(super) model: String,
    pub(super) policy: RunPolicy,
    /// The run's sandbox made read-only.
    pub(super) sandbox: Arc<SandboxPlan>,
    pub(super) grants: Arc<ChatGrants>,
    /// The parent run's stop flag: stopping the chat stops its subagents.
    pub(super) stopped: Arc<AtomicBool>,
    pub(super) notes: Arc<RunNotes>,
    pub(super) slots: Arc<tokio::sync::Semaphore>,
    /// Subagents this run started, in order.
    pub(super) started: Mutex<Vec<Arc<Subagent>>>,
    other_llm: tokio::sync::OnceCell<Result<Arc<dyn LLMProvider>, String>>,
}

impl SubagentKit {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        state: AppState,
        sink: Arc<dyn EventSink>,
        ids: (&str, &str),
        snapshot: &super::RunSnapshot,
        config: RuntimeConfig,
        catalog: Vec<themis_core::providers::GoModel>,
        endpoint_override: bool,
        hooks: Vec<themis_core::plugins::hooks::Hook>,
        llm: Arc<dyn LLMProvider>,
        policy: RunPolicy,
        sandbox: &SandboxPlan,
        grants: Arc<ChatGrants>,
        stopped: Arc<AtomicBool>,
    ) -> Self {
        let slots = Arc::new(tokio::sync::Semaphore::new(
            config.subagents.max_parallel.max(1),
        ));
        Self {
            state,
            sink,
            thread_id: ids.0.to_owned(),
            run_id: ids.1.to_owned(),
            work_root: snapshot.work_root.clone(),
            is_git: snapshot.is_git,
            mode: snapshot.approval_mode,
            approval_timeout: std::time::Duration::from_secs(u64::from(
                snapshot.approval_timeout_seconds,
            )),
            unattended: snapshot.automation_run,
            config,
            catalog,
            endpoint_override,
            hooks,
            llm,
            model: snapshot.model.clone(),
            policy,
            sandbox: Arc::new(sandbox.read_only()),
            grants,
            stopped,
            notes: RunNotes::new(),
            slots,
            started: Mutex::new(Vec::new()),
            other_llm: tokio::sync::OnceCell::new(),
        }
    }

    /// The model subagents use.
    fn child_model(&self) -> String {
        self.config
            .subagents
            .model
            .clone()
            .unwrap_or_else(|| self.model.clone())
    }

    async fn child_llm(&self) -> Result<Arc<dyn LLMProvider>, String> {
        let model = self.child_model();
        if model == self.model {
            return Ok(Arc::clone(&self.llm));
        }
        self.other_llm
            .get_or_init(|| async {
                let config = Box::pin(self.state.provider_config_for(
                    &self.config,
                    &model,
                    &self.thread_id,
                ))
                .await?;
                themis_core::providers::resolve(&config)
                    .await
                    .map_err(|error| format!("failed to resolve the subagent model: {error:#}"))
            })
            .await
            .clone()
    }

    /// Starts one subagent; it waits for a free slot before its first step.
    pub(super) fn start(self: &Arc<Self>, name: String, brief: String) -> Arc<Subagent> {
        let agent = Arc::new(Subagent {
            id: uuid::Uuid::new_v4().simple().to_string()[..12].to_owned(),
            name,
            brief,
            model: self.child_model(),
            run_id: uuid::Uuid::new_v4().to_string(),
            started: Instant::now(),
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
            confined: self.sandbox.enabled(),
        });
        let _ = self.state.inner.diagnostics.append_with_level(
            "subagents",
            "started",
            "info",
            json!({"thread_id": self.thread_id, "run_id": self.run_id, "subagent_id": agent.id,
                "child_run_id": agent.run_id, "confined": self.sandbox.enabled()}),
            Some(&self.config.logging.level),
        );
        let kit = Arc::clone(self);
        let child = Arc::clone(&agent);
        tauri::async_runtime::spawn(async move { kit.run(child).await });
        agent
    }

    /// Saves and sends an event of the parent chat.
    fn publish(&self, event: ThreadEvent) {
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
    fn reporter(&self) -> Reporter {
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
                },
            });
        })
    }

    fn stopping(&self, agent: &Subagent) -> bool {
        agent.stop.load(Ordering::SeqCst) || self.stopped.load(Ordering::SeqCst)
    }

    async fn run(self: Arc<Self>, agent: Arc<Subagent>) {
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

    async fn wait_for_slot(&self, agent: &Subagent) -> Option<tokio::sync::OwnedSemaphorePermit> {
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

    fn diagnostic_context(&self, agent: &Subagent) -> themis_core::diagnostics::DiagnosticContext {
        themis_core::diagnostics::DiagnosticContext {
            log: self.state.inner.diagnostics.clone(),
            thread_id: self.thread_id.clone(),
            run_id: agent.run_id.clone(),
            level: self.config.logging.level.clone(),
        }
    }

    /// Rules first, then the chat's grants, then what was allowed for this subagent alone:
    /// "Allow in this chat" on a subagent's request covers only that subagent.
    fn approvals(&self, agent: &Subagent) -> Arc<dyn ApprovalHook> {
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
        Arc::new(AuditedApprovals {
            inner: themis_core::configuration::configured_hook_with_grants(
                self.mode,
                self.config.approval.clone(),
                own,
                Arc::clone(&self.grants),
            ),
            context,
        })
    }

    fn tools(
        &self,
        agent: &Arc<Subagent>,
        approvals: Arc<dyn ApprovalHook>,
    ) -> Result<Vec<Box<dyn ToolT>>, String> {
        let tools = themis_core::tools::boxed_tools_for_run(
            &self.work_root,
            approvals,
            themis_core::tools::ToolOptions {
                yolo: self.mode == ApprovalMode::Yolo,
                background: None,
                cancel: Some(Arc::clone(&agent.stop)),
                sandbox: Some(Arc::clone(&self.sandbox)),
                no_escalation: true,
            },
        )
        .map_err(|error| format!("failed to build tools: {error:#}"))?;
        Ok(tools
            .into_iter()
            .filter(|tool| READ_ONLY_TOOLS.contains(&tool.name()))
            .map(|tool| {
                Box::new(super::subagent_tools::StatusField {
                    inner: tool,
                    agent: Arc::clone(agent),
                    report: self.reporter(),
                }) as Box<dyn ToolT>
            })
            .collect())
    }

    fn hook_runtime(
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
        }
    }

    async fn run_child(&self, agent: &Arc<Subagent>) -> Ending {
        let llm = match self.child_llm().await {
            Ok(llm) => llm,
            Err(error) => return Ending::failed(error),
        };
        let approvals = self.approvals(agent);
        let tools = match self.tools(agent, Arc::clone(&approvals)) {
            Ok(tools) => tools,
            Err(error) => return Ending::failed(error),
        };
        let hooks = self.hook_runtime(agent, Arc::clone(&approvals));
        let (events_tx, events_rx) = tokio::sync::mpsc::channel::<RunEvent>(1024);
        if let Err(error) = hooks
            .emit(
                "SubagentStart",
                json!({"name": agent.name, "brief": agent.brief}),
                &events_tx,
            )
            .await
        {
            return Ending::failed(format!("A SubagentStart hook refused: {error}"));
        }
        let pump = tokio::spawn(pump_events(
            events_rx,
            self.transcript_path(agent),
            Arc::clone(agent),
            self.reporter(),
        ));
        // Stopping the chat stops its subagents.
        let done = Arc::new(AtomicBool::new(false));
        tokio::spawn({
            let parent = Arc::clone(&self.stopped);
            let child = Arc::clone(&agent.stop);
            let done = Arc::clone(&done);
            async move {
                while !done.load(Ordering::SeqCst) {
                    if parent.load(Ordering::SeqCst) {
                        child.store(true, Ordering::SeqCst);
                        return;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
            }
        });
        let model = self.child_model();
        let capacity = self
            .catalog
            .iter()
            .find(|entry| entry.id == model)
            .and_then(|entry| entry.context_tokens);
        let head = [
            themis_core::prompts::SUBAGENT_SYSTEM,
            themis_core::prompts::SUBAGENT_READ_ONLY_NOTE,
            themis_core::prompts::SUBAGENT_STATUS_FIELD,
        ]
        .join("\n\n");
        let workspace = themis_core::prompts::fill(
            themis_core::prompts::DESKTOP_RUN_WORKSPACE,
            &[(
                "workspace",
                &json!({"project_root": self.work_root}).to_string(),
            )],
        );
        let policy = RunPolicy {
            total_turns: self.config.subagents.max_turns,
            ..self.policy
        };
        let run = themis_core::runtime::run_task_with_evidence(
            llm,
            tools,
            agent.brief.clone(),
            Vec::new(),
            approvals,
            policy,
            events_tx.clone(),
            Arc::clone(&agent.stop),
            format!("\n{workspace}\n"),
            None,
        );
        let outcome = themis_core::providers::with_catalog_limits(
            &self.catalog,
            themis_core::context_metrics::with_usage_recorder(
                self.state.usage_recorder(&self.thread_id, &agent.run_id),
                themis_core::diagnostics::ACTIVE.scope(
                    self.diagnostic_context(agent),
                    themis_core::runtime::with_model_context_capacity(
                        (!self.endpoint_override).then_some(capacity),
                        themis_core::context_metrics::with_run_policy(
                            &self.config,
                            themis_core::runtime::with_configuration(
                                self.config.compaction.clone(),
                                themis_core::plugins::hooks::ACTIVE.scope(
                                    hooks.clone(),
                                    themis_core::runtime::with_steer_inbox(
                                        Arc::clone(&agent.inbox),
                                        themis_core::runtime::with_system_head(head, Box::pin(run)),
                                    ),
                                ),
                            ),
                        ),
                    ),
                ),
            ),
        )
        .await;
        done.store(true, Ordering::SeqCst);
        let status = if self.stopping(agent) {
            "stopped"
        } else {
            match &outcome {
                Ok(_) => "finished",
                Err(_) => "failed",
            }
        };
        let ending = match outcome {
            Ok(result) => Ending {
                status: status.into(),
                result,
            },
            Err(error) => Ending {
                status: status.into(),
                result: error.to_string(),
            },
        };
        let _ = hooks
            .emit(
                "SubagentEnd",
                json!({"status": ending.status, "ok": ending.status == "finished"}),
                &events_tx,
            )
            .await;
        drop(events_tx);
        let incomplete = pump.await.unwrap_or(false);
        if incomplete && ending.status == "finished" {
            return Ending {
                status: "incomplete".into(),
                result: ending.result,
            };
        }
        ending
    }

    async fn end(&self, agent: &Subagent, ending: Ending) {
        let result = bounded(&ending.result);
        let note = themis_core::prompts::fill(
            themis_core::prompts::SUBAGENT_RESULT_NOTE,
            &[
                ("name", &agent.name),
                ("id", &agent.id),
                ("status", &ending.status),
                ("result", &result),
            ],
        );
        {
            let mut progress = agent.progress.lock().unwrap_or_else(|e| e.into_inner());
            progress.ended = Some((ending.status.clone(), result.clone()));
            progress.note = note.clone();
        }
        let seconds = agent.started.elapsed().as_secs();
        let tool_calls = agent.tool_calls.load(Ordering::SeqCst);
        self.publish(ThreadEvent::SubagentEnded {
            id: agent.id.clone(),
            status: ending.status.clone(),
            result,
            tool_calls,
            seconds,
        });
        let _ = self.state.inner.diagnostics.append_with_level(
            "subagents",
            "ended",
            "info",
            json!({"thread_id": self.thread_id, "run_id": self.run_id, "subagent_id": agent.id,
                "status": ending.status, "seconds": seconds, "tool_calls": tool_calls}),
            Some(&self.config.logging.level),
        );
        self.notes.deliver(note);
    }

    fn transcript_path(&self, agent: &Subagent) -> Option<PathBuf> {
        self.state
            .subagent_transcript_path(&self.thread_id, &agent.id)
    }

    /// Finds one of this run's subagents.
    pub(super) fn find(&self, id: &str) -> Option<Arc<Subagent>> {
        self.started
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .find(|agent| agent.id == id)
            .cloned()
    }

    /// Stops one subagent and denies the approvals it is waiting on.
    pub(super) fn stop(&self, agent: &Subagent) {
        agent.stop.store(true, Ordering::SeqCst);
        let mut pending = self
            .state
            .inner
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        pending.retain(|approval_id, approval| {
            let mine = approval.thread_id == self.thread_id
                && approval
                    .request
                    .subagent
                    .as_ref()
                    .is_some_and(|label| label.id == agent.id);
            if mine {
                let _ = approval.sender.send(Approval::Deny);
                self.sink.emit_approval_expired(&ApprovalExpired {
                    thread_id: self.thread_id.clone(),
                    approval_id: approval_id.clone(),
                });
            }
            !mine
        });
    }
}

/// One subagent's live state.
pub(super) struct Subagent {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) brief: String,
    pub(super) model: String,
    pub(super) run_id: String,
    started: Instant,
    pub(super) stop: Arc<AtomicBool>,
    pub(super) inbox: SteerInbox,
    pub(super) tool_calls: AtomicUsize,
    progress: Mutex<Progress>,
}

#[derive(Clone, Default)]
pub(super) struct Progress {
    /// What it says it is doing, or else its latest action.
    pub(super) status: String,
    /// Whether `status` came from the subagent itself.
    pub(super) own_status: bool,
    /// Status and result once it ended.
    pub(super) ended: Option<(String, String)>,
    /// The note its result was delivered as.
    pub(super) note: String,
}

impl Subagent {
    pub(super) fn progress(&self) -> Progress {
        self.progress
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// The status line the subagent gave.
    pub(super) fn set_status(&self, status: &str) {
        let status: String = status.trim().chars().take(80).collect();
        if !status.is_empty() {
            let mut progress = self.progress.lock().unwrap_or_else(|e| e.into_inner());
            progress.status = status;
            progress.own_status = true;
        }
    }

    /// Its latest action, shown until it gives a status line of its own.
    pub(super) fn set_action(&self, action: &str) {
        let mut progress = self.progress.lock().unwrap_or_else(|e| e.into_inner());
        if !progress.own_status {
            progress.status = action.chars().take(80).collect();
        }
    }

    pub(super) fn ended(&self) -> bool {
        self.progress
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .ended
            .is_some()
    }
}

struct Ending {
    status: String,
    result: String,
}

impl Ending {
    fn stopped(result: String) -> Self {
        Self {
            status: "stopped".into(),
            result,
        }
    }
    fn failed(result: String) -> Self {
        Self {
            status: "failed".into(),
            result,
        }
    }
}

/// At most [`RESULT_LIMIT`] bytes, cut on a character boundary.
fn bounded(result: &str) -> String {
    if result.len() <= RESULT_LIMIT {
        return result.to_owned();
    }
    let mut end = RESULT_LIMIT;
    while !result.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n[cut at {} KiB]", &result[..end], RESULT_LIMIT / 1024)
}

/// Saves the child's events to its own transcript, counts tool calls and keeps its status
/// line. Returns whether the run ended incomplete.
async fn pump_events(
    mut events: tokio::sync::mpsc::Receiver<RunEvent>,
    path: Option<PathBuf>,
    agent: Arc<Subagent>,
    report: Reporter,
) -> bool {
    let mut file = path.and_then(|path| {
        std::fs::create_dir_all(path.parent()?).ok()?;
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .ok()
    });
    let mut text = String::new();
    let mut incomplete = false;
    let save = |event: ThreadEvent, file: &mut Option<std::fs::File>| {
        use std::io::Write;
        let Some(handle) = file.as_mut() else { return };
        if handle.metadata().map(|m| m.len()).unwrap_or(0) >= TRANSCRIPT_LIMIT {
            return;
        }
        if let Ok(line) = serde_json::to_string(&ThreadEventEnvelope {
            thread_id: String::new(),
            run_id: agent.run_id.clone(),
            event,
        }) {
            let _ = writeln!(handle, "{line}");
        }
    };
    while let Some(event) = events.recv().await {
        match &event {
            RunEvent::AssistantText(delta) => {
                text.push_str(delta);
                continue;
            }
            RunEvent::ToolCallStarted { .. } => {
                agent.tool_calls.fetch_add(1, Ordering::SeqCst);
                report(&agent);
            }
            RunEvent::Incomplete { .. } => incomplete = true,
            RunEvent::Started { .. } | RunEvent::ContextUsage { .. } => continue,
            _ => {}
        }
        if !text.is_empty() {
            save(
                ThreadEvent::AssistantText {
                    text: std::mem::take(&mut text),
                },
                &mut file,
            );
        }
        save(ThreadEvent::from(event), &mut file);
    }
    if !text.is_empty() {
        save(ThreadEvent::AssistantText { text }, &mut file);
    }
    incomplete
}

/// Keeps a subagent's own "allow in this chat" answers to that subagent.
struct OwnGrants {
    inner: Arc<dyn ApprovalHook>,
    grants: Arc<ChatGrants>,
}

impl ApprovalHook for OwnGrants {
    fn denied_by_rule(&self, action: &ToolAction) -> bool {
        self.inner.denied_by_rule(action)
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
}

impl AppState {
    fn remember_subagent(&self, thread_id: &str, agent: &Arc<Subagent>) {
        let mut all = self
            .inner
            .subagents
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let list = all.entry(thread_id.to_owned()).or_default();
        list.push(Arc::clone(agent));
        while list.len() > KEPT_PER_CHAT {
            match list.iter().position(|agent| agent.ended()) {
                Some(index) => {
                    list.remove(index);
                }
                None => break,
            }
        }
    }

    /// Where a subagent's events are saved.
    pub(super) fn subagent_transcript_path(&self, thread_id: &str, id: &str) -> Option<PathBuf> {
        (super::chat_grants::safe_chat_id(thread_id) && id.bytes().all(|b| b.is_ascii_hexdigit()))
            .then(|| {
                self.app_data_dir()
                    .join(SUBAGENTS_DIR)
                    .join(thread_id)
                    .join(format!("{id}.jsonl"))
            })
    }

    /// The chat's subagents, oldest first, with what each is doing or how it ended.
    pub fn list_subagents(&self, thread_id: &str) -> Vec<Value> {
        self.inner
            .subagents
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(thread_id)
            .map(|list| {
                list.iter()
                    .map(|agent| {
                        let progress = agent.progress();
                        let (status, result) = progress
                            .ended
                            .unwrap_or_else(|| ("running".into(), String::new()));
                        json!({"id": agent.id, "name": agent.name, "brief": agent.brief,
                            "model": agent.model, "status": status, "text": progress.status,
                            "result": result, "tool_calls": agent.tool_calls.load(Ordering::SeqCst),
                            "seconds": agent.started.elapsed().as_secs()})
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Stops every running subagent of the chat (its run ended).
    pub(super) fn stop_subagents(&self, thread_id: &str) {
        if let Some(list) = self
            .inner
            .subagents
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(thread_id)
        {
            for agent in list {
                agent.stop.store(true, Ordering::SeqCst);
            }
        }
    }

    /// Forgets deleted chats' subagents and their transcripts.
    pub(super) fn forget_subagents(&self, thread_ids: &[String]) {
        let mut all = self
            .inner
            .subagents
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        for id in thread_ids {
            if let Some(list) = all.remove(id) {
                for agent in list {
                    agent.stop.store(true, Ordering::SeqCst);
                }
            }
            if super::chat_grants::safe_chat_id(id) {
                let _ = std::fs::remove_dir_all(self.app_data_dir().join(SUBAGENTS_DIR).join(id));
            }
        }
    }
}

/// Sends a subagent's status line.
pub(super) type Reporter = Arc<dyn Fn(&Subagent) + Send + Sync>;

/// Live subagents by chat.
pub(super) type SubagentMap = Mutex<HashMap<String, Vec<Arc<Subagent>>>>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn results_are_cut_on_a_character_boundary() {
        let long = "é".repeat(RESULT_LIMIT);
        let cut = bounded(&long);
        assert!(cut.len() < long.len());
        assert!(cut.ends_with("[cut at 8 KiB]"));
        assert_eq!(bounded("short"), "short");
    }
}
