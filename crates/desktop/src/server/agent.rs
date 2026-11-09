//! Calls from an agent's shell: run credentials and the guard on what they may call.
//!
//! The person's credential lives in `server.json`, which sandboxed commands cannot read. Each
//! run's commands get their own credential instead (`THEMIS_AGENT_TOKEN`, issued on the run's
//! first command and revoked when the chat's run ends, bound to the run's project and network
//! block). A call carrying it is the agent's: it may read status, search its own chat's saved
//! originals and memory when its run has those tools, and manage integrations through
//! the store, and nothing else. The allowed lists, of methods and of `plugin_action` actions, are
//! explicit, so a method or action added later stays refused until someone decides otherwise. A
//! script holding the credential reaches the same checks as `themis` does: they look at the
//! request, never at command text. See
//! `docs/configuration.md` ("The `themis` CLI in the agent's shell").

use std::collections::HashMap;
use std::sync::Mutex;

use super::*;

/// Methods an agent's call may use; `threadId`, when given, must be its own chat.
const ALLOWED: &[&str] = &[
    // Status and diagnostics (metadata only).
    "ping",
    "get_diagnostics",
    "query_diagnostic_logs",
    "get_effective_permissions",
    "get_usage_summary",
    "list_go_models",
    "list_model_providers",
    "get_secret_status",
    "mcp_config_status",
    "resource_usage",
    // Reading configuration and settings.
    "get_runtime_configuration",
    "get_settings",
    "get_browser_settings",
    "get_web_settings",
    "get_subagent_settings",
    "get_memory_settings",
    // Skills and integrations (changes go through the store as the agent's).
    "list_prompt_skills",
    "list_skills",
    "plugin_action",
];

/// Methods only for the agent's own chat: they need its `threadId`.
const OWN_CHAT: &[&str] = &[
    "get_thread",
    "get_thread_history_page",
    "get_thread_tasks",
    "get_background_task_log",
    "get_context_usage",
    "get_chat_usage",
];

/// Read-only methods mirroring a built-in tool of the run (`recall`, `search_memory`): only for
/// the agent's own chat, whose `threadId` the server fills in when the call names none, and only
/// when its run has that tool (memory on, no rule denying it; a subagent's run has neither).
const CHAT_READS: [&str; 2] = themis_core::agent_cli::CHAT_READ_TOOLS;

/// `plugin_action` actions an agent's call may take that fetch nothing and start no code: reads of
/// the store (and of what the app already holds), and changes through the store as the agent's,
/// which keep every new or changed hook and command-based MCP server switched off. Everything
/// else is refused, including an action added later, a missing one and an unknown one.
const LOCAL_ACTIONS: &[&str] = &[
    // Reads.
    "list",
    "inventory",
    "marketplaces",
    "sources",
    "updates",
    "plugin_icon",
    "inspect_path",
    "inspect_json",
    "mcp_tools",
    // Only without `connect` or `retry`, which start connections (see `refusal`).
    "mcp_status",
    // Changes through the store.
    "save",
    "save_skill",
    "save_hook",
    "set_component_enabled",
    "remove_component",
    "enable",
    "disable",
    "delete",
    "import_path",
    "import_json",
    "set_marketplace_enabled",
    "remove_marketplace",
];

/// `plugin_action` actions that may fetch from the network (a repository, a marketplace source or
/// its catalog, a scan they start). The server runs them, outside the run's sandbox, so a run
/// whose sandbox blocks the network may not ask for them.
const NETWORK_ACTIONS: &[&str] = &[
    "install",
    "update",
    "import_repository",
    "preview_repository",
    "inspect_repository",
    "add_marketplace",
    "preview_source",
    "check_source",
    "check_sources",
    "scan_marketplace",
    "catalog",
    "catalog_check",
    "preview",
    "inspect_marketplace",
];

