//! Calls from an agent's shell: run credentials and the guard on what they may call.
//!
//! The person's credential lives in `server.json`, which sandboxed commands cannot read. Each
//! run's commands get their own credential instead (`THEMIS_AGENT_TOKEN`, issued on the run's
//! first command and revoked when the chat's run ends). A call carrying it is the agent's: it may
//! read status and manage integrations through the store, and nothing else. The allowed list is
//! explicit, so a method added later stays refused until someone decides otherwise. See
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

/// `plugin_action` actions that only the person may take: secrets for a connection and sign-in.
const PERSON_ONLY_ACTIONS: &[&str] = &[
    "mcp_allow_secrets",
    "mcp_sign_in",
    "mcp_sign_out",
    "mcp_sign_in_cancel",
];

/// The run and chat a credential belongs to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Grant {
    pub thread_id: String,
    pub run_id: String,
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
    fn issue(&self, thread_id: &str, run_id: &str) -> Option<themis_core::agent_cli::Access> {
        let mut issued = self.issued();
        let run = (thread_id.to_owned(), run_id.to_owned());
        let token = match issued.by_run.get(&run) {
            Some(token) => token.clone(),
            None => {
                let token = format!("{}{}", uuid::Uuid::new_v4(), uuid::Uuid::new_v4());
                issued.by_token.insert(
                    Key(token.clone()),
                    Grant {
                        thread_id: run.0.clone(),
                        run_id: run.1.clone(),
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
pub(crate) fn refusal(grant: &Grant, method: &str, args: &Value) -> Option<String> {
    let own_chat = OWN_CHAT.contains(&method);
    let thread = args.get("threadId").and_then(Value::as_str);
    let permitted = (ALLOWED.contains(&method) || own_chat)
        && thread.is_none_or(|thread| thread == grant.thread_id)
        && (!own_chat || thread.is_some());
    let action = args.get("action").and_then(Value::as_str);
    let person_only = method == "plugin_action" && action.is_some_and(is_person_only_action);
    if permitted && !person_only {
        return None;
    }
    // Only plain names go back: the method is the caller's own text.
    let shown = if person_only {
        format!("plugin_action {}", method_label(action.unwrap_or_default()))
    } else {
        method_label(method).to_owned()
    };
    Some(themis_core::prompts::fill(
        themis_core::prompts::AGENT_CLI_REFUSED,
        &[("method", &shown)],
    ))
}

fn is_person_only_action(action: &str) -> bool {
    PERSON_ONLY_ACTIONS.contains(&action)
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
                    "thread_id": grant.thread_id,
                    "run_id": grant.run_id,
                }),
            );
            Err(refused)
        }
        None if request.method == "plugin_action" => {
            // As the agent's: saving or enabling a connection gives no consent to start it.
            state
                .plugin_action_for_agent(request.args.clone(), Vec::new())
                .await
        }
        None => {
            dispatch(
                state,
                Arc::clone(sink),
                &request.method,
                request.args.clone(),
            )
            .await
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
    use themis_core::agent_cli::Issuer;

    fn tokens() -> AgentTokens {
        AgentTokens::new("person-token".into(), 4711, None)
    }

    fn grant() -> Grant {
        Grant {
            thread_id: "chat-a".into(),
            run_id: "run-1".into(),
        }
    }

    #[test]
    fn a_run_credential_names_its_run_until_the_chat_run_ends() {
        let tokens = tokens();
        assert_eq!(tokens.caller("person-token"), Ok(None));
        assert_eq!(tokens.caller("forged").unwrap_err(), "unauthorized");
        let first = tokens.issue("chat-a", "run-1").unwrap();
        let again = tokens.issue("chat-a", "run-1").unwrap();
        assert_eq!(first, again, "one credential per run");
        let other = tokens.issue("chat-b", "run-2").unwrap();
        let token = |access: &themis_core::agent_cli::Access| access.env()[0].1.clone();
        assert_ne!(token(&first), token(&other));
        assert_eq!(tokens.caller(&token(&first)), Ok(Some(grant())));
        tokens.revoke_thread("chat-a");
        assert_eq!(tokens.caller(&token(&first)).unwrap_err(), "unauthorized");
        assert!(tokens.caller(&token(&other)).unwrap().is_some());
        let next = tokens.issue("chat-a", "run-3").unwrap();
        assert_ne!(
            token(&next),
            token(&first),
            "a new run gets a new credential"
        );
    }

    #[test]
    fn only_listed_methods_pass_and_only_for_the_agents_own_chat() {
        let grant = grant();
        for method in [
            "plugin_action",
            "ping",
            "get_settings",
            "list_prompt_skills",
        ] {
            assert_eq!(refusal(&grant, method, &Value::Null), None, "{method}");
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
            assert!(refused.contains(method), "{refused}");
        }
        let own = json!({"threadId": "chat-a"});
        let other = json!({"threadId": "chat-b"});
        assert_eq!(refusal(&grant, "get_thread_tasks", &own), None);
        assert!(refusal(&grant, "get_thread_tasks", &other).is_some());
        assert!(refusal(&grant, "get_thread_tasks", &Value::Null).is_some());
        assert!(refusal(&grant, "get_effective_permissions", &other).is_some());
        let sign_in = json!({"action": "mcp_sign_in"});
        assert!(refusal(&grant, "plugin_action", &sign_in)
            .unwrap()
            .contains("plugin_action mcp_sign_in"));
        assert_eq!(
            refusal(&grant, "plugin_action", &json!({"action": "install"})),
            None
        );
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
            action in proptest::option::of("[a-z_]{0,20}"),
        ) {
            let mut args = json!({});
            if let Some(thread) = &thread { args["threadId"] = json!(thread); }
            if let Some(action) = &action { args["action"] = json!(action); }
            let passed = refusal(&grant(), &method, &args).is_none();
            let listed = ALLOWED.contains(&method.as_str()) || OWN_CHAT.contains(&method.as_str());
            let own = thread.as_deref().is_none_or(|thread| thread == "chat-a");
            let needs_chat = OWN_CHAT.contains(&method.as_str());
            let person_only = method == "plugin_action"
                && action.as_deref().is_some_and(is_person_only_action);
            proptest::prop_assert_eq!(
                passed,
                listed && own && (!needs_chat || thread.is_some()) && !person_only
            );
        }
    }

    /// The check per call looks at one credential however many runs were issued one.
    #[test]
    fn the_guard_check_stays_flat_with_many_issued_credentials() {
        let samples = crate::scale::measure(&[100, 8000], |size| {
            let tokens = tokens();
            let mut issued = Vec::new();
            for index in 0..size {
                let access = tokens.issue(&format!("chat-{index}"), "run").unwrap();
                issued.push(access.env()[0].1.clone());
            }
            assert_eq!(tokens.len(), size);
            COMPARED.with(|count| count.set(0));
            for token in issued.iter().take(100) {
                let grant = tokens.caller(token).unwrap().unwrap();
                assert!(refusal(&grant, "plugin_action", &Value::Null).is_none());
                assert!(refusal(&grant, "approve_action", &Value::Null).is_some());
            }
            assert!(tokens.caller("forged").is_err());
            COMPARED.with(std::cell::Cell::get)
        });
        let rate = crate::scale::growth_rate(&samples);
        assert!(rate.abs() < 0.1, "guard work grew: {samples:?}");
    }
}
