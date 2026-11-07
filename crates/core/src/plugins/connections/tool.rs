//! A connection's tools as agent tools, and the approval to start a server.

use super::*;

pub(super) struct McpTool {
    pub(super) name: String,
    pub(super) remote: String,
    pub(super) description: String,
    pub(super) schema: Value,
    pub(super) connection: Arc<tokio::sync::Mutex<Connection>>,
    pub(super) healthy: Arc<std::sync::atomic::AtomicBool>,
    /// How to start a local server again; a remote one has no process to bring back.
    pub(super) restart: Option<Arc<(McpServer, PathBuf)>>,
    pub(super) approvals: Arc<dyn ApprovalHook>,
}

impl std::fmt::Debug for McpTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("McpTool").field(&self.name).finish()
    }
}

#[async_trait]
impl ToolRuntime for McpTool {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        crate::tools::check_approval_permitted(
            self.approvals.as_ref(),
            &ToolAction {
                display: None,
                command: Vec::new(),
                tool: self.name.clone(),
                summary: format!("Call MCP tool {}", self.remote),
                risk: RiskLevel::Network,
            },
        )?;
        let mut connection = self.connection.lock().await;
        if let Some(restart) = &self.restart {
            if connection.stopped() || !self.healthy.load(std::sync::atomic::Ordering::Relaxed) {
                *connection = Connection::connect(&restart.0, &restart.1)
                    .await
                    .map_err(|e| ToolCallError::RuntimeError(e.to_string().into()))?;
                self.healthy
                    .store(true, std::sync::atomic::Ordering::Relaxed);
                crate::diagnostics::emit("mcp", "server_restarted", "info", json!({}));
            }
        }
        let result = connection
            .request("tools/call", json!({"name":self.remote,"arguments":args}))
            .await
            .map_err(|e| {
                if !e.is::<RequestRejected>() {
                    self.healthy
                        .store(false, std::sync::atomic::Ordering::Relaxed);
                }
                let timed_out = e.to_string() == "MCP request timed out";
                ToolCallError::RuntimeError(
                    if timed_out && self.restart.is_some() {
                        crate::prompts::fill(
                            crate::prompts::MCP_TOOL_TIMEOUT,
                            &[("seconds", &connection.tool_deadline.as_secs().to_string())],
                        )
                    } else {
                        e.to_string()
                    }
                    .into(),
                )
            })?;
        if result["isError"] == true {
            return Err(ToolCallError::RuntimeError(tool_error_text(&result).into()));
        }
        Ok(result)
    }
}

/// Most characters of an MCP tool's own error text passed to the model.
const MAX_TOOL_ERROR_CHARS: usize = 4_000;

/// An MCP tool's error with the server's explanation (its text content,
/// bounded), so the agent can fix the call instead of retrying blind.
fn tool_error_text(result: &Value) -> String {
    let text = result["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| item["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let text = text.trim();
    if text.is_empty() {
        return "MCP tool reported an error".to_owned();
    }
    let count = text.chars().count();
    let mut message = format!(
        "MCP tool reported an error: {}",
        text.chars().take(MAX_TOOL_ERROR_CHARS).collect::<String>()
    );
    if count > MAX_TOOL_ERROR_CHARS {
        message.push_str(&format!(
            " [… {} more characters]",
            count - MAX_TOOL_ERROR_CHARS
        ));
    }
    message
}

impl ToolT for McpTool {
    fn name(&self) -> &str {
        &self.name
    }
    fn description(&self) -> &str {
        &self.description
    }
    fn args_schema(&self) -> Value {
        self.schema.clone()
    }
    fn output_schema(&self) -> Option<Value> {
        None
    }
}

/// A server's tools for one run, its instructions, and each tool's Themis name with the server's
/// own name for it (the Themis name cuts long ones short).
pub type McpTools = (Vec<Box<dyn ToolT>>, Option<String>, Vec<(String, String)>);

pub async fn tools(
    name: &str,
    server: &McpServer,
    root: &Path,
    approvals: Arc<dyn ApprovalHook>,
) -> anyhow::Result<McpTools> {
    authorize_start(name, server, approvals.as_ref())?;
    Session::connect(server, root).await?.tools(name, approvals)
}

pub fn startup_action(name: &str, server: &McpServer) -> anyhow::Result<ToolAction> {
    let transport = server
        .command
        .as_ref()
        .or(server.url.as_ref())
        .context("Missing MCP transport")?;
    // Local servers show their structured command, never a joined string.
    let display = server.command.as_ref().map(|command| CommandDisplay {
        argv: std::iter::once(command.clone())
            .chain(server.args.iter().cloned())
            .collect(),
        cwd: None,
        background: true,
        sandbox_blocked: None,
    });
    Ok(ToolAction {
        display,
        command: Vec::new(),
        tool: format!("mcp_start_{name}"),
        summary: if server.command.is_some() {
            "Connect local MCP server".to_owned()
        } else {
            format!("Connect remote MCP server {transport}")
        },
        risk: if server.command.is_some() {
            RiskLevel::Execute
        } else {
            RiskLevel::Network
        },
    })
}

/// Whether an explicit deny rule blocks starting this server. Enabling a
/// connection is the consent to start it, so a run asks nothing here; only a
/// deny rule stops it. YOLO callers skip this check.
pub fn startup_denied(
    name: &str,
    server: &McpServer,
    policy: &crate::configuration::ApprovalPolicy,
) -> anyhow::Result<bool> {
    let action = startup_action(name, server)?;
    Ok(policy.decision(&action) == crate::configuration::PolicyAction::Deny)
}

pub fn authorize_start(
    name: &str,
    server: &McpServer,
    approvals: &dyn ApprovalHook,
) -> anyhow::Result<()> {
    let action = startup_action(name, server)?;
    if approvals.approve(&action).is_denied() {
        crate::diagnostics::emit("mcp", "startup_denied", "warn", json!({"server":name}));
        bail!("MCP startup denied");
    }
    Ok(())
}