/// `plugin_action` tests that run a definition's command outside the sandbox. The server cannot
/// know the person saw that command, so an agent's credential never runs one.
const RUNS_CODE_ACTIONS: &[&str] = &["test_hook", "test_mcp"];

/// The run and chat a credential belongs to, and what it is bound to: the run's project
/// (canonical) and whether its sandbox blocks the network.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Grant {
    pub thread_id: String,
    pub run_id: String,
    pub project_root: Option<PathBuf>,
    pub network_blocked: bool,
    /// The [`CHAT_READS`] tools its run has.
    pub reads: Vec<String>,
}

/// Why an agent's call was refused, as the log keeps it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Reason {
    /// A method off the list, or another chat's `threadId`.
    MethodNotAllowed,
    /// A `plugin_action` action off the list (missing and unknown ones included).
    ActionNotAllowed,
    /// A test that would run a command outside the sandbox.
    RunsCode,
    /// A `projectRoot` other than the run's project.
    OtherProject,
    /// A fetching action from a run whose sandbox blocks the network.
    NetworkBlocked,
    /// A chat read (`recall`, `search_memory`) whose tool the run does not have.
    ToolUnavailable,
}

impl Reason {
    fn label(self) -> &'static str {
        match self {
            Self::MethodNotAllowed => "method_not_allowed",
            Self::ActionNotAllowed => "action_not_allowed",
            Self::RunsCode => "runs_code",
            Self::OtherProject => "other_project",
            Self::NetworkBlocked => "network_blocked",
            Self::ToolUnavailable => "tool_unavailable",
        }
    }
}

/// A refused call: why, and the answer the agent gets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Refusal {
    pub reason: Reason,
    pub message: String,
}

/// A credential as a map key. Its comparisons are counted in tests, which shows a check
/// looks at one entry however many credentials exist.
#[derive(Clone, Eq)]
struct Key(String);

impl std::hash::Hash for Key {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

impl PartialEq for Key {
    fn eq(&self, other: &Self) -> bool {
        #[cfg(test)]
        COMPARED.with(|count| count.set(count.get() + 1));
        self.0 == other.0
    }
}

#[cfg(test)]
thread_local! {
    static COMPARED: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

#[derive(Default)]
struct Issued {
    by_token: HashMap<Key, Grant>,
    by_run: HashMap<(String, String), String>,
    by_thread: HashMap<String, Vec<String>>,
}

/// The person's credential and the run credentials this server issued.
pub struct AgentTokens {
    person: String,
    port: u16,
    bin_dir: Option<PathBuf>,
    issued: Mutex<Issued>,
}

impl AgentTokens {
    pub(super) fn new(person: String, port: u16, bin_dir: Option<PathBuf>) -> Self {
        Self {
            person,
            port,
            bin_dir,
            issued: Mutex::new(Issued::default()),
        }
    }

    fn issued(&self) -> std::sync::MutexGuard<'_, Issued> {
        self.issued
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }

    /// Who a credential belongs to: `Ok(None)` is the person, `Ok(Some)` a run.
    pub(crate) fn caller(&self, token: &str) -> Result<Option<Grant>, String> {
        if same(token.as_bytes(), self.person.as_bytes()) {
            return Ok(None);
        }
        self.issued()
            .by_token
            .get(&Key(token.to_owned()))
            .cloned()
            .map(Some)
            .ok_or_else(|| super::UNAUTHORIZED.to_owned())
    }

    /// Credentials currently issued (tests).
    #[cfg(test)]
    fn len(&self) -> usize {
        self.issued().by_token.len()
    }
}

impl themis_core::agent_cli::Issuer for AgentTokens {
    fn issue(
        &self,
        thread_id: &str,
        run_id: &str,
        scope: &themis_core::agent_cli::RunScope,
    ) -> Option<themis_core::agent_cli::Access> {
        let mut issued = self.issued();
        let run = (thread_id.to_owned(), run_id.to_owned());
        let token = match issued.by_run.get(&run) {
            Some(token) => token.clone(),
            None => {
                let token = format!("{}{}", uuid::Uuid::new_v4(), uuid::Uuid::new_v4());
                // Canonical, as the call's `projectRoot` is compared canonical.
                let project_root = scope
                    .project_root
                    .as_deref()
                    .map(|root| root.canonicalize().unwrap_or_else(|_| root.to_path_buf()));
                issued.by_token.insert(
                    Key(token.clone()),
                    Grant {
                        thread_id: run.0.clone(),
                        run_id: run.1.clone(),
                        project_root,
                        network_blocked: scope.network_blocked,
                        reads: scope.reads.clone(),
                    },
                );
                issued
                    .by_thread
                    .entry(run.0.clone())
                    .or_default()
                    .push(token.clone());
                issued.by_run.insert(run, token.clone());
                token
            }
        };
        Some(themis_core::agent_cli::Access::new(
            token,
            self.port,
            self.bin_dir.clone(),
        ))
    }

