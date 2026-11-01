//! Subagents: helper runs the agent starts for independent parts of its task. Read-only unless
//! given `write_paths`, the only places they can change.
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
use themis_core::plugins::agents::{NamedProfile, ProfileCatalog, READ_ONLY_TOOLS, WRITE_TOOLS};
use themis_core::runtime::{
    ConversationRole, ConversationTurn, LLMProvider, RunEvent, RunNotes, RunPolicy, SteerInbox,
};
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

/// A model's provider, or why it could not start.
type Resolved = Result<Arc<dyn LLMProvider>, String>;

/// The run's MCP tools, which are the connections the person enabled that came up when it
/// started. Nothing here can add to them.
pub(super) struct SubagentMcp {
    /// Claude's names for each tool, by its Themis name.
    pub(super) names: themis_core::plugins::hooks::McpNames,
    /// "server · tool", by its Themis name.
    pub(super) labels: Arc<HashMap<String, String>>,
    /// The rules that may stop a server starting, as for the run.
    pub(super) startup_rules: themis_core::configuration::ApprovalPolicy,
}

/// What MCP tools one subagent gets, decided when it is started.
#[derive(Default)]
pub(super) struct McpAccess {
    /// Their Themis names.
    pub(super) tools: Vec<String>,
    /// Their servers.
    pub(super) connections: Vec<String>,
    /// What was named or asked for that it doesn't get.
    pub(super) left_out: Vec<String>,
}

