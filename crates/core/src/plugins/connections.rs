//! MCP JSON-RPC transport with bounded calls and approval-gated agent tools.
use crate::tools::{ApprovalHook, CommandDisplay, RiskLevel, ToolAction, ToolRuntime, ToolT};
use anyhow::{bail, Context};
use autoagents::async_trait;
use autoagents::core::tool::ToolCallError;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

mod definition;
mod sse;
mod tool;

// The children name these as `super::keys` and `super::safe_relative`, as this file did.
use super::{keys, safe_relative};
use definition::{check_url, PROTOCOL_DEADLINE};
pub use definition::{tool_call_deadline, McpOauth, McpServer, UNCHECKABLE_DESTINATION};
#[cfg(test)]
use sse::SSE_EVENT_BYTES;
use sse::{http_failure, open_sse, AbortOnDrop};
pub use sse::{sse_endpoint, SseEvent, SseParser};
use tool::McpTool;
pub use tool::{authorize_start, startup_action, startup_denied, tools, McpTools};

#[derive(Debug)]
struct RequestRejected;
impl std::fmt::Display for RequestRejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MCP request failed")
    }
}
impl std::error::Error for RequestRejected {}

enum Transport {
    Stdio {
        child: Box<tokio::process::Child>,
        group: super::hooks::ProcessGroup,
        input: tokio::process::ChildStdin,
        output: BufReader<tokio::process::ChildStdout>,
    },
    Http {
        client: reqwest::Client,
        url: String,
        session: Option<String>,
        bearer: Option<String>,
        headers: Vec<(String, String)>,
    },
    Sse {
        client: reqwest::Client,
        /// Where messages go: the address the server named, held to its own host and port.
        endpoint: String,
        bearer: Option<String>,
        headers: Vec<(String, String)>,
        /// What arrives on the stream, until it ends.
        incoming: tokio::sync::mpsc::Receiver<Value>,
        _stream: AbortOnDrop,
    },
}

/// How a server is reached, for the diagnostics.
fn transport_name(server: &McpServer) -> &'static str {
    match (server.command.is_some(), server.sse) {
        (true, _) => "stdio",
        (false, true) => "sse",
        (false, false) => "http",
    }
}