    fn revoke_thread(&self, thread_id: &str) {
        let mut issued = self.issued();
        for token in issued.by_thread.remove(thread_id).unwrap_or_default() {
            issued.by_token.remove(&Key(token));
        }
        issued.by_run.retain(|(thread, _), _| thread != thread_id);
    }
}

/// Byte comparison that takes the same time wherever the first difference is.
fn same(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .fold(0u8, |differ, (a, b)| differ | (a ^ b))
            == 0
}

/// Why an agent's call is refused, or `None` when it may go ahead.
pub(crate) fn refusal(grant: &Grant, method: &str, args: &Value) -> Option<Refusal> {
    let own_chat = OWN_CHAT.contains(&method);
    let chat_read = CHAT_READS.contains(&method);
    let thread = args.get("threadId").and_then(Value::as_str);
    let permitted = (ALLOWED.contains(&method) || own_chat || chat_read)
        && thread.is_none_or(|thread| thread == grant.thread_id)
        && (!own_chat || thread.is_some());
    if !permitted {
        // Only plain names go back: the method is the caller's own text.
        return Some(refused(Reason::MethodNotAllowed, method_label(method)));
    }
    if chat_read && !grant.reads.iter().any(|read| read == method) {
        return Some(Refusal {
            reason: Reason::ToolUnavailable,
            message: themis_core::prompts::fill(
                themis_core::prompts::AGENT_CLI_READ_UNAVAILABLE,
                &[("method", method)],
            ),
        });
    }
    if method != "plugin_action" {
        return None;
    }
    plugin_action_refusal(grant, args)
}

/// What an agent's `plugin_action` may do: a listed action, in its run's project, and nothing
/// that fetches when its run blocks the network. Decided on the request, never on command text.
fn plugin_action_refusal(grant: &Grant, args: &Value) -> Option<Refusal> {
    let action = args.get("action").and_then(Value::as_str).unwrap_or("");
    let starts = |key: &str| args.get(key).and_then(Value::as_bool) == Some(true);
    let reason = if RUNS_CODE_ACTIONS.contains(&action)
        || (action == "mcp_status" && (starts("connect") || starts("retry")))
    {
        Some(Reason::RunsCode)
    } else if !LOCAL_ACTIONS.contains(&action) && !NETWORK_ACTIONS.contains(&action) {
        Some(Reason::ActionNotAllowed)
    } else if !in_run_project(grant, args) {
        Some(Reason::OtherProject)
    } else if grant.network_blocked && NETWORK_ACTIONS.contains(&action) {
        Some(Reason::NetworkBlocked)
    } else {
        None
    };
    let reason = reason?;
    Some(match reason {
        Reason::RunsCode => Refusal {
            reason,
            message: themis_core::prompts::AGENT_CLI_RUNS_CODE_REFUSED.to_owned(),
        },
        Reason::OtherProject => Refusal {
            reason,
            message: themis_core::prompts::AGENT_CLI_OTHER_PROJECT_REFUSED.to_owned(),
        },
        Reason::NetworkBlocked => Refusal {
            reason,
            message: themis_core::prompts::AGENT_CLI_NETWORK_BLOCKED.to_owned(),
        },
        _ => {
            let shown = format!("plugin_action {}", method_label(action));
            refused(reason, &shown)
        }
    })
}

/// An agent's call as the server runs it: a chat read names the agent's own chat (the refusal
/// check already turned away any other one).
fn bound(grant: &Grant, method: &str, mut args: Value) -> Value {
    if CHAT_READS.contains(&method) {
        if !args.is_object() {
            args = json!({});
        }
        args["threadId"] = json!(grant.thread_id);
    }
    args
}

/// A call without `projectRoot` (or with a non-text one, which the handler reads as none) works
/// globally; one naming a project must name the run's own, compared canonical as the handler
/// opens it.
fn in_run_project(grant: &Grant, args: &Value) -> bool {
    let Some(named) = args.get("projectRoot").and_then(Value::as_str) else {
        return true;
    };
    let named = PathBuf::from(named.trim()).canonicalize();
    matches!((named, &grant.project_root), (Ok(named), Some(root)) if named == *root)
}

fn refused(reason: Reason, shown: &str) -> Refusal {
    Refusal {
        reason,
        message: themis_core::prompts::fill(
            themis_core::prompts::AGENT_CLI_REFUSED,
            &[("method", shown)],
        ),
    }
}

/// The method as the log may keep it: a method name, never other text.
fn method_label(method: &str) -> &str {
    let plain = !method.is_empty()
        && method.len() <= 64
        && method
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte == b'_');
    if plain {
        method
    } else {
        "(other)"
    }
}

