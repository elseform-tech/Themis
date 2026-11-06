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
use themis_core::plugins::agents::{
    NamedProfile, ProfileCatalog, READ_ONLY_TOOLS, SPAWN_TOOL, WEB_TOOLS, WRITE_TOOLS,
};
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

/// The run's web tools: the settings and site permissions the main agent's use.
pub(super) struct SubagentWeb {
    pub(super) config: themis_core::configuration::WebConfig,
    pub(super) permissions: Arc<themis_core::tools::site_permissions::SitePermissions>,
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

/// What web tools one subagent gets, decided when it is started.
#[derive(Default)]
pub(super) struct WebAccess {
    pub(super) tools: Vec<String>,
    /// What its profile names that it can't have.
    pub(super) left_out: Vec<String>,
}

/// What a subagent hands down to the subagents it starts: they can do no more than it can.
pub(super) struct NestedLimits {
    parent_id: String,
    parent_name: String,
    /// The tools it has: theirs are chosen as usual, then held to these.
    tools: Vec<String>,
    /// Theirs must lie inside these.
    write_paths: Vec<String>,
    /// What its shell is limited to; empty is no limit.
    shell_scopes: Vec<String>,
    /// Its list says which profiles it may start, so it can't start a plain subagent.
    profile_required: bool,
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
    prompt_model: themis_core::plugins::hooks::PromptModel,
    /// The run's MCP tools.
    pub(super) mcp: SubagentMcp,
    web: SubagentWeb,
    /// The chat's skills, as the main agent's system message lists them.
    skills: String,
    /// What the project says about working in it, as the main agent read it.
    project_guidance: String,
    /// The same skills for the `Skill` tool.
    skill_list: Vec<themis_core::skills::Skill>,
    /// The `bin` folders of the run's plugins.
    plugin_bins: Vec<PathBuf>,
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
    /// Slots of subagents other subagents start. Apart from `slots`: a subagent keeps its slot
    /// while it waits for its own, so sharing them could wait forever.
    nested_slots: Arc<tokio::sync::Semaphore>,
    /// Set when a subagent, not the main agent, is starting these.
    pub(super) nested: Option<NestedLimits>,
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
        prompt_model: themis_core::plugins::hooks::PromptModel,
        mcp: SubagentMcp,
        web: SubagentWeb,
        skills: String,
        project_guidance: String,
        plugin_bins: Vec<PathBuf>,
        llm: Arc<dyn LLMProvider>,
        policy: RunPolicy,
        sandbox: &SandboxPlan,
        grants: Arc<ChatGrants>,
        stopped: Arc<AtomicBool>,
    ) -> Self {
        let slots = Arc::new(tokio::sync::Semaphore::new(
            config.subagents.max_parallel.max(1),
        ));
        let nested_slots = Arc::new(tokio::sync::Semaphore::new(
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
            prompt_model,
            mcp,
            web,
            skills,
            project_guidance,
            skill_list: snapshot.skill_catalog.clone(),
            plugin_bins,
            plugins: snapshot.plugins.clone(),
            llm,
            model: snapshot.model.clone(),
            policy,
            sandbox: Arc::new(sandbox.read_only()),
            grants,
            stopped,
            notes: RunNotes::new(),
            slots,
            nested_slots,
            nested: None,
            profiles: ProfileCatalog::from_plugins(&snapshot.plugins),
            started: Mutex::new(Vec::new()),
            other_llms: tokio::sync::Mutex::new(HashMap::new()),
        }
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
        let owner = self.nested.as_ref().map(|nested| nested.parent_id.as_str());
        self.find(id).or_else(|| {
            self.state
                .inner
                .subagents
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&self.thread_id)
                .and_then(|list| list.iter().find(|agent| agent.id == id).cloned())
                // Each agent answers for the ones it started.
                .filter(|agent| agent.parent.as_deref() == owner)
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
    /// The subagent that started it; none when the main agent did.
    pub(super) parent: Option<String>,
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
    /// The web tools it may use.
    pub(super) web: WebAccess,
    /// Whether an OS sandbox really confines its shell: not when off, nor when none can run.
    pub(super) confined: bool,
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

/// Whether a normalized write path lies inside another: the same file, or under a folder.
fn path_inside(own: &str, path: &str) -> bool {
    own == path || (own.ends_with('/') && path.starts_with(own))
}

/// Whether its profile's list names this tool.
fn names(agent: &Subagent, tool: &str) -> bool {
    agent
        .profile
        .as_ref()
        .is_some_and(|named| named.profile.names(tool))
}

/// Claude's names for the tools it has, for instructions written for Claude.
fn claude_names(tools: &[Box<dyn ToolT>]) -> Vec<String> {
    const NAMES: [(&str, &str); 13] = [
        ("read_file", "Read"),
        ("search_file", "Grep"),
        ("list_dir", "LS and Glob"),
        ("shell", "Bash"),
        ("write_file", "Write"),
        ("apply_patch", "Edit"),
        ("fetch_url", "WebFetch"),
        ("web_search", "WebSearch"),
        ("background_task", "BashOutput and KillShell"),
        ("ask_user", "AskUserQuestion"),
        ("create_dir", "mkdir"),
        ("recall", "recall"),
        (SPAWN_TOOL, "Agent and Task"),
    ];
    NAMES
        .iter()
        .filter(|(tool, _)| tools.iter().any(|known| known.name() == *tool))
        .map(|(tool, claude)| format!("{tool} for {claude}"))
        .collect()
}

impl Subagent {
    /// What its shell is limited to; empty is no limit.
    pub(super) fn shell_scopes(&self) -> &[String] {
        self.profile
            .as_ref()
            .map_or(&[], |named| named.profile.shell_scopes.as_slice())
    }

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

mod access;
mod app;
mod attempt;
mod running;
use attempt::*;
#[cfg(test)]
use running::ScopedShell;

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

    /// A subagent held to named commands still asks about a sensitive file, and its
    /// scope still refuses first. (A wrapper without these two methods loses the check.)
    #[test]
    fn a_scoped_shell_keeps_the_sensitive_file_check() {
        use themis_core::configuration::{
            configured_hook_for_project, ApprovalMode, ApprovalPolicy,
        };
        use themis_core::tools::{CommandDisplay, FnHook, RiskLevel};
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".env"), "A=1\n").unwrap();
        let asked = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&asked);
        let scoped = ScopedShell {
            inner: configured_hook_for_project(
                ApprovalMode::Custom,
                ApprovalPolicy::default(),
                Arc::new(FnHook::new(move |_: &ToolAction| {
                    counter.fetch_add(1, Ordering::SeqCst);
                    Approval::Deny
                })),
                ChatGrants::in_memory(),
                Some(dir.path()),
            ),
            scopes: vec!["cat:*".to_owned()],
            context: themis_core::diagnostics::DiagnosticContext {
                log: Arc::new(themis_core::diagnostics::DiagnosticLog::new(
                    dir.path().join("logs"),
                )),
                thread_id: "t".to_owned(),
                run_id: "r".to_owned(),
                level: "info".to_owned(),
            },
        };
        let action = |argv: &[&str]| ToolAction {
            display: None::<CommandDisplay>,
            tool: "shell".to_owned(),
            summary: String::new(),
            risk: RiskLevel::Execute,
            command: argv.iter().map(|word| (*word).to_owned()).collect(),
        };
        let call = |argv: &[&str]| json!({"command": argv[0], "args": &argv[1..]});
        let inside = ["cat", ".env"];
        let access = scoped
            .sensitive_access("shell", &call(&inside))
            .expect("the check reaches through the scope");
        assert_eq!(access.names(), [".env"]);
        assert!(scoped
            .approve_sensitive(&action(&inside), &access)
            .is_denied());
        assert_eq!(asked.load(Ordering::SeqCst), 1, "the person was asked");
        let outside = ["head", ".env"];
        assert!(scoped
            .approve_sensitive(&action(&outside), &access)
            .is_denied());
        assert_eq!(asked.load(Ordering::SeqCst), 1, "the scope refused first");
    }
}
