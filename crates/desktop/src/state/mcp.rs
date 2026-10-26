//! App-owned MCP transports. Saved definitions stay in the plugin registry.
use super::*;
use serde_json::{json, Value};
use themis_core::{
    plugins::{
        connections::{self, McpServer, Session},
        Plugin,
    },
    tools::ApprovalHook,
};

/// Status shown when a definition has not been enabled by a person since it last changed.
/// MCP sign-in tokens kept in the app's secret store (the Keychain in the desktop app).
pub(super) struct SecretTokens(pub(super) Arc<dyn SecretStore>);

impl themis_core::plugins::oauth::TokenStore for SecretTokens {
    fn load(&self, key: &str) -> Option<String> {
        self.0.get(key)
    }
    fn save(&self, key: &str, value: &str) -> Result<(), String> {
        self.0.set(key, value)
    }
    fn delete(&self, key: &str) -> Result<(), String> {
        self.0.clear(key)
    }
}

/// Sign-ins waiting for the browser, by server address.
fn pending_sign_ins() -> &'static std::sync::Mutex<HashMap<String, Arc<tokio::sync::Notify>>> {
    static PENDING: std::sync::OnceLock<
        std::sync::Mutex<HashMap<String, Arc<tokio::sync::Notify>>>,
    > = std::sync::OnceLock::new();
    PENDING.get_or_init(Default::default)
}

/// The answer to a sign-in the person cancelled (the UI shows no error for it).
const SIGN_IN_CANCELLED: &str = "Sign-in cancelled";

/// The connection failure that means "sign in", kept apart from other failures.
const NEEDS_SIGN_IN: &str = "Sign in to use this server.";

pub(super) const NEEDS_CONSENT: &str =
    "Needs your approval: enable this connection yourself in Integrations or the CLI";