/// Screens one request: answers it when it is not the person's (refused credential or an
/// agent's call) and returns `None` for the person's calls, which go on as before.
pub(super) async fn screen(
    stream: &mut TcpStream,
    state: &AppState,
    sink: &Arc<dyn EventSink>,
    tokens: &AgentTokens,
    request: &Request,
) -> Option<Result<(), String>> {
    let grant = match tokens.caller(&request.token) {
        Ok(None) => return None,
        Ok(Some(grant)) => grant,
        Err(error) => return Some(reply(stream, state, &request.method, Err(error)).await),
    };
    let result = match refusal(&grant, &request.method, &request.args) {
        Some(refused) => {
            let action = request.args.get("action").and_then(Value::as_str);
            state.log_server_details(
                "agent_call_refused",
                "warn",
                json!({
                    "method": method_label(&request.method),
                    "action": action.map(method_label),
                    "reason": refused.reason.label(),
                    "thread_id": grant.thread_id,
                    "run_id": grant.run_id,
                }),
            );
            Err(refused.message)
        }
        None if request.method == "plugin_action" => {
            // As the agent's: no consent to start a connection, and never a hook or a
            // command-based MCP server left switched on.
            state.plugin_action_for_agent(request.args.clone()).await
        }
        None => {
            let args = bound(&grant, &request.method, request.args.clone());
            dispatch(state, Arc::clone(sink), &request.method, args).await
        }
    };
    Some(reply(stream, state, &request.method, result).await)
}

/// The folder that puts the app's own `themis` first on an agent's `PATH`: `agent-cli/` in the
/// app data (which agents cannot write), holding only a link named `themis` to `cli`. `None`
/// where it cannot be made (Windows ships no console CLI in the app).
pub(super) fn link_cli(data_dir: &Path, cli: Option<&Path>) -> Option<PathBuf> {
    #[cfg(unix)]
    {
        let target = cli?.canonicalize().ok()?;
        let folder = data_dir.join("agent-cli");
        std::fs::create_dir_all(&folder).ok()?;
        let link = folder.join("themis");
        if std::fs::read_link(&link).ok().as_deref() == Some(target.as_path()) {
            return Some(folder);
        }
        let fresh = folder.join(format!(".themis-{}", uuid::Uuid::new_v4()));
        std::os::unix::fs::symlink(&target, &fresh).ok()?;
        if std::fs::rename(&fresh, &link).is_err() {
            let _ = std::fs::remove_file(&fresh);
            return None;
        }
        Some(folder)
    }
    #[cfg(not(unix))]
    {
        let _ = (data_dir, cli);
        None
    }
}

