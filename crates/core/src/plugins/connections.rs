//! MCP JSON-RPC transport with bounded calls and approval-gated agent tools.
use crate::tools::{ApprovalHook, RiskLevel, ToolAction, ToolRuntime, ToolT};
use anyhow::{bail, Context};
use autoagents::async_trait;
use autoagents::core::tool::ToolCallError;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{collections::BTreeMap, path::Path, sync::Arc, time::Duration};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct McpServer {
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub url: Option<String>,
    /// Environment values are references to process variables, never literal credentials.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub bearer_env: Option<String>,
    #[serde(default)]
    pub enabled: bool,
}
impl McpServer {
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.command.is_some() == self.url.is_some() {
            bail!("Specify exactly one MCP command or URL");
        }
        if self.command.as_ref().is_some_and(|c| c.trim().is_empty()) {
            bail!("MCP command is empty");
        }
        if let Some(url) = &self.url {
            let u = reqwest::Url::parse(url)?;
            if !(u.scheme() == "https"
                || (u.scheme() == "http"
                    && matches!(u.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))))
                || !u.username().is_empty()
                || u.password().is_some()
                || u.query().is_some()
            {
                bail!("Use HTTPS (or localhost HTTP) without embedded credentials");
            }
        }
        if self
            .env
            .values()
            .chain(self.bearer_env.iter())
            .any(|v| !v.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') || v.is_empty())
        {
            bail!("MCP environment values must name environment variables");
        }
        Ok(())
    }
}
enum Transport {
    Stdio {
        child: tokio::process::Child,
        _group: super::hooks::ProcessGroup,
        input: tokio::process::ChildStdin,
        output: BufReader<tokio::process::ChildStdout>,
    },
    Http {
        client: reqwest::Client,
        url: String,
        session: Option<String>,
        bearer: Option<String>,
    },
}
pub struct Connection {
    transport: Transport,
    next_id: u64,
    protocol: Option<String>,
}
impl std::fmt::Debug for Connection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MCP connection")
    }
}
impl Connection {
    pub async fn connect(server: &McpServer, root: &Path) -> anyhow::Result<Self> {
        server.validate()?;
        let transport = if let Some(command) = &server.command {
            let mut c = tokio::process::Command::new(command);
            c.args(&server.args)
                .current_dir(root)
                .env_clear()
                .kill_on_drop(true)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null());
            for name in ["PATH", "HOME", "SYSTEMROOT", "TMPDIR"] {
                if let Some(value) = std::env::var_os(name) {
                    c.env(name, value);
                }
            }
            for (name, source) in &server.env {
                c.env(
                    name,
                    std::env::var_os(source)
                        .context("Required MCP environment variable is unset")?,
                );
            }
            #[cfg(unix)]
            c.process_group(0);
            let mut child = c.spawn().context("Could not start MCP server")?;
            let input = child.stdin.take().context("Missing stdin")?;
            let output = BufReader::new(child.stdout.take().context("Missing stdout")?);
            Transport::Stdio {
                _group: super::hooks::ProcessGroup(child.id()),
                child,
                input,
                output,
            }
        } else {
            Transport::Http {
                client: reqwest::Client::builder()
                    .timeout(Duration::from_secs(30))
                    .redirect(reqwest::redirect::Policy::none())
                    .build()?,
                url: server.url.clone().context("Missing URL")?,
                session: None,
                bearer: server
                    .bearer_env
                    .as_ref()
                    .map(|name| {
                        std::env::var(name)
                            .map_err(|_| anyhow::anyhow!("MCP authentication variable is unset"))
                    })
                    .transpose()?,
            }
        };
        let mut result = Self {
            transport,
            next_id: 1,
            protocol: None,
        };
        let init=result.request("initialize",json!({"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"Themis","version":env!("CARGO_PKG_VERSION")}})).await?;
        if !matches!(
            init["protocolVersion"].as_str(),
            Some("2024-11-05" | "2025-03-26" | "2025-06-18")
        ) {
            bail!("Unsupported MCP protocol version");
        }
        result.protocol = init["protocolVersion"].as_str().map(str::to_owned);
        result
            .send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
            .await?;
        Ok(result)
    }
    async fn send(&mut self, message: Value) -> anyhow::Result<Option<Value>> {
        match &mut self.transport {
            Transport::Stdio { input, .. } => {
                let mut bytes = serde_json::to_vec(&message)?;
                bytes.push(b'\n');
                input.write_all(&bytes).await?;
                input.flush().await?;
                Ok(None)
            }
            Transport::Http {
                client,
                url,
                session,
                bearer,
            } => {
                let mut request = client
                    .post(url.as_str())
                    .header("Accept", "application/json, text/event-stream")
                    .json(&message);
                if let Some(id) = session.as_ref() {
                    request = request.header("Mcp-Session-Id", id);
                }
                if let Some(version) = &self.protocol {
                    request = request.header("MCP-Protocol-Version", version);
                }
                if let Some(token) = bearer {
                    request = request.bearer_auth(token.as_str());
                }
                let mut response = request.send().await.context("MCP connection failed")?;
                if !response.status().is_success() {
                    bail!("MCP server returned HTTP {}", response.status());
                }
                if let Some(id) = response.headers().get("mcp-session-id") {
                    *session = Some(id.to_str()?.into());
                }
                if message.get("id").is_none() {
                    return Ok(None);
                }
                let mut bytes = Vec::new();
                while let Some(chunk) = response.chunk().await? {
                    bytes.extend_from_slice(&chunk);
                    if bytes.len() > 1024 * 1024 {
                        bail!("MCP response exceeds 1 MiB");
                    }
                    if let Ok(value) = serde_json::from_slice::<Value>(&bytes) {
                        return Ok(Some(value));
                    }
                    let text = String::from_utf8_lossy(&bytes);
                    for line in text.lines() {
                        if let Some(data) = line.strip_prefix("data: ") {
                            if let Ok(value) = serde_json::from_str::<Value>(data) {
                                if value.get("id") == message.get("id") {
                                    return Ok(Some(value));
                                }
                            }
                        }
                    }
                }
                bail!("MCP response contains no matching JSON-RPC result")
            }
        }
    }
    pub async fn request(&mut self, method: &str, params: Value) -> anyhow::Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let outcome = tokio::time::timeout(Duration::from_secs(30), async {
            let direct = self
                .send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
                .await?;
            let response = if let Some(value) = direct {
                value
            } else {
                let Transport::Stdio { output, .. } = &mut self.transport else {
                    bail!("Missing MCP response");
                };
                let mut found = None;
                for _ in 0..100 {
                    let mut line = Vec::new();
                    let count = output
                        .take(1024 * 1024 + 1)
                        .read_until(b'\n', &mut line)
                        .await?;
                    if count == 0 {
                        bail!("MCP server disconnected");
                    }
                    if line.len() > 1024 * 1024 {
                        bail!("MCP response exceeds 1 MiB");
                    }
                    let value: Value = serde_json::from_slice(&line)?;
                    if value.get("id") == Some(&json!(id)) {
                        found = Some(value);
                        break;
                    }
                    if value.get("method").is_some() && value.get("id").is_some() {
                        bail!("Server-initiated MCP requests are unsupported");
                    }
                }
                found.context("Too many MCP notifications")?
            };
            if response.get("id") != Some(&json!(id)) {
                bail!("Mismatched MCP response id");
            }
            if response.get("error").is_some() {
                bail!("MCP request failed");
            }
            response
                .get("result")
                .cloned()
                .context("Missing MCP result")
        })
        .await;
        match outcome {
            Ok(result) => result,
            Err(_) => {
                if let Transport::Stdio { child, .. } = &mut self.transport {
                    let _ = child.kill().await;
                }
                bail!("MCP request timed out");
            }
        }
    }
    pub async fn tools(&mut self) -> anyhow::Result<Vec<Value>> {
        let mut tools = vec![];
        let mut cursor = None;
        for _ in 0..20 {
            let page = self
                .request(
                    "tools/list",
                    cursor
                        .as_ref()
                        .map(|c| json!({"cursor":c}))
                        .unwrap_or(json!({})),
                )
                .await?;
            tools.extend(
                page["tools"]
                    .as_array()
                    .context("Missing MCP tools")?
                    .iter()
                    .cloned(),
            );
            if tools.len() > 500 {
                bail!("MCP tool list exceeds limit");
            }
            cursor = page["nextCursor"].as_str().map(str::to_owned);
            if cursor.is_none() {
                return Ok(tools);
            }
        }
        bail!("MCP tool pagination exceeds limit")
    }
}
struct McpTool {
    name: String,
    remote: String,
    description: String,
    schema: Value,
    connection: Arc<tokio::sync::Mutex<Connection>>,
    approvals: Arc<dyn ApprovalHook>,
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
                tool: self.name.clone(),
                summary: format!("Call MCP tool {}", self.remote),
                risk: RiskLevel::Network,
            },
        )?;
        let result = self
            .connection
            .lock()
            .await
            .request("tools/call", json!({"name":self.remote,"arguments":args}))
            .await
            .map_err(|e| ToolCallError::RuntimeError(e.to_string().into()))?;
        if result["isError"] == true {
            return Err(ToolCallError::RuntimeError(
                "MCP tool reported an error".into(),
            ));
        }
        Ok(result)
    }
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
pub async fn tools(
    name: &str,
    server: &McpServer,
    root: &Path,
    approvals: Arc<dyn ApprovalHook>,
) -> anyhow::Result<Vec<Box<dyn ToolT>>> {
    let action = ToolAction {
        tool: format!("mcp_start_{name}"),
        summary: format!(
            "Connect MCP server: {} {:?}",
            server
                .command
                .as_ref()
                .or(server.url.as_ref())
                .context("Missing MCP transport")?,
            server.args
        ),
        risk: if server.command.is_some() {
            RiskLevel::Execute
        } else {
            RiskLevel::Network
        },
    };
    if approvals.approve(&action) == crate::tools::Approval::Deny {
        bail!("MCP startup denied");
    }
    let mut connection = Connection::connect(server, root).await?;
    let metadata = connection.tools().await?;
    let connection = Arc::new(tokio::sync::Mutex::new(connection));
    let mut result: Vec<Box<dyn ToolT>> = vec![];
    for tool in metadata {
        let remote = tool["name"].as_str().context("MCP tool has no name")?;
        if !super::safe_name(remote) {
            bail!("Unsupported MCP tool name");
        }
        result.push(Box::new(McpTool {
            name: format!("mcp_{}", tool_identity(name, remote)),
            remote: remote.into(),
            description: tool["description"].as_str().unwrap_or("MCP tool").into(),
            schema: tool["inputSchema"].clone(),
            connection: connection.clone(),
            approvals: approvals.clone(),
        }));
    }
    Ok(result)
}

pub fn tool_identity(namespace: &str, component: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    (namespace, component).hash(&mut hash);
    format!(
        "{:016x}_{}",
        hash.finish(),
        component.chars().take(32).collect::<String>()
    )
}
