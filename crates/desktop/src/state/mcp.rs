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

pub(super) type Connections = HashMap<PathBuf, HashMap<String, Arc<McpEntry>>>;
type SessionResult = Result<Arc<Session>, String>;
pub(super) struct McpEntry {
    fingerprint: String,
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
        match self.result.borrow().as_ref() {
            None => json!({"status":"connecting"}),
            Some(Ok(session)) if session.is_connected() => json!({"status":"connected"}),
            Some(Ok(_)) => {
                json!({"status":"failed","reason":"Connection closed. Refresh to reconnect."})
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
        blocked: bool,
        retry: bool,
    ) -> Arc<McpEntry> {
        let key = format!("{}:{}:mcp:{name}", plugin.scope, plugin.spec.name);
        let fingerprint = format!(
            "{}:{blocked}:{}",
            plugin.revision,
            serde_json::to_string(&server).expect("MCP config serializes")
        );
        let mut projects = self.inner.mcp.lock().unwrap_or_else(|e| e.into_inner());
        let entries = projects.entry(root.to_owned()).or_default();
        if let Some(entry) = entries.get(&key) {
            if entry.fingerprint == fingerprint && !(retry && entry.status()["status"] == "failed")
            {
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
                    if blocked {
                        return Err(
                            "Connection blocked by project or user approval policy".to_owned()
                        );
                    }
                    store
                        .materialize(&[plugin], &root)
                        .map_err(|_| "MCP resources could not be prepared".to_owned())?;
                    Session::connect(&server, &root)
                        .await
                        .map(Arc::new)
                        .map_err(|error| {
                            match error.to_string().as_str() {
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
                let blocked =
                    policy.decision(&action) == themis_core::configuration::PolicyAction::Deny;
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
    ) -> anyhow::Result<connections::McpTools> {
        let server = resolved_server(plugin, name, root);
        connections::authorize_start(&plugin.skill_id(name), &server, approvals.as_ref())?;
        let entry = self.mcp_entry(root, plugin, name, server, false, false);
        let session = entry.ready().await.map_err(anyhow::Error::msg)?;
        session.tools(&plugin.skill_id(name), approvals)
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