/// The CLI this server's executable provides, when it is one (`themis` or the desktop app,
/// which acts as the CLI when started as `themis`).
pub(super) fn own_cli() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let stem = exe.file_stem()?.to_str()?;
    matches!(stem, "themis" | "themis-desktop").then_some(exe)
}

#[cfg(test)]
mod tests {
    use super::*;
    use themis_core::agent_cli::{Issuer, RunScope};

    fn tokens() -> AgentTokens {
        AgentTokens::new("person-token".into(), 4711, None)
    }

    fn grant() -> Grant {
        Grant {
            thread_id: "chat-a".into(),
            run_id: "run-1".into(),
            project_root: None,
            network_blocked: false,
            reads: Vec::new(),
        }
    }

    fn unbound() -> RunScope {
        RunScope::default()
    }

    fn reason(refusal: Option<Refusal>) -> Option<Reason> {
        refusal.map(|refused| refused.reason)
    }

    #[test]
    fn a_run_credential_names_its_run_until_the_chat_run_ends() {
        let tokens = tokens();
        assert_eq!(tokens.caller("person-token"), Ok(None));
        assert_eq!(tokens.caller("forged").unwrap_err(), "unauthorized");
        let first = tokens.issue("chat-a", "run-1", &unbound()).unwrap();
        let again = tokens.issue("chat-a", "run-1", &unbound()).unwrap();
        assert_eq!(first, again, "one credential per run");
        let other = tokens.issue("chat-b", "run-2", &unbound()).unwrap();
        let token = |access: &themis_core::agent_cli::Access| access.env()[0].1.clone();
        assert_ne!(token(&first), token(&other));
        assert_eq!(tokens.caller(&token(&first)), Ok(Some(grant())));
        tokens.revoke_thread("chat-a");
        assert_eq!(tokens.caller(&token(&first)).unwrap_err(), "unauthorized");
        assert!(tokens.caller(&token(&other)).unwrap().is_some());
        let next = tokens.issue("chat-a", "run-3", &unbound()).unwrap();
        assert_ne!(
            token(&next),
            token(&first),
            "a new run gets a new credential"
        );
    }

    /// The first issue binds the credential to the run's project (canonical) and network block;
    /// a later command of the same run cannot rebind it.
    #[test]
    fn a_run_credential_keeps_the_scope_it_was_issued_with() {
        let tokens = tokens();
        let project = tempfile::tempdir().unwrap();
        let scope = RunScope {
            project_root: Some(project.path().join(".").join("")),
            network_blocked: true,
            reads: vec!["recall".into()],
        };
        let access = tokens.issue("chat-a", "run-1", &scope).unwrap();
        let _ = tokens.issue("chat-a", "run-1", &unbound()).unwrap();
        let grant = tokens.caller(&access.env()[0].1).unwrap().unwrap();
        assert_eq!(
            grant.project_root,
            Some(project.path().canonicalize().unwrap())
        );
        assert!(grant.network_blocked);
        assert_eq!(grant.reads, vec!["recall"], "a later issue cannot widen it");
    }