pub struct Connection {
    transport: Transport,
    next_id: u64,
    protocol: Option<String>,
    instructions: Option<String>,
    tool_deadline: Duration,
}
impl std::fmt::Debug for Connection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MCP connection")
    }
}
impl Connection {
    pub async fn connect(server: &McpServer, root: &Path) -> anyhow::Result<Self> {
        let started = std::time::Instant::now();
        let result = Self::connect_inner(server, root).await;
        crate::diagnostics::emit(
            "mcp",
            if result.is_ok() {
                "initialized"
            } else {
                "initialization_failed"
            },
            if result.is_ok() { "info" } else { "warn" },
            json!({"duration_ms":started.elapsed().as_millis(),"transport":transport_name(server),"protocol":result.as_ref().ok().and_then(|c|c.protocol.clone()),"error_category":result.as_ref().err().map(connection_error_category)}),
        );
        result
    }
    async fn connect_inner(server: &McpServer, root: &Path) -> anyhow::Result<Self> {
        server.validate()?;
        crate::diagnostics::emit(
            "mcp",
            "connection_started",
            "info",
            json!({"transport":transport_name(server)}),
        );
        // Every key the server reads, resolved up front so all missing ones are named at once.
        let mut keys = BTreeMap::new();
        let mut missing = Vec::new();
        for name in server.key_names() {
            match super::keys::value(&name).or_else(|| server.defaults.get(&name).cloned()) {
                Some(value) => {
                    keys.insert(name, value);
                }
                None => missing.push(name),
            }
        }
        if !missing.is_empty() {
            return Err(super::keys::missing_error(&missing));
        }
        let transport = if let Some(command) = &server.command {
            let mut c = tokio::process::Command::new(command);
            let folder = match &server.cwd {
                Some(cwd) => {
                    let folder = root
                        .join(cwd)
                        .canonicalize()
                        .context("MCP cwd is missing")?;
                    if !folder.starts_with(root.canonicalize()?) {
                        bail!("MCP cwd is outside the project");
                    }
                    folder
                }
                None => root.to_path_buf(),
            };
            c.args(&server.args)
                .current_dir(folder)
                .env_clear()
                .kill_on_drop(true)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped());
            for name in ["PATH", "HOME", "SYSTEMROOT", "TMPDIR"] {
                if let Some(value) = std::env::var_os(name) {
                    c.env(name, value);
                }
            }
            for (name, source) in &server.env {
                c.env(name, &keys[source]);
            }
            for (name, value) in &server.env_values {
                c.env(name, value);
            }
            #[cfg(unix)]
            c.process_group(0);
            let mut child = match c.spawn() {
                Ok(child) => child,
                // Name the program: "npx" missing reads very differently from a crash.
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    anyhow::bail!(
                        "MCP program not found: {}",
                        server.command.as_deref().unwrap_or_default()
                    )
                }
                Err(error) => return Err(error).context("Could not start MCP server"),
            };
            if let Some(stderr) = child.stderr.take() {
                crate::diagnostics::capture_stderr(stderr, "mcp", keys.values().cloned().collect());
            }
            let input = child.stdin.take().context("Missing stdin")?;
            let output = BufReader::new(child.stdout.take().context("Missing stdout")?);
            Transport::Stdio {
                group: super::hooks::ProcessGroup(child.id()),
                child: Box::new(child),
                input,
                output,
            }
        } else {
            let url = super::keys::fill(server.url.as_deref().context("Missing URL")?, |name| {
                keys[name].clone()
            });
            check_url(&url)?;
            let bearer = match &server.bearer_env {
                Some(name) => Some(keys[name].clone()),
                // A sign-in made in Integrations (OAuth), refreshed when it expires.
                None if !server.has_credentials() => super::oauth::access_token(&url).await,
                None => None,
            };
            let headers = server
                .headers
                .iter()
                // `${NAME:-}` left empty: the header isn't sent at all.
                .filter(|(_, template)| {
                    super::keys::header_template(template)
                        .map_or(true, |(_, name)| !keys[name].is_empty())
                })
                .map(|(header, template)| {
                    let (prefix, name) = super::keys::header_template(template)?;
                    Ok((header.clone(), format!("{prefix}{}", keys[name])))
                })
                .chain(
                    server
                        .header_values
                        .iter()
                        .map(|(header, value)| Ok((header.clone(), value.clone()))),
                )
                .collect::<anyhow::Result<Vec<_>>>()?;
            if server.sse {
                let (client, endpoint, incoming, stream) =
                    open_sse(&url, bearer.as_deref(), &headers).await?;
                Transport::Sse {
                    client,
                    endpoint,
                    bearer,
                    headers,
                    incoming,
                    _stream: stream,
                }
            } else {
                Transport::Http {
                    client: reqwest::Client::builder()
                        .connect_timeout(PROTOCOL_DEADLINE)
                        .redirect(reqwest::redirect::Policy::none())
                        .build()?,
                    url,
                    session: None,
                    bearer,
                    headers,
                }
            }
        };
        let mut result = Self {
            transport,
            next_id: 1,
            protocol: None,
            instructions: None,
            tool_deadline: tool_call_deadline(server.tool_timeout_secs),
        };
        let init=result.request("initialize",json!({"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"Themis","version":env!("CARGO_PKG_VERSION")}})).await?;
        if !matches!(
            init["protocolVersion"].as_str(),
            Some("2024-11-05" | "2025-03-26" | "2025-06-18")
        ) {
            bail!("Unsupported MCP protocol version");
        }
        result.protocol = init["protocolVersion"].as_str().map(str::to_owned);
        if let Some(instructions) = init.get("instructions") {
            let instructions = instructions
                .as_str()
                .context("MCP instructions must be a string")?;
            if instructions.len() > 32768 {
                bail!("MCP instructions exceed 32768 bytes");
            }
            result.instructions = Some(instructions.to_owned());
        }
        result
            .send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
            .await?;
        Ok(result)
    }
    async fn send(&mut self, message: Value) -> anyhow::Result<Option<Value>> {
        let deadline = self.deadline(message["method"].as_str().unwrap_or_default());
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
                headers,
            } => {
                let mut request = client
                    .post(url.as_str())
                    // A backstop for messages sent outside `request`, whose deadline ends first.
                    .timeout(deadline + Duration::from_secs(5))
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
                for (name, value) in headers.iter() {
                    request = request.header(name.as_str(), value.as_str());
                }
                let mut response = request.send().await.context("MCP connection failed")?;
                if !response.status().is_success() {
                    return Err(http_failure(&response));
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
            Transport::Sse {
                client,
                endpoint,
                bearer,
                headers,
                ..
            } => {
                let mut request = client
                    .post(endpoint.as_str())
                    .timeout(Duration::from_secs(30))
                    .json(&message);
                if let Some(token) = bearer {
                    request = request.bearer_auth(token.as_str());
                }
                for (name, value) in headers.iter() {
                    request = request.header(name.as_str(), value.as_str());
                }
                let response = request.send().await.context("MCP connection failed")?;
                if !response.status().is_success() {
                    return Err(http_failure(&response));
                }
                // The answer comes on the stream.
                Ok(None)
            }
        }
    }

    /// The answer to request `id`, from what a stream-based transport has received.
    async fn next_answer(&mut self, id: u64) -> anyhow::Result<Value> {
        match &mut self.transport {
            Transport::Stdio { output, .. } => {
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
                found.context("Too many MCP notifications")
            }
            Transport::Sse { incoming, .. } => {
                for _ in 0..100 {
                    let value = incoming.recv().await.context("MCP server disconnected")?;
                    if value.get("id") == Some(&json!(id)) && value.get("method").is_none() {
                        return Ok(value);
                    }
                    if value.get("method").is_some() && value.get("id").is_some() {
                        bail!("Server-initiated MCP requests are unsupported");
                    }
                }
                bail!("Too many MCP notifications")
            }
            Transport::Http { .. } => bail!("Missing MCP response"),
        }
    }
    /// Whether a local server's process has ended.
    fn stopped(&mut self) -> bool {
        match &mut self.transport {
            Transport::Stdio { child, .. } => !matches!(child.try_wait(), Ok(None)),
            _ => false,
        }
    }
    fn deadline(&self, method: &str) -> Duration {
        if method == "tools/call" {
            self.tool_deadline
        } else {
            PROTOCOL_DEADLINE
        }
    }
    pub async fn request(&mut self, method: &str, params: Value) -> anyhow::Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let deadline = self.deadline(method);
        let outcome = tokio::time::timeout(deadline, async {
            let direct = self
                .send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
                .await?;
            let response = if let Some(value) = direct {
                value
            } else {
                self.next_answer(id).await?
            };
            if response.get("id") != Some(&json!(id)) {
                bail!("Mismatched MCP response id");
            }
            if response.get("error").is_some() {
                return Err(RequestRejected.into());
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
                if let Transport::Stdio { child, group, .. } = &mut self.transport {
                    // Everything the server started goes with it, such as a browser.
                    *group = super::hooks::ProcessGroup(None);
                    let _ = child.kill().await;
                }
                bail!("MCP request timed out");
            }
        }
    }
    pub async fn tools(&mut self) -> anyhow::Result<Vec<Value>> {
        let started = std::time::Instant::now();
        let result = self.tools_inner().await;
        crate::diagnostics::emit(
            "mcp",
            if result.is_ok() {
                "tools_discovered"
            } else {
                "discovery_failed"
            },
            if result.is_ok() { "info" } else { "warn" },
            json!({"duration_ms":started.elapsed().as_millis(),"tool_count":result.as_ref().map_or(0,Vec::len),"error_category":result.as_ref().err().map(connection_error_category)}),
        );
        result
    }
    async fn tools_inner(&mut self) -> anyhow::Result<Vec<Value>> {
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

/// A reusable transport; tool approval hooks remain specific to each run.
pub struct Session {
    connection: Arc<tokio::sync::Mutex<Connection>>,
    metadata: Vec<Value>,
    instructions: Option<String>,
    healthy: Arc<std::sync::atomic::AtomicBool>,
    restart: Option<Arc<(McpServer, PathBuf)>>,
}
impl Session {
    pub async fn connect(server: &McpServer, root: &Path) -> anyhow::Result<Self> {
        crate::diagnostics::emit("mcp", "setup_started", "info", json!({}));
        let mut connection = Connection::connect(server, root).await.inspect_err(|error| {
            crate::diagnostics::emit("mcp", "setup_failed", "warn", json!({"stage":"initialization","error_category":connection_error_category(error)}));
        })?;
        let metadata = connection.tools().await.inspect_err(|error| {
            crate::diagnostics::emit(
                "mcp",
                "setup_failed",
                "warn",
                json!({"stage":"tool_discovery","error_category":connection_error_category(error)}),
            );
        })?;
        for tool in &metadata {
            if !tool["name"].as_str().is_some_and(super::safe_name) {
                bail!("Unsupported MCP tool name");
            }
        }
        crate::diagnostics::emit(
            "mcp",
            "setup_completed",
            "info",
            json!({"tool_count":metadata.len()}),
        );
        Ok(Self {
            instructions: connection.instructions.clone(),
            connection: Arc::new(tokio::sync::Mutex::new(connection)),
            metadata,
            healthy: Arc::new(std::sync::atomic::AtomicBool::new(true)),
            restart: server
                .command
                .is_some()
                .then(|| Arc::new((server.clone(), root.to_owned()))),
        })
    }
    pub fn tool_summaries(&self) -> Vec<Value> {
        self.metadata.iter().map(|tool| json!({"name":tool["name"], "description":tool["description"].as_str().unwrap_or("")})).collect()
    }
    pub fn is_connected(&self) -> bool {
        if let Ok(mut connection) = self.connection.try_lock() {
            if let Transport::Stdio { child, .. } = &mut connection.transport {
                if !matches!(child.try_wait(), Ok(None)) {
                    self.healthy
                        .store(false, std::sync::atomic::Ordering::Relaxed);
                }
            }
        }
        self.healthy.load(std::sync::atomic::Ordering::Relaxed)
    }
    /// Caller must authorize startup before obtaining the session. Calls still
    /// consult the supplied run's approval hook individually.
    pub fn tools(&self, name: &str, approvals: Arc<dyn ApprovalHook>) -> anyhow::Result<McpTools> {
        if self.restart.is_none() && !self.is_connected() {
            bail!("MCP connection closed; reconnect it from Integrations");
        }
        let wire = |remote: &str| format!("mcp_{}", tool_identity(name, remote));
        let names = self
            .metadata
            .iter()
            .map(|tool| {
                let remote = tool["name"].as_str().expect("validated tool name");
                (wire(remote), remote.to_owned())
            })
            .collect();
        let tools = self
            .metadata
            .iter()
            .map(|tool| {
                let remote = tool["name"].as_str().expect("validated tool name");
                Box::new(McpTool {
                    name: wire(remote),
                    remote: remote.into(),
                    description: tool["description"].as_str().unwrap_or("MCP tool").into(),
                    schema: tool["inputSchema"].clone(),
                    connection: self.connection.clone(),
                    healthy: self.healthy.clone(),
                    restart: self.restart.clone(),
                    approvals: approvals.clone(),
                }) as Box<dyn ToolT>
            })
            .collect();
        Ok((tools, self.instructions.clone(), names))
    }
}

fn connection_error_category(error: &anyhow::Error) -> &'static str {
    if let Some(error) = error.downcast_ref::<reqwest::Error>() {
        if error.is_timeout() {
            return "timeout";
        }
        if error.is_connect() {
            return "transport";
        }
    }
    let message = error.to_string();
    if message.starts_with("MCP server returned HTTP 401")
        || message.starts_with("MCP server returned HTTP 403")
        || message == "MCP server needs sign-in"
        || message.starts_with(super::keys::NEEDS_KEY)
    {
        return "authentication";
    }
    if message.starts_with("MCP server returned HTTP ") {
        return "remote_http";
    }
    match message.as_str() {
        "Could not start MCP server" => "process_start",
        "Required MCP environment variable is unset" | "MCP authentication variable is unset" => {
            "missing_environment"
        }
        "Unsupported MCP protocol version" => "unsupported_protocol",
        "MCP request timed out" => "timeout",
        "Missing MCP tools" | "MCP tool list exceeds limit" => "invalid_tool_list",
        _ => "connection_or_protocol",
    }
}

/// Generated tool name tail: `<server>_<16 hex>_<tool>`, e.g.
/// `playwright_3f2a9c1b4d5e6f70_browser_click`.
/// `<server>` is the server's own name (letters and digits, at most 16) so a model can read
/// and copy it; the 16 hex digits (64 bits) come from the whole namespace (which binds the
/// server's definition), so a changed definition is another server and a chat grant covers
/// every tool of exactly one (see [`crate::tools::mcp_server_id`]); 64 bits keep a crafted
/// plugin from matching a granted server's id. A live run's model kept mistyping the old 24
/// hex digits. The tool name is cut to 26 characters (64 in all).
pub fn tool_identity(namespace: &str, component: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(namespace.as_bytes());
    let label = namespace.split('#').next().unwrap_or_default();
    let label = label.rsplit([':', '/']).next().unwrap_or_default();
    let mut server: String = label
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .take(16)
        .collect();
    if server.is_empty() {
        server.push_str("server");
    }
    format!(
        "{server}_{}_{}",
        digest[..8]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>(),
        component.chars().take(26).collect::<String>()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_secrets_destination_is_where_a_connection_would_really_go() {
        let at = |url: &str, defaults: &[(&str, &str)]| {
            McpServer {
                url: Some(url.to_owned()),
                defaults: defaults
                    .iter()
                    .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
                    .collect(),
                ..McpServer::default()
            }
            .secret_destination()
            .map_err(|error| error.to_string())
        };
        for (url, host) in [
            ("https://evil.example/mcp", "evil.example"),
            ("HTTPS://EVIL.EXAMPLE./mcp", "evil.example"),
            ("https://evil.example:8443/mcp", "evil.example:8443"),
            ("https://evil.example:443/mcp", "evil.example"),
            // As the parser that connects reads it: the backslash ends the host.
            ("https://good.example\\@evil.example/", "good.example"),
            ("http://127.0.0.1:9000/mcp", "127.0.0.1:9000"),
        ] {
            assert_eq!(at(url, &[]).as_deref(), Ok(host), "{url}");
        }
        // Filled in, with what it would be filled with.
        assert_eq!(
            at("https://${HOST}/mcp", &[("HOST", "a.example")]).as_deref(),
            Ok("a.example")
        );
        assert_eq!(
            at("${BASE}/mcp", &[("BASE", "https://B.example.:444")]).as_deref(),
            Ok("b.example:444")
        );
        assert_eq!(
            at(
                "https://connect.${REGION}.example/mcp",
                &[("REGION", "eu-1")]
            )
            .as_deref(),
            Ok("connect.eu-1.example")
        );
        let refused = Err(UNCHECKABLE_DESTINATION.to_owned());
        for (url, defaults) in [
            // A name, in the address, that the connection would resolve on the way.
            (
                "https://${OPENCODE_KEY}.evil.example/",
                &[("OPENCODE_KEY", "abc")][..],
            ),
            (
                "${API_TOKEN}/mcp",
                &[("API_TOKEN", "https://evil.example")][..],
            ),
            (
                "https://${HOST}.evil.example/",
                &[("HOST", "aZ8kQ2mX9pL4vB7nR3tY6wE1")][..],
            ),
            // Hidden behind a user name, or going nowhere a connection would.
            ("https://${X}@evil.example/", &[("X", "good.example")][..]),
            ("https://good.example@evil.example/", &[][..]),
            ("http://evil.example/mcp", &[][..]),
            ("https://${MISSING}/mcp", &[][..]),
            ("not an address", &[][..]),
        ] {
            assert_eq!(at(url, defaults), refused, "{url}");
        }
        let local = McpServer {
            command: Some("run".into()),
            ..McpServer::default()
        };
        assert_eq!(local.secret_destination().unwrap(), "the program run");
    }

    proptest::proptest! {
        #[test]
        /// However a stream's bytes are cut up the events are the same, and nothing a server sends
        /// makes it panic or keep more than the bound.
        #[test]
        fn server_sent_events_read_the_same_however_they_are_cut(
            events in proptest::collection::vec(("[a-z]{0,8}", "[ -~]{0,40}"), 0..8),
            noise in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..200),
            cut in 1usize..16,
        ) {
            let mut text = String::new();
            for (name, data) in &events {
                if !name.is_empty() {
                    text.push_str(&format!("event: {name}\r\n"));
                }
                text.push_str(&format!("data: {data}\n\n"));
            }
            let mut whole = SseParser::default();
            let at_once = whole.feed(text.as_bytes()).unwrap();
            let mut cut_up = SseParser::default();
            let mut pieces = Vec::new();
            for piece in text.as_bytes().chunks(cut) {
                pieces.extend(cut_up.feed(piece).unwrap());
            }
            proptest::prop_assert_eq!(&at_once, &pieces);
            let mut stray = SseParser::default();
            let _ = stray.feed(&noise);
            proptest::prop_assert!(stray.line.len() + stray.data.len() <= SSE_EVENT_BYTES);
        }

        /// A server can name any address it likes: what comes back is on that server, or nothing.
        #[test]
        fn a_message_address_stays_on_its_server_or_is_refused(named in "\\PC{0,60}") {
            if let Ok(target) = sse_endpoint("https://mcp.example.com:8443/sse", &named) {
                let target = reqwest::Url::parse(&target).unwrap();
                proptest::prop_assert_eq!(target.host_str(), Some("mcp.example.com"));
                proptest::prop_assert_eq!(target.port(), Some(8443));
                proptest::prop_assert_eq!(target.scheme(), "https");
                proptest::prop_assert!(target.username().is_empty() && target.password().is_none());
            }
        }

        #[test]
        fn any_address_gives_a_plain_host_or_is_refused(url in "\\PC{0,80}", value in "\\PC{0,40}") {
            let server = McpServer {
                url: Some(url),
                defaults: std::iter::once(("HOST".to_owned(), value)).collect(),
                ..McpServer::default()
            };
            if let Ok(host) = server.secret_destination() {
                proptest::prop_assert!(!host.contains(['@', '/', '\\']), "{}", host);
            }
        }
    }
}