pub(super) type Connections = HashMap<PathBuf, HashMap<String, Arc<McpEntry>>>;
type SessionResult = Result<Arc<Session>, String>;
pub(super) struct McpEntry {
    fingerprint: String,
    /// A remote server's address when it may use a sign-in (no token variable of its own).
    remote: Option<String>,
    result: tokio::sync::watch::Receiver<Option<SessionResult>>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for McpEntry {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl McpEntry {
    async fn ready(&self) -> SessionResult {
        let mut result = self.result.clone();
        loop {
            if let Some(value) = result.borrow().clone() {
                return value;
            }
            result
                .changed()
                .await
                .map_err(|_| "MCP startup stopped".to_owned())?;
        }
    }
    fn status(&self) -> Value {
        let mut status = self.connection_status();
        if let Some(url) = &self.remote {
            status["signed_in"] = json!(themis_core::plugins::oauth::signed_in(url));
            status["url"] = json!(url);
        }
        status
    }
    fn connection_status(&self) -> Value {
        match self.result.borrow().as_ref() {
            None => json!({"status":"connecting"}),
            Some(Ok(session)) if session.is_connected() => json!({"status":"connected"}),
            Some(Ok(_)) => {
                json!({"status":"failed","reason":"Connection closed. Refresh to reconnect."})
            }
            Some(Err(reason)) if reason == NEEDS_SIGN_IN => {
                json!({"status":"needs_sign_in","reason":reason})
            }
            Some(Err(reason)) if reason.starts_with(themis_core::plugins::keys::NEEDS_KEY) => {
                let names = &reason[themis_core::plugins::keys::NEEDS_KEY.len()..];
                json!({
                    "status": "needs_key",
                    "keys": names.split(", ").collect::<Vec<_>>(),
                    "reason": format!("Add {names} in Settings › Keys."),
                })
            }
            Some(Err(reason)) => json!({"status":"failed","reason":reason}),
        }
    }
}

impl AppState {
    fn mcp_entry(
        &self,
        root: &Path,
        plugin: &Plugin,
        name: &str,
        server: McpServer,
        blocked: Option<&'static str>,
        retry: bool,
    ) -> Arc<McpEntry> {
        let key = format!("{}:{}:mcp:{name}", plugin.scope, plugin.spec.name);
        let remote_url = server.url.clone().filter(|_| !server.has_credentials());
        let fingerprint = format!(
            "{}:{blocked:?}:{}",
            plugin.revision,
            serde_json::to_string(&server).expect("MCP config serializes")
        );
        let mut projects = self.inner.mcp.lock().unwrap_or_else(|e| e.into_inner());
        let entries = projects.entry(root.to_owned()).or_default();
        if let Some(entry) = entries.get(&key) {
            // A retry reconnects anything not up (also after signing in).
            let up = matches!(
                entry.connection_status()["status"].as_str(),
                Some("connected" | "connecting")
            );
            if entry.fingerprint == fingerprint && !(retry && !up) {
                return entry.clone();
            }
        }
        let (send, result) = tokio::sync::watch::channel(None);
        let root = root.to_owned();
        let plugin = plugin.clone();
        let store = self.plugin_store(Some(root.clone()));
        let context = themis_core::diagnostics::DiagnosticContext {
            log: self.inner.diagnostics.clone(),
            thread_id: String::new(),
            run_id: uuid::Uuid::new_v4().to_string(),
            level: self
                .runtime_configuration(Some(&root), None)
                .map(|r| r.config.logging.level)
                .unwrap_or_else(|_| "info".into()),
        };
        let task = tokio::spawn(async move {
            let result = tokio::time::timeout(
                std::time::Duration::from_secs(30),
                themis_core::diagnostics::ACTIVE.scope(context, async {
                    if let Some(reason) = blocked {
                        return Err(reason.to_owned());
                    }
                    store
                        .materialize(&[plugin], &root)
                        .map_err(|_| "MCP resources could not be prepared".to_owned())?;
                    Session::connect(&server, &root)
                        .await
                        .map(Arc::new)
                        .map_err(|error| {
                            if error.chain().any(|e| e.to_string() == "MCP server needs sign-in") {
                                return NEEDS_SIGN_IN.to_owned();
                            }
                            let text = error.to_string();
                            if text.starts_with(themis_core::plugins::keys::NEEDS_KEY) {
                                return text;
                            }
                            if let Some(program) = text.strip_prefix("MCP program not found: ") {
                                return format!("MCP process could not start: '{program}' was not found on this computer. Install it, or save the command with its full path.");
                            }
                            match text.as_str() {
                                "Could not start MCP server" => {
                                    "MCP process could not start. Check the saved command."
                                }
                                "Required MCP environment variable is unset"
                                | "MCP authentication variable is unset" => {
                                    "Required MCP environment variable is unavailable."
                                }
                                _ => "MCP connection or tool discovery failed. Refresh to retry.",
                            }
                            .to_owned()
                        })
                }),
            )
            .await
            .unwrap_or_else(|_| Err("MCP connection timed out. Refresh to retry.".to_owned()));
            let _ = send.send(Some(result));
        });
        let entry = Arc::new(McpEntry {
            fingerprint,
            remote: remote_url,
            result,
            task,
        });
        entries.insert(key, entry.clone());
        entry
    }
    pub(super) fn mcp_tool_summaries(&self, root: &Path, key: &str) -> Result<Value, String> {
        let projects = self.inner.mcp.lock().unwrap_or_else(|e| e.into_inner());
        let entry = projects
            .get(root)
            .and_then(|entries| entries.get(key))
            .ok_or("MCP connection is unavailable")?;
        let result = entry.result.borrow();
        match result.as_ref() {
            Some(Ok(session)) if session.is_connected() => Ok(json!(session.tool_summaries())),
            _ => Err("MCP connection is not ready".into()),
        }
    }
    pub(super) fn mcp_snapshot(&self, root: &Path) -> Value {
        Value::Object(
            self.inner
                .mcp
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(root)
                .into_iter()
                .flat_map(|entries| entries.iter())
                .map(|(key, entry)| (key.clone(), entry.status()))
                .collect(),
        )
    }
    pub(super) fn mcp_status(&self, root: &Path, retry: bool) -> Result<Value, String> {
        let plugins = self
            .plugin_store(Some(root.to_owned()))
            .list()
            .map_err(|e| e.to_string())?;
        let policy = self
            .runtime_configuration(Some(root), None)?
            .config
            .approval;
        let mut statuses = serde_json::Map::new();
        for plugin in plugins.iter().filter(|p| p.enabled) {
            for (name, _) in plugin.spec.mcp.iter().filter(|(_, s)| s.enabled) {
                let server = resolved_server(plugin, name, root);
                // Enabling a connection authorizes background startup, never tool calls.
                // Explicit user/project denials still prevent startup.
                let action = connections::startup_action(&plugin.skill_id(name), &server)
                    .map_err(|e| e.to_string())?;
                // Only a definition a person approved may start; a deny rule still wins.
                let blocked =
                    if policy.decision(&action) == themis_core::configuration::PolicyAction::Deny {
                        Some("Connection blocked by project or user approval policy")
                    } else if !self.mcp_consented(root, plugin, name, &plugin.spec.mcp[name]) {
                        Some(NEEDS_CONSENT)
                    } else {
                        None
                    };
                let entry = self.mcp_entry(root, plugin, name, server, blocked, retry);
                statuses.insert(
                    format!("{}:{}:mcp:{name}", plugin.scope, plugin.spec.name),
                    entry.status(),
                );
            }
        }
        self.inner
            .mcp
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(root.to_owned())
            .or_default()
            .retain(|key, _| statuses.contains_key(key));
        Ok(Value::Object(statuses))
    }
    pub(super) async fn mcp_tools(
        &self,
        root: &Path,
        plugin: &Plugin,
        name: &str,
        approvals: Arc<dyn ApprovalHook>,
        startup_rules: Option<&themis_core::configuration::ApprovalPolicy>,
    ) -> anyhow::Result<connections::McpTools> {
        let server = resolved_server(plugin, name, root);
        // Enabling a connection is the consent to start it: the run asks
        // nothing here, and only an explicit deny rule stops startup (YOLO
        // callers pass the policy's YOLO view: deny rules only).
        // A new or changed definition starts only after a person enabled it.
        if !self.mcp_consented(root, plugin, name, &plugin.spec.mcp[name]) {
            anyhow::bail!("MCP startup needs your approval");
        }
        if let Some(policy) = startup_rules {
            if connections::startup_denied(&plugin.skill_id(name), &server, policy)? {
                anyhow::bail!("MCP startup denied");
            }
        }
        let entry = self.mcp_entry(root, plugin, name, server, None, false);
        let session = entry.ready().await.map_err(anyhow::Error::msg)?;
        session.tools(
            &super::mcp_consent::mcp_namespace(plugin, name, &plugin.spec.mcp[name]),
            approvals,
        )
    }
    pub(super) fn refresh_mcp_connections(&self) {
        let roots: HashSet<_> = self
            .inner
            .mcp
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .keys()
            .cloned()
            .collect();
        for root in roots {
            let _ = self.mcp_status(&root, false);
        }
    }
}
fn resolved_server(plugin: &Plugin, name: &str, root: &Path) -> McpServer {
    let mut server = plugin.spec.mcp[name].clone();
    let resources = root
        .join(".themis/plugin-files")
        .join(plugin.skill_id("resources"));
    for arg in &mut server.args {
        *arg = arg.replace("${CLAUDE_PLUGIN_ROOT}", &resources.to_string_lossy());
    }
    server
}

impl AppState {
    /// Signs in to an installed remote MCP server: its sign-in page opens in the person's
    /// browser; once they finish, its connection is tried again. Only URLs of servers you have
    /// installed are accepted.
    pub(super) async fn mcp_sign_in(&self, root: &Path, url: &str) -> Result<Value, String> {
        self.installed_mcp_url(root, url)?;
        // The person may close the browser tab instead of finishing: Cancel (in Integrations)
        // stops the wait at once instead of leaving it until the time limit.
        let cancel = Arc::new(tokio::sync::Notify::new());
        pending_sign_ins()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(url.to_owned(), Arc::clone(&cancel));
        let outcome = tokio::select! {
            outcome = themis_core::plugins::oauth::sign_in(
                url,
                crate::state::spawn_opener_url,
                std::time::Duration::from_secs(300),
            ) => outcome.map_err(|e| format!("{e:#}")),
            () = cancel.notified() => Err(SIGN_IN_CANCELLED.to_owned()),
        };
        pending_sign_ins()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(url);
        outcome?;
        self.mcp_status(root, true)
    }

    /// Stops a sign-in still waiting for the browser.
    pub(super) fn mcp_sign_in_cancel(&self, url: &str) -> Result<Value, String> {
        if let Some(cancel) = pending_sign_ins()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(url)
        {
            cancel.notify_one();
        }
        Ok(Value::Null)
    }

    /// Forgets the sign-in of an installed remote MCP server.
    pub(super) fn mcp_sign_out(&self, root: &Path, url: &str) -> Result<Value, String> {
        self.installed_mcp_url(root, url)?;
        themis_core::plugins::oauth::sign_out(url)?;
        self.mcp_status(root, true)
    }

    fn installed_mcp_url(&self, root: &Path, url: &str) -> Result<(), String> {
        let plugins = self
            .plugin_store(Some(root.to_owned()))
            .list()
            .map_err(|e| e.to_string())?;
        plugins
            .iter()
            .flat_map(|plugin| plugin.spec.mcp.values())
            .any(|server| server.url.as_deref() == Some(url))
            .then_some(())
            .ok_or_else(|| "No installed MCP server has that address".to_owned())
    }
}