    #[test]
    fn only_listed_methods_pass_and_only_for_the_agents_own_chat() {
        let grant = grant();
        let list = json!({"action": "list"});
        for method in [
            "plugin_action",
            "ping",
            "get_settings",
            "list_prompt_skills",
        ] {
            assert_eq!(refusal(&grant, method, &list), None, "{method}");
        }
        for method in [
            "approve_action",
            "answer_question",
            "set_thread_approval_mode",
            "save_runtime_configuration",
            "update_settings",
            "set_secret",
            "terminal_input",
            "create_project",
            "send_message",
            "stop_thread",
            "accept_file",
            "browser_navigate",
            "shutdown",
            "subscribe",
            "delete_memory",
            "no_such_method",
        ] {
            let refused = refusal(&grant, method, &Value::Null).expect(method);
            assert_eq!(refused.reason, Reason::MethodNotAllowed);
            assert!(refused.message.contains(method), "{refused:?}");
        }
        let own = json!({"threadId": "chat-a"});
        let other = json!({"threadId": "chat-b"});
        assert_eq!(refusal(&grant, "get_thread_tasks", &own), None);
        assert!(refusal(&grant, "get_thread_tasks", &other).is_some());
        assert!(refusal(&grant, "get_thread_tasks", &Value::Null).is_some());
        assert!(refusal(&grant, "get_effective_permissions", &other).is_some());
    }

    /// `plugin_action` passes only for a listed action: sign-in and secrets stay the person's,
    /// a test that runs a command is refused as such, and a missing or unknown action is refused.
    #[test]
    fn only_listed_plugin_actions_pass() {
        let grant = grant();
        let action = |action: Value| refusal(&grant, "plugin_action", &json!({"action": action}));
        for listed in LOCAL_ACTIONS.iter().chain(NETWORK_ACTIONS) {
            assert_eq!(action(json!(listed)), None, "{listed}");
        }
        let sign_in = action(json!("mcp_sign_in")).unwrap();
        assert_eq!(sign_in.reason, Reason::ActionNotAllowed);
        assert!(sign_in.message.contains("plugin_action mcp_sign_in"));
        for person_only in [
            "mcp_allow_secrets",
            "mcp_sign_out",
            "mcp_sign_in_cancel",
            "no_such_action",
            "",
        ] {
            assert_eq!(
                reason(action(json!(person_only))),
                Some(Reason::ActionNotAllowed),
                "{person_only}"
            );
        }
        for missing in [Value::Null, json!(7), json!({})] {
            let args = json!({"scope": "global", "action": missing});
            assert_eq!(
                reason(refusal(&grant, "plugin_action", &args)),
                Some(Reason::ActionNotAllowed)
            );
        }
        assert_eq!(
            reason(refusal(&grant, "plugin_action", &Value::Null)),
            Some(Reason::ActionNotAllowed)
        );
        for test in ["test_hook", "test_mcp"] {
            let refused = action(json!(test)).unwrap();
            assert_eq!(refused.reason, Reason::RunsCode);
            assert_eq!(
                refused.message,
                themis_core::prompts::AGENT_CLI_RUNS_CODE_REFUSED
            );
        }
        for starts in ["connect", "retry"] {
            let args = json!({"action": "mcp_status", starts: true});
            assert_eq!(
                reason(refusal(&grant, "plugin_action", &args)),
                Some(Reason::RunsCode)
            );
            let args = json!({"action": "mcp_status", starts: false});
            assert_eq!(refusal(&grant, "plugin_action", &args), None);
        }
    }