/// Names one diagnostics event may list, and how long each may be.
const LOGGED_NAMES: usize = 20;
const LOGGED_NAME_CHARS: usize = 80;

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
    /// The run's MCP tools.
    pub(super) mcp: SubagentMcp,
    plugins: Vec<themis_core::plugins::Plugin>,
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
    /// The profiles of this run's enabled plugins.
    pub(super) profiles: ProfileCatalog,
    /// Subagents this run started, in order.
    pub(super) started: Mutex<Vec<Arc<Subagent>>>,
    /// Providers resolved for models other than the chat's, by model.
    other_llms: tokio::sync::Mutex<HashMap<String, Resolved>>,
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
        mcp: SubagentMcp,
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
            mcp,
            plugins: snapshot.plugins.clone(),
            llm,
            model: snapshot.model.clone(),
            policy,
            sandbox: Arc::new(sandbox.read_only()),
            grants,
            stopped,
            notes: RunNotes::new(),
            slots,
            profiles: ProfileCatalog::from_plugins(&snapshot.plugins),
            started: Mutex::new(Vec::new()),
            other_llms: tokio::sync::Mutex::new(HashMap::new()),
        }
    }

    /// The write paths of this run's running subagents, with their owners.
    fn running_paths(&self) -> Vec<(String, String)> {
        self.started
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter(|agent| !agent.ended())
            .flat_map(|agent| {
                agent
                    .write_paths
                    .iter()
                    .map(|path| (path.clone(), agent.name.clone()))
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    /// The write paths a batch of briefs asks for, normalized and checked together: nothing
    /// outside the project or in protected folders, and no path another running subagent or
    /// another brief already owns.
    pub(super) fn check_write_paths(
        &self,
        asked: &[Vec<String>],
    ) -> Result<Vec<Vec<String>>, String> {
        let protected = self.sandbox.protected().to_vec();
        let mut taken = self.running_paths();
        let mut normalized = Vec::new();
        for (index, paths) in asked.iter().enumerate() {
            let mut own = Vec::new();
            for raw in paths {
                let path =
                    themis_core::sandbox::normalize_write_path(&self.work_root, raw, &protected)?;
                if let Some((other, owner)) = taken
                    .iter()
                    .find(|(other, _)| themis_core::sandbox::write_paths_overlap(other, &path))
                {
                    return Err(format!("'{path}' overlaps '{other}', which {owner} owns"));
                }
                own.push(path);
            }
            taken.extend(
                own.iter()
                    .map(|path| (path.clone(), format!("subagent {}", index + 1))),
            );
            normalized.push(own);
        }
        Ok(normalized)
    }

    /// The model subagents use when none is given: `subagents.model`, else the chat's.
    pub(super) fn default_model(&self) -> String {
        self.config
            .subagents
            .model
            .clone()
            .unwrap_or_else(|| self.model.clone())
    }

    /// The further models the agent may give a subagent (`subagents.models`), without the default.
    pub(super) fn other_models(&self) -> Vec<String> {
        let default = self.default_model();
        let mut models: Vec<String> = Vec::new();
        for model in &self.config.subagents.models {
            if *model != default && !models.contains(model) {
                models.push(model.clone());
            }
        }
        models
    }

    /// The models a subagent tries in turn: its own, the default, the chat's.
    fn candidates(&self, chosen: &str) -> Vec<String> {
        let mut models = vec![chosen.to_owned()];
        for model in [self.default_model(), self.model.clone()] {
            if !models.contains(&model) {
                models.push(model);
            }
        }
        models
    }

    async fn llm_for(&self, model: &str) -> Result<Arc<dyn LLMProvider>, String> {
        if let Some(reason) = self
            .catalog
            .iter()
            .find(|entry| entry.id == model)
            .and_then(|entry| entry.unavailable_reason.clone())
        {
            return Err(format!("{model} is unavailable: {reason}"));
        }
        if model == self.model {
            return Ok(Arc::clone(&self.llm));
        }
        let mut cache = self.other_llms.lock().await;
        if let Some(found) = cache.get(model) {
            return found.clone();
        }
        let resolved = match Box::pin(self.state.provider_config_for(
            &self.config,
            model,
            &self.thread_id,
        ))
        .await
        {
            Ok(config) => themis_core::providers::resolve(&config)
                .await
                .map_err(|error| format!("{model} could not start: {error:#}")),
            Err(error) => Err(format!("{model} could not start: {error}")),
        };
        cache.insert(model.to_owned(), resolved.clone());
        resolved
    }

    /// Whether the agent can give subagents MCP tools.
    pub(super) fn offers_mcp(&self) -> bool {
        self.config.subagents.allow_mcp && !self.mcp.names.is_empty()
    }

    /// The MCP tools a subagent gets: what its profile's list allows, narrowed by what the main
    /// agent asked for; with no profile, only what was asked for. `asked` holds Themis names or
    /// `*`. Whatever was named and can't be had is left out, not fatal.
    pub(super) fn mcp_access(
        &self,
        profile: Option<&NamedProfile>,
        asked: Option<&[String]>,
    ) -> McpAccess {
        let mut access = McpAccess::default();
        let wanted_by_asking = |tool: &str| {
            asked.map_or(profile.is_some(), |asked| {
                asked.iter().any(|name| name == "*" || name == tool)
            })
        };
        for (tool, names) in self.mcp.names.iter() {
            if !wanted_by_asking(tool) {
                continue;
            }
            if profile.is_none_or(|named| named.profile.allows_mcp(names)) {
                access.tools.push(tool.clone());
            } else if asked.is_some_and(|asked| asked.iter().any(|name| name == tool)) {
                access.left_out.push(tool.clone());
            }
        }
        access.tools.sort();
        let all_names: Vec<&String> = self.mcp.names.values().flatten().collect();
        if let Some(named) = profile {
            let missing = named.profile.missing_mcp_tools(&all_names);
            access
                .left_out
                .extend(missing.into_iter().map(str::to_owned));
        }
        access.left_out.extend(
            asked
                .into_iter()
                .flatten()
                .filter(|name| *name != "*" && !self.mcp.names.contains_key(*name))
                .cloned(),
        );
        let off = !self.config.subagents.allow_mcp;
        if off {
            let tools = std::mem::take(&mut access.tools);
            access.left_out.extend(tools);
        }
        for tool in &access.tools {
            let label = self.mcp.labels.get(tool);
            let server = label.and_then(|label| label.split(" · ").next());
            if let Some(server) = server {
                if !access.connections.iter().any(|known| known == server) {
                    access.connections.push(server.to_owned());
                }
            }
        }
        access.connections.sort();
        if !access.left_out.is_empty() {
            self.log_left_out(
                if off { "setting" } else { "not_connected" },
                &access.left_out,
            );
        }
        access
    }

    /// Metadata only: why, how many and which tools, never a call's contents.
    fn log_left_out(&self, reason: &str, names: &[String]) {
        let listed: Vec<String> = names
            .iter()
            .take(LOGGED_NAMES)
            .map(|name| name.chars().take(LOGGED_NAME_CHARS).collect())
            .collect();
        let _ = self.state.inner.diagnostics.append_with_level(
            "subagents",
            "mcp_left_out",
            "warn",
            json!({"thread_id": self.thread_id, "run_id": self.run_id, "reason": reason,
                "count": names.len(), "tools": listed}),
            Some(&self.config.logging.level),
        );
    }

    /// This subagent's MCP tools, asking `approvals` on every call: the tools themselves, what
    /// they are called, and what could not be had after all.
    async fn mcp_tools_for(
        &self,
        agent: &Arc<Subagent>,
        approvals: &Arc<dyn ApprovalHook>,
    ) -> (Vec<Box<dyn ToolT>>, Vec<String>) {
        let mut tools: Vec<Box<dyn ToolT>> = Vec::new();
        if agent.mcp.tools.is_empty() {
            return (tools, Vec::new());
        }
        for plugin in &self.plugins {
            for (name, server) in &plugin.spec.mcp {
                if !server.enabled {
                    continue;
                }
                let connected = themis_core::diagnostics::ACTIVE
                    .scope(
                        self.diagnostic_context(agent),
                        self.state.mcp_tools(
                            &self.work_root,
                            plugin,
                            name,
                            Arc::clone(approvals),
                            Some(&self.mcp.startup_rules),
                        ),
                    )
                    .await;
                if let Ok((found, _, _)) = connected {
                    tools.extend(
                        found.into_iter().filter(|tool| {
                            agent.mcp.tools.iter().any(|wanted| wanted == tool.name())
                        }),
                    );
                }
            }
        }
        let missing: Vec<String> = agent
            .mcp
            .tools
            .iter()
            .filter(|wanted| !tools.iter().any(|tool| tool.name() == wanted.as_str()))
            .cloned()
            .collect();
        if !missing.is_empty() {
            self.log_left_out("not_connected", &missing);
        }
        (tools, missing)
    }

    /// Starts one subagent on `model` (already checked against the allowed ones); it waits
    /// for a free slot before its first step.
    pub(super) fn start(
        self: &Arc<Self>,
        name: String,
        brief: String,
        model: String,
        write_paths: Vec<String>,
        profile: Option<Arc<NamedProfile>>,
        mcp: McpAccess,
    ) -> Arc<Subagent> {
        let agent = Arc::new(Subagent {
            id: uuid::Uuid::new_v4().simple().to_string()[..12].to_owned(),
            name,
            task: Mutex::new(brief.clone()),
            brief,
            model,
            earlier: Mutex::new(Vec::new()),
            write_paths,
            profile,
            mcp,
            run_id: uuid::Uuid::new_v4().to_string(),
            started: Mutex::new(Instant::now()),
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
            write_paths: agent.write_paths.clone(),
            connections: agent.mcp.connections.clone(),
            at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        });
        let _ = self.state.inner.diagnostics.append_with_level(
            "subagents",
            "started",
            "info",
            json!({"thread_id": self.thread_id, "run_id": self.run_id, "subagent_id": agent.id,
                "child_run_id": agent.run_id, "confined": self.sandbox.enabled(),
                "write_paths": agent.write_paths.len(),
                "mcp_tools": agent.mcp.tools.len(),
                "profile": agent.profile.as_ref().map(|named| &named.profile.name)}),
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
                    model: agent.progress().ran_on,
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
            inner: themis_core::configuration::configured_hook_for_project(
                self.mode,
                self.config.approval.clone(),
                own,
                Arc::clone(&self.grants),
                Some(&self.work_root),
            ),
            context,
        })
    }

    fn tools(
        &self,
        agent: &Arc<Subagent>,
        approvals: Arc<dyn ApprovalHook>,
        mcp: Vec<Box<dyn ToolT>>,
    ) -> Result<Vec<Box<dyn ToolT>>, String> {
        // Checked again on every start: a path may have become a link since it was given.
        for path in &agent.write_paths {
            themis_core::sandbox::normalize_write_path(
                &self.work_root,
                path,
                self.sandbox.protected(),
            )?;
        }
        let owned: Vec<PathBuf> = agent
            .write_paths
            .iter()
            .map(|path| self.work_root.join(path))
            .collect();
        // Folders exist from the start, so an OS sandbox can allow them.
        for path in agent.write_paths.iter().filter(|path| path.ends_with('/')) {
            let _ = std::fs::create_dir_all(self.work_root.join(path));
        }
        let tools = themis_core::tools::boxed_tools_for_run(
            &self.work_root,
            approvals,
            themis_core::tools::ToolOptions {
                yolo: self.mode == ApprovalMode::Yolo,
                background: None,
                cancel: Some(Arc::clone(&agent.stop)),
                sandbox: Some(Arc::new(self.sandbox.confined_to(&self.work_root, &owned))),
                no_escalation: true,
            },
        )
        .map_err(|error| format!("failed to build tools: {error:#}"))?;
        Ok(tools
            .into_iter()
            .filter(|tool| {
                READ_ONLY_TOOLS.contains(&tool.name())
                    || (!agent.write_paths.is_empty() && WRITE_TOOLS.contains(&tool.name()))
            })
            // A profile's list can only take away from the above.
            .filter(|tool| {
                agent
                    .profile
                    .as_ref()
                    .is_none_or(|named| named.profile.allows(tool.name()))
            })
            // Chosen when it started; they are no file permission, so none of the above applies.
            .chain(mcp.into_iter().map(|tool| {
                Box::new(super::subagent_tools::Stoppable {
                    inner: tool,
                    stop: Arc::clone(&agent.stop),
                }) as Box<dyn ToolT>
            }))
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
            mcp_names: Arc::clone(&self.mcp.names),
        }
    }

    async fn run_child(&self, agent: &Arc<Subagent>) -> Ending {
        let approvals = self.approvals(agent);
        let hooks = self.hook_runtime(agent, Arc::clone(&approvals));
        // Hook outcomes of SubagentStart/End are not part of its steps.
        let (hook_events, mut hook_outcomes) = tokio::sync::mpsc::channel::<RunEvent>(64);
        tokio::spawn(async move { while hook_outcomes.recv().await.is_some() {} });
        if let Err(error) = hooks
            .emit(
                "SubagentStart",
                json!({"name": agent.name, "brief": agent.task.lock().unwrap_or_else(|e| e.into_inner()).clone(), "model": agent.model}),
                &hook_events,
            )
            .await
        {
            return Ending::failed(format!("A SubagentStart hook refused: {error}"));
        }
        let candidates = self.candidates(&agent.model);
        let mut ending = Ending::failed("No model could start".into());
        let mut reason = String::new();
        for (index, model) in candidates.iter().enumerate() {
            let last = index + 1 == candidates.len();
            let llm = match self.llm_for(model).await {
                Ok(llm) => llm,
                Err(error) => {
                    reason = error;
                    ending = Ending::failed(reason.clone());
                    continue;
                }
            };
            if index > 0 {
                agent.ran_on(model, &reason);
                (self.reporter())(agent);
                let _ = self.state.inner.diagnostics.append_with_level(
                    "subagents",
                    "fallback",
                    "warn",
                    json!({"thread_id": self.thread_id, "subagent_id": agent.id,
                        "from": agent.model, "to": model}),
                    Some(&self.config.logging.level),
                );
            }
            let (outcome, incomplete) = self
                .attempt(agent, model, llm, Arc::clone(&approvals), &hooks)
                .await;
            // Its model failed before it did anything (a quota, a provider error): try the next.
            let untouched = agent.tool_calls.load(Ordering::SeqCst) == 0;
            if let Err(error) = &outcome {
                if untouched && !last && !self.stopping(agent) {
                    reason = format!("{model} failed: {}", first_line_of(&error.to_string()));
                    continue;
                }
            }
            let status = if self.stopping(agent) {
                "stopped"
            } else {
                match (&outcome, incomplete) {
                    (Ok(_), true) => "incomplete",
                    (Ok(_), false) => "finished",
                    (Err(_), _) => "failed",
                }
            };
            ending = Ending {
                status: status.into(),
                result: match outcome {
                    Ok(result) => result,
                    Err(error) => error.to_string(),
                },
            };
            break;
        }
        let _ = hooks
            .emit(
                "SubagentEnd",
                json!({"status": ending.status, "ok": ending.status == "finished"}),
                &hook_events,
            )
            .await;
        ending
    }

    /// One run of the subagent on `model`. Returns its outcome and whether it ended incomplete.
    async fn attempt(
        &self,
        agent: &Arc<Subagent>,
        model: &str,
        llm: Arc<dyn LLMProvider>,
        approvals: Arc<dyn ApprovalHook>,
        hooks: &themis_core::plugins::hooks::HookRuntime,
    ) -> (anyhow::Result<String>, bool) {
        let (mcp_tools, mcp_missing) = self.mcp_tools_for(agent, &approvals).await;
        let mcp_labels: Vec<String> = mcp_tools
            .iter()
            .map(|tool| {
                let label = self.mcp.labels.get(tool.name());
                label.map_or_else(|| tool.name().to_owned(), Clone::clone)
            })
            .collect();
        let tools = match self.tools(agent, Arc::clone(&approvals), mcp_tools) {
            Ok(tools) => tools,
            Err(error) => return (Err(anyhow::anyhow!(error)), false),
        };
        let (events_tx, events_rx) = tokio::sync::mpsc::channel::<RunEvent>(1024);
        let pump = tokio::spawn(pump_events(
            events_rx,
            self.transcript_path(agent),
            Arc::clone(agent),
            self.reporter(),
            Arc::clone(&self.mcp.labels),
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
        let capacity = self
            .catalog
            .iter()
            .find(|entry| entry.id == model)
            .and_then(|entry| entry.context_tokens);
        let access = if agent.write_paths.is_empty() {
            themis_core::prompts::SUBAGENT_READ_ONLY_NOTE.to_owned()
        } else {
            themis_core::prompts::fill(
                themis_core::prompts::SUBAGENT_WRITE_PATHS_NOTE,
                &[("paths", &agent.write_paths.join(", "))],
            )
        };
        // Before the brief, and with its access stated after it, so that has the last word. In
        // the system message it outlasts a checkpoint, a fallback and being picked up again.
        let role = agent.profile.as_ref().map(|named| {
            themis_core::prompts::fill(
                themis_core::prompts::SUBAGENT_PROFILE,
                &[
                    ("name", &named.profile.name),
                    ("plugin", &named.plugin),
                    ("instructions", &named.profile.instructions),
                ],
            )
        });
        let mcp_note = (!mcp_labels.is_empty()).then(|| {
            themis_core::prompts::fill(
                themis_core::prompts::SUBAGENT_MCP_NOTE,
                &[("tools", &mcp_labels.join(", "))],
            )
        });
        let left_out: Vec<&str> = agent
            .mcp
            .left_out
            .iter()
            .chain(&mcp_missing)
            .map(String::as_str)
            .collect();
        let left_out_note = (!left_out.is_empty()).then(|| {
            themis_core::prompts::fill(
                themis_core::prompts::SUBAGENT_MCP_LEFT_OUT,
                &[("tools", &left_out.join(", "))],
            )
        });
        let head = std::iter::once(themis_core::prompts::SUBAGENT_SYSTEM)
            .chain([themis_core::prompts::SUBAGENT_SECRETS_NOTE])
            .chain(role.as_deref())
            .chain([access.as_str()])
            .chain(mcp_note.as_deref())
            .chain(left_out_note.as_deref())
            .chain([themis_core::prompts::SUBAGENT_STATUS_FIELD])
            .collect::<Vec<_>>()
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
            agent.task.lock().unwrap_or_else(|e| e.into_inner()).clone(),
            agent
                .earlier
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone(),
            approvals,
            policy,
            events_tx,
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
        let incomplete = pump.await.unwrap_or(false);
        (outcome, incomplete)
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
        let seconds = agent
            .started
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .elapsed()
            .as_secs();
        let tool_calls = agent.tool_calls.load(Ordering::SeqCst);
        self.publish(ThreadEvent::SubagentEnded {
            id: agent.id.clone(),
            status: ending.status.clone(),
            result: result.clone(),
            tool_calls,
            seconds,
            model: agent.progress().ran_on,
        });
        // Marked ended only now: a message picking it up again then follows this ending in the chat.
        {
            let mut progress = agent.progress.lock().unwrap_or_else(|e| e.into_inner());
            progress.ended = Some((ending.status.clone(), result.clone()));
            progress.note = note.clone();
        }
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

    /// Finds a subagent of this chat, from this run or an earlier one.
    pub(super) fn find_in_chat(&self, id: &str) -> Option<Arc<Subagent>> {
        self.find(id).or_else(|| {
            self.state
                .inner
                .subagents
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&self.thread_id)
                .and_then(|list| list.iter().find(|agent| agent.id == id).cloned())
        })
    }

    /// Shows the main agent's message in the chat and in the subagent's own transcript.
    pub(super) fn show_message(&self, agent: &Subagent, text: &str, resumed: bool) {
        let event = ThreadEvent::SubagentMessage {
            id: agent.id.clone(),
            text: text.to_owned(),
            resumed,
        };
        if let Some(path) = self.transcript_path(agent) {
            let line = serde_json::to_string(&ThreadEventEnvelope {
                thread_id: String::new(),
                run_id: agent.run_id.clone(),
                event: event.clone(),
            });
            if let (Ok(line), Ok(mut file)) = (
                line,
                std::fs::OpenOptions::new()
                    .append(true)
                    .create(true)
                    .open(path),
            ) {
                use std::io::Write;
                let _ = writeln!(file, "{line}");
            }
        }
        self.publish(event);
    }

    /// Picks an ended subagent up again: it keeps its name, model and write paths, and reports
    /// back as a new note. Refused when another running subagent now owns its paths.
    pub(super) fn resume(
        self: &Arc<Self>,
        agent: &Arc<Subagent>,
        message: String,
    ) -> Result<(), String> {
        if !agent.write_paths.is_empty() && !self.config.subagents.allow_writes {
            return Err(format!(
                "{} has write paths, and subagents are set to read-only now; start a new one",
                agent.name
            ));
        }
        for path in &agent.write_paths {
            if let Some((other, owner)) = self
                .running_paths()
                .into_iter()
                .find(|(other, _)| themis_core::sandbox::write_paths_overlap(other, path))
            {
                return Err(format!(
                    "'{path}' overlaps '{other}', which {owner} owns now"
                ));
            }
        }
        agent.reopen(message.clone());
        self.notes.expect(1);
        {
            let mut started = self.started.lock().unwrap_or_else(|e| e.into_inner());
            if !started.iter().any(|known| known.id == agent.id) {
                started.push(Arc::clone(agent));
            }
        }
        self.show_message(agent, &message, true);
        let _ = self.state.inner.diagnostics.append_with_level(
            "subagents",
            "resumed",
            "info",
            json!({"thread_id": self.thread_id, "run_id": self.run_id, "subagent_id": agent.id}),
            Some(&self.config.logging.level),
        );
        let kit = Arc::clone(self);
        let child = Arc::clone(agent);
        tauri::async_runtime::spawn(async move { kit.run(child).await });
        Ok(())
    }

    /// Stops one subagent and denies the approvals it is waiting on.
    pub(super) fn stop(&self, agent: &Subagent) {
        self.state
            .stop_one_subagent(self.sink.as_ref(), &self.thread_id, agent);
    }
}

/// One subagent's live state.
pub(super) struct Subagent {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) brief: String,
    pub(super) model: String,
    /// What it is working on now: its brief, or the latest message that picked it up again.
    task: Mutex<String>,
    /// Its earlier exchanges with the main agent, when it was picked up again.
    earlier: Mutex<Vec<ConversationTurn>>,
    /// The project paths it may change, normalized; none when it is read-only.
    pub(super) write_paths: Vec<String>,
    /// The plugin profile it runs as.
    profile: Option<Arc<NamedProfile>>,
    /// The MCP tools it may use.
    pub(super) mcp: McpAccess,
    pub(super) run_id: String,
    started: Mutex<Instant>,
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
    /// The model it fell back to when its own could not run, and why.
    pub(super) ran_on: Option<String>,
    pub(super) fallback_reason: String,
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

    /// It runs on `model` instead of its own, for `reason`.
    fn ran_on(&self, model: &str, reason: &str) {
        let mut progress = self.progress.lock().unwrap_or_else(|e| e.into_inner());
        progress.ran_on = Some(model.to_owned());
        progress.fallback_reason = reason.to_owned();
    }

    /// Its latest action, shown until it gives a status line of its own.
    pub(super) fn set_action(&self, action: &str) {
        let mut progress = self.progress.lock().unwrap_or_else(|e| e.into_inner());
        if !progress.own_status {
            progress.status = action.chars().take(80).collect();
        }
    }

    /// Picks an ended subagent up again on `message`, with its last exchange as history.
    fn reopen(&self, message: String) {
        let mut progress = self.progress.lock().unwrap_or_else(|e| e.into_inner());
        let (_, result) = progress.ended.take().unwrap_or_default();
        progress.note.clear();
        progress.status.clear();
        progress.own_status = false;
        drop(progress);
        let mut task = self.task.lock().unwrap_or_else(|e| e.into_inner());
        let mut earlier = self.earlier.lock().unwrap_or_else(|e| e.into_inner());
        earlier.push(ConversationTurn {
            role: ConversationRole::User,
            text: std::mem::replace(&mut *task, message),
        });
        earlier.push(ConversationTurn {
            role: ConversationRole::Assistant,
            text: result,
        });
        self.stop.store(false, Ordering::SeqCst);
        *self.started.lock().unwrap_or_else(|e| e.into_inner()) = Instant::now();
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

/// The first line of an error, for a fallback reason.
fn first_line_of(text: &str) -> String {
    text.lines()
        .next()
        .unwrap_or_default()
        .chars()
        .take(200)
        .collect()
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
    labels: Arc<HashMap<String, String>>,
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
        let mut saved = ThreadEvent::from(event);
        if let ThreadEvent::ToolStarted { tool, label, .. } = &mut saved {
            *label = labels.get(tool.as_str()).cloned();
        }
        save(saved, &mut file);
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

    /// Asks past a grant this subagent has for the tool or command alone, and remembers each
    /// file with it, in the same place: "allow in this chat" stays its own.
    fn approve_sensitive(
        &self,
        action: &ToolAction,
        access: &themis_core::tools::sensitive::SensitiveAccess,
    ) -> Approval {
        let files = access.file_actions();
        let command = themis_core::tools::is_command_tool(&action.tool);
        if files.iter().all(|file| self.grants.covers(file))
            && (!command || self.grants.covers(action))
        {
            return Approval::AllowOnce;
        }
        match self.inner.approve_sensitive(action, access) {
            Approval::AllowAlways => {
                if action.grantable() {
                    for file in &files {
                        self.grants.remember(file);
                    }
                    if command {
                        self.grants.remember(action);
                    }
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
                            "model": agent.model, "write_paths": agent.write_paths, "connections": agent.mcp.connections, "profile": agent.profile.as_ref().map(|named| &named.profile.name), "ran_on": progress.ran_on, "status": status, "text": progress.status,
                            "result": result, "tool_calls": agent.tool_calls.load(Ordering::SeqCst),
                            "seconds": agent.started.lock().unwrap_or_else(|e| e.into_inner()).elapsed().as_secs()})
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn stop_one_subagent(&self, sink: &dyn EventSink, thread_id: &str, agent: &Subagent) {
        agent.stop.store(true, Ordering::SeqCst);
        let mut pending = self.inner.pending.lock().unwrap_or_else(|e| e.into_inner());
        pending.retain(|approval_id, approval| {
            let mine = approval.thread_id == thread_id
                && approval
                    .request
                    .subagent
                    .as_ref()
                    .is_some_and(|label| label.id == agent.id);
            if mine {
                let _ = approval.sender.send(Approval::Deny);
                sink.emit_approval_expired(&ApprovalExpired {
                    thread_id: thread_id.to_owned(),
                    approval_id: approval_id.clone(),
                    steered: false,
                });
            }
            !mine
        });
    }

    /// Stops one subagent of the chat (Workspace › Tasks); the main agent gets what it
    /// had so far. Completed work is never undone.
    pub fn stop_subagent(
        &self,
        sink: &dyn EventSink,
        thread_id: &str,
        subagent_id: &str,
    ) -> Result<(), String> {
        let agent = self
            .inner
            .subagents
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(thread_id)
            .and_then(|list| list.iter().find(|agent| agent.id == subagent_id).cloned())
            .ok_or_else(|| "This subagent is not running".to_owned())?;
        if !agent.ended() {
            self.stop_one_subagent(sink, thread_id, &agent);
        }
        Ok(())
    }

    /// A subagent's saved steps, oldest first: at most the last 512 KiB, whole lines only.
    pub fn get_subagent_transcript(
        &self,
        thread_id: &str,
        subagent_id: &str,
    ) -> Result<Vec<ThreadEvent>, String> {
        use std::io::{Read, Seek, SeekFrom};
        const LIMIT: u64 = 512 * 1024;
        let path = self
            .subagent_transcript_path(thread_id, subagent_id)
            .ok_or_else(|| "Unknown subagent".to_owned())?;
        let Ok(mut file) = std::fs::File::open(&path) else {
            return Ok(Vec::new());
        };
        let length = file.metadata().map(|m| m.len()).unwrap_or(0);
        let start = length.saturating_sub(LIMIT);
        file.seek(SeekFrom::Start(start))
            .map_err(|error| error.to_string())?;
        let mut bytes = Vec::new();
        file.take(LIMIT)
            .read_to_end(&mut bytes)
            .map_err(|error| error.to_string())?;
        let text = String::from_utf8_lossy(&bytes);
        let mut lines = text.lines();
        if start > 0 {
            // The first line may be cut.
            lines.next();
        }
        Ok(lines
            .filter_map(|line| serde_json::from_str::<ThreadEventEnvelope>(line).ok())
            .map(|envelope| envelope.event)
            .collect())
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