    /// A `projectRoot` must be the run's project, compared canonical; none keeps global scope.
    #[test]
    fn a_plugin_action_stays_in_the_runs_project() {
        let project = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let root = project.path().canonicalize().unwrap();
        let bound = Grant {
            project_root: Some(root.clone()),
            ..grant()
        };
        let save = |root: Value| json!({"action": "save", "scope": "local", "projectRoot": root});
        assert_eq!(refusal(&bound, "plugin_action", &save(json!(root))), None);
        let spelled = format!(" {}/./ ", project.path().display());
        assert_eq!(
            refusal(&bound, "plugin_action", &save(json!(spelled))),
            None
        );
        assert_eq!(refusal(&bound, "plugin_action", &save(Value::Null)), None);
        assert_eq!(
            refusal(&bound, "plugin_action", &json!({"action": "save"})),
            None
        );
        for elsewhere in [
            json!(other.path()),
            json!(root.join("missing")),
            json!(format!("{}/..", root.display())),
            json!(""),
        ] {
            let refused = refusal(&bound, "plugin_action", &save(elsewhere.clone())).unwrap();
            assert_eq!(refused.reason, Reason::OtherProject, "{elsewhere}");
            assert_eq!(
                refused.message,
                themis_core::prompts::AGENT_CLI_OTHER_PROJECT_REFUSED
            );
        }
        // A credential without a project names none.
        let unbound = grant();
        assert_eq!(
            reason(refusal(&unbound, "plugin_action", &save(json!(root)))),
            Some(Reason::OtherProject)
        );
    }

    #[test]
    fn a_network_block_refuses_the_fetching_actions_only() {
        let blocked = Grant {
            network_blocked: true,
            ..grant()
        };
        for action in NETWORK_ACTIONS {
            let refused = refusal(&blocked, "plugin_action", &json!({"action": action})).unwrap();
            assert_eq!(refused.reason, Reason::NetworkBlocked, "{action}");
            assert_eq!(
                refused.message,
                themis_core::prompts::AGENT_CLI_NETWORK_BLOCKED
            );
        }
        for action in LOCAL_ACTIONS {
            assert_eq!(
                refusal(&blocked, "plugin_action", &json!({"action": action})),
                None,
                "{action}"
            );
        }
    }

    /// The lists do not overlap, and the in-run management tool's executable tests are the ones
    /// refused here.
    #[test]
    fn the_action_lists_are_disjoint() {
        for action in LOCAL_ACTIONS {
            assert!(!NETWORK_ACTIONS.contains(action), "{action}");
            assert!(!RUNS_CODE_ACTIONS.contains(action), "{action}");
        }
        for action in NETWORK_ACTIONS {
            assert!(!RUNS_CODE_ACTIONS.contains(action), "{action}");
        }
    }

    /// The shell's approval step treats `themis call METHOD` as a read for exactly the reading
    /// methods this guard allows (see `themis_core::tools::themis_cli`).
    #[test]
    fn the_shell_reads_exactly_the_allowed_reading_methods() {
        let mut allowed: Vec<&str> = ALLOWED
            .iter()
            .chain(OWN_CHAT)
            .chain(&CHAT_READS)
            .copied()
            .filter(|method| *method != "plugin_action")
            .collect();
        let mut reads = themis_core::tools::themis_cli::AGENT_READ_METHODS.to_vec();
        allowed.sort_unstable();
        reads.sort_unstable();
        assert_eq!(allowed, reads);
    }

    /// `recall` and `search_memory` pass only when the run has the tool and only for its own
    /// chat; a call naming none is bound to it, one naming another chat is refused.
    #[test]
    fn chat_reads_pass_only_for_the_runs_own_tools_and_chat() {
        let with = Grant {
            reads: vec!["recall".into(), "search_memory".into()],
            ..grant()
        };
        for method in CHAT_READS {
            assert_eq!(refusal(&with, method, &json!({"query": "x"})), None);
            assert_eq!(refusal(&with, method, &json!({"threadId": "chat-a"})), None);
            assert_eq!(refusal(&with, method, &Value::Null), None);
            assert_eq!(
                reason(refusal(&with, method, &json!({"threadId": "chat-b"}))),
                Some(Reason::MethodNotAllowed)
            );
            let lacking = refusal(&grant(), method, &json!({"query": "x"})).unwrap();
            assert_eq!(lacking.reason, Reason::ToolUnavailable);
            assert!(lacking.message.contains(method), "{lacking:?}");
            for args in [
                json!({"query": "x"}),
                Value::Null,
                json!({"threadId": "chat-b"}),
            ] {
                assert_eq!(bound(&with, method, args)["threadId"], "chat-a");
            }
        }
        let only_recall = Grant {
            reads: vec!["recall".into()],
            ..grant()
        };
        assert_eq!(refusal(&only_recall, "recall", &Value::Null), None);
        assert_eq!(
            reason(refusal(&only_recall, "search_memory", &Value::Null)),
            Some(Reason::ToolUnavailable)
        );
        // Other methods keep their arguments as sent.
        let args = json!({"threadId": "chat-b"});
        assert_eq!(bound(&with, "get_thread", args.clone()), args);
    }

    #[test]
    fn the_log_keeps_method_names_only() {
        assert_eq!(method_label("approve_action"), "approve_action");
        assert_eq!(method_label("secret value"), "(other)");
        assert_eq!(method_label(&"a".repeat(65)), "(other)");
        assert_eq!(method_label(""), "(other)");
    }

    proptest::proptest! {
        /// Whatever a model sends, a call passes only for a listed method and its own chat.
        #[test]
        fn arbitrary_calls_pass_only_when_listed(
            method in "[a-z_]{0,24}|\\PC{0,24}",
            thread in proptest::option::of("[a-z-]{0,8}"),
        ) {
            let mut args = json!({"action": "list"});
            if let Some(thread) = &thread { args["threadId"] = json!(thread); }
            let passed = refusal(&grant(), &method, &args).is_none();
            // `grant()` has no chat-read tools, so those never pass either.
            let listed = ALLOWED.contains(&method.as_str()) || OWN_CHAT.contains(&method.as_str());
            let own = thread.as_deref().is_none_or(|thread| thread == "chat-a");
            let needs_chat = OWN_CHAT.contains(&method.as_str());
            proptest::prop_assert_eq!(passed, listed && own && (!needs_chat || thread.is_some()));
        }

        /// Whatever action a model sends, `plugin_action` passes only for a listed one, and only
        /// for a local one behind a network block.
        #[test]
        fn arbitrary_plugin_actions_pass_only_when_listed(
            action in "[a-z_]{0,24}|\\PC{0,24}|list|save|install|test_hook|mcp_status",
            blocked in proptest::bool::ANY,
            connect in proptest::option::of(proptest::bool::ANY),
        ) {
            let grant = Grant { network_blocked: blocked, ..grant() };
            let mut args = json!({"action": action});
            if let Some(connect) = connect { args["connect"] = json!(connect); }
            let passed = refusal(&grant, "plugin_action", &args).is_none();
            let starts = action == "mcp_status" && connect == Some(true);
            let local = LOCAL_ACTIONS.contains(&action.as_str()) && !starts;
            let network = NETWORK_ACTIONS.contains(&action.as_str()) && !blocked;
            proptest::prop_assert_eq!(passed, local || network);
        }
    }

    /// The check per call looks at one credential however many runs were issued one.
    #[test]
    fn the_guard_check_stays_flat_with_many_issued_credentials() {
        let samples = crate::scale::measure(&[100, 8000], |size| {
            let tokens = tokens();
            let mut issued = Vec::new();
            for index in 0..size {
                let access = tokens
                    .issue(&format!("chat-{index}"), "run", &unbound())
                    .unwrap();
                issued.push(access.env()[0].1.clone());
            }
            assert_eq!(tokens.len(), size);
            COMPARED.with(|count| count.set(0));
            let list = json!({"action": "list"});
            for token in issued.iter().take(100) {
                let grant = tokens.caller(token).unwrap().unwrap();
                assert!(refusal(&grant, "plugin_action", &list).is_none());
                assert!(refusal(&grant, "approve_action", &Value::Null).is_some());
            }
            assert!(tokens.caller("forged").is_err());
            COMPARED.with(std::cell::Cell::get)
        });
        let rate = crate::scale::growth_rate(&samples);
        assert!(rate.abs() < 0.1, "guard work grew: {samples:?}");
    }
}
