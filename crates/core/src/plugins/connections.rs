//! MCP JSON-RPC transport with bounded calls and approval-gated agent tools.
use crate::tools::{ApprovalHook, CommandDisplay, RiskLevel, ToolAction, ToolRuntime, ToolT};
use anyhow::{bail, Context};
use autoagents::async_trait;
use autoagents::core::tool::ToolCallError;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{collections::BTreeMap, path::Path, sync::Arc, time::Duration};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

#[derive(Debug)]
struct RequestRejected;
impl std::fmt::Display for RequestRejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MCP request failed")
    }
}
impl std::error::Error for RequestRejected {}

/// How a remote server signs in, as an imported `oauth` block says: a client someone registered
/// with its provider, the fixed address it was registered with, and what to ask for. Never a
/// credential: `client_secret_env` names a secret.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct McpOauth {
    /// A preregistered client (or a client metadata document's address); absent, Themis registers
    /// a client of its own if the provider lets it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    /// The port the local listener takes; absent, any free one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub callback_port: Option<u16>,
    /// Where the authorization server's metadata is, when discovery wouldn't find it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_server_metadata_url: Option<String>,
    /// Space-separated; absent, what the server lists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scopes: Option<String>,
    /// The secret holding the client's secret, for a client that has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret_env: Option<String>,
}

impl McpOauth {
    fn validate(&self) -> anyhow::Result<()> {
        let plain = |text: &str, limit: usize| {
            !text.is_empty()
                && text.len() <= limit
                && text.bytes().all(|b| (b' '..=b'~').contains(&b))
        };
        if self.client_id.as_deref().is_some_and(|id| !plain(id, 512)) {
            bail!("MCP oauth clientId is not valid");
        }
        if self
            .scopes
            .as_deref()
            .is_some_and(|scopes| !plain(scopes, 2048))
        {
            bail!("MCP oauth scopes are not valid");
        }
        // Below 1024 a listener needs privileges a sign-in never has.
        if self.callback_port.is_some_and(|port| port < 1024) {
            bail!("MCP oauth callbackPort must be 1024 or above");
        }
        if let Some(url) = &self.auth_server_metadata_url {
            check_url(url).context("MCP oauth authServerMetadataUrl is not valid")?;
        }
        if self
            .client_secret_env
            .as_deref()
            .is_some_and(|name| !super::keys::valid_name(name))
        {
            bail!("MCP oauth clientSecret must name a secret");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
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
    /// Extra HTTP headers, each `prefix${NAME}` (such as `Token ${API_KEY}`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    /// Plain, non-secret environment values (see `keys::safe_literal`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env_values: BTreeMap<String, String>,
    /// Plain, non-secret HTTP headers (see `keys::safe_literal`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub header_values: BTreeMap<String, String>,
    /// Values for keys that are not set (`${NAME:-default}`), never credentials.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub defaults: BTreeMap<String, String>,
    /// Where a local server runs: relative to the project, or `${CLAUDE_PLUGIN_ROOT}/…`. Filled in
    /// and held inside the project when it starts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// How a remote server signs in; see [`McpOauth`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oauth: Option<McpOauth>,
    #[serde(default)]
    pub enabled: bool,
}
impl McpServer {
    pub fn validate(&self) -> anyhow::Result<()> {
        if let Some(cwd) = &self.cwd {
            let inside = cwd
                .strip_prefix("${CLAUDE_PLUGIN_ROOT}")
                .or_else(|| cwd.strip_prefix("${PLUGIN_ROOT}"))
                .map_or(cwd.as_str(), |rest| rest.trim_start_matches('/'));
            let safe = inside.is_empty() || inside == "." || super::safe_relative(inside);
            if !safe || self.command.is_none() {
                bail!("MCP cwd must be a folder inside the project, for a local server");
            }
        }
        if self.command.is_some() == self.url.is_some() {
            bail!("Specify exactly one MCP command or URL");
        }
        if self.command.as_ref().is_some_and(|c| c.trim().is_empty()) {
            bail!("MCP command is empty");
        }
        if let Some(url) = &self.url {
            let names = super::keys::template_names(url);
            if names.iter().any(|name| !super::keys::valid_name(name)) {
                bail!("MCP URL references must be ${{NAME}}");
            }
            // A URL that starts with a key (`${BASE}/mcp`) is checked once the key is known.
            if !url.starts_with("${") {
                check_url(&super::keys::fill(url, |_| "key".into()))?;
            }
        }
        if let Some(oauth) = &self.oauth {
            if self.url.is_none() {
                bail!("Only remote MCP servers sign in");
            }
            oauth.validate()?;
        }
        self.validate_plain_values()?;
        if !self.header_values.is_empty() && self.url.is_none() {
            bail!("Only remote MCP servers take headers");
        }
        if self
            .env
            .keys()
            .chain(self.env.values())
            .chain(self.bearer_env.iter())
            .any(|v| !v.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') || v.is_empty())
        {
            bail!("MCP environment values must name environment variables");
        }
        if !self.headers.is_empty() && self.url.is_none() {
            bail!("Only remote MCP servers take headers");
        }
        for (name, template) in self.headers.iter().chain(&self.header_values) {
            let literal = self.header_values.contains_key(name);
            if literal && self.headers.contains_key(name) {
                bail!("Duplicate MCP header '{name}'");
            }
            const RESERVED: [&str; 7] = [
                "accept",
                "content-type",
                "content-length",
                "host",
                "mcp-session-id",
                "mcp-protocol-version",
                "authorization",
            ];
            let lower = name.to_ascii_lowercase();
            if name.is_empty()
                || !name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
                || (RESERVED.contains(&lower.as_str())
                    && !(lower == "authorization" && self.bearer_env.is_none()))
            {
                bail!("Unsupported MCP header '{name}'");
            }
            if !literal {
                super::keys::header_template(template)?;
            }
        }
        Ok(())
    }
    /// Plain values and defaults are never credentials, and plain env names are unique.
    fn validate_plain_values(&self) -> anyhow::Result<()> {
        for (name, value) in self.env_values.iter().chain(&self.header_values) {
            if !super::keys::safe_plugin_value(name, value) {
                bail!("MCP value for '{name}' must reference a key as ${{NAME}}; literal values are unsupported");
            }
        }
        for (name, value) in &self.defaults {
            if !super::keys::valid_name(name)
                || !(value.is_empty() || super::keys::safe_literal(name, value))
            {
                bail!("MCP default for '{name}' must be a plain, non-secret value");
            }
        }
        if self.env_values.keys().any(|name| {
            self.env.contains_key(name)
                || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        }) {
            bail!("MCP environment names must be unique variable names");
        }
        Ok(())
    }
    /// The key names this server reads, in order, without duplicates.
    pub fn key_names(&self) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        let headers = self
            .headers
            .values()
            .filter_map(|t| super::keys::header_template(t).ok().map(|(_, n)| n));
        let url = self
            .url
            .as_deref()
            .map(super::keys::template_names)
            .unwrap_or_default();
        for name in url
            .into_iter()
            .chain(self.env.values().map(String::as_str))
            .chain(self.bearer_env.as_deref())
            .chain(headers)
        {
            if !names.iter().any(|known| known == name) {
                names.push(name.to_owned());
            }
        }
        names
    }
    /// Whether the server brings its own credentials (else a remote one may sign in). A header
    /// written `${NAME:-}` only counts once it has a value: such a plugin takes a key or a sign-in.
    pub fn has_credentials(&self) -> bool {
        let filled = |template: &String| {
            let Ok((_, name)) = super::keys::header_template(template) else {
                return true;
            };
            self.defaults
                .get(name)
                .is_none_or(|default| !default.is_empty())
                || super::keys::value(name).is_some()
        };
        self.bearer_env.is_some() || self.headers.values().any(filled)
    }
    /// The server's address with its secrets filled in, as a connection reaches it; `None` for a
    /// local server or while a secret it needs is missing.
    pub fn filled_url(&self) -> Option<String> {
        let url = self.url.as_deref()?;
        let known =
            |name: &&str| super::keys::value(name).is_some() || self.defaults.contains_key(*name);
        super::keys::template_names(url)
            .iter()
            .all(|name| known(name))
            .then(|| {
                super::keys::fill(url, |name| {
                    super::keys::value(name)
                        .or_else(|| self.defaults.get(name).cloned())
                        .unwrap_or_default()
                })
            })
    }
}
/// A remote MCP address: HTTPS (or HTTP to this computer) with no user name, password or
/// fragment. A query is allowed when every parameter is plain and none names a credential.
fn check_url(url: &str) -> anyhow::Result<()> {
    let u = reqwest::Url::parse(url)?;
    let query_ok = u
        .query_pairs()
        .all(|(name, value)| super::keys::safe_literal(&name, &value));
    if !(u.scheme() == "https"
        || (u.scheme() == "http"
            && matches!(u.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))))
        || !u.username().is_empty()
        || u.password().is_some()
        || !query_ok
        || u.fragment().is_some()
    {
        bail!("Use HTTPS (or localhost HTTP) without embedded credentials");
    }
    Ok(())
}
enum Transport {
    Stdio {
        child: Box<tokio::process::Child>,
        _group: super::hooks::ProcessGroup,
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
}
pub struct Connection {
    transport: Transport,
    next_id: u64,
    protocol: Option<String>,
    instructions: Option<String>,
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
            json!({"duration_ms":started.elapsed().as_millis(),"transport":if server.command.is_some(){"stdio"}else{"http"},"protocol":result.as_ref().ok().and_then(|c|c.protocol.clone()),"error_category":result.as_ref().err().map(connection_error_category)}),
        );
        result
    }
    async fn connect_inner(server: &McpServer, root: &Path) -> anyhow::Result<Self> {
        server.validate()?;
        crate::diagnostics::emit(
            "mcp",
            "connection_started",
            "info",
            json!({"transport":if server.command.is_some(){"stdio"}else{"http"}}),
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
                _group: super::hooks::ProcessGroup(child.id()),
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
            Transport::Http {
                client: reqwest::Client::builder()
                    .timeout(Duration::from_secs(30))
                    .redirect(reqwest::redirect::Policy::none())
                    .build()?,
                url,
                session: None,
                bearer,
                headers,
            }
        };
        let mut result = Self {
            transport,
            next_id: 1,
            protocol: None,
            instructions: None,
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
                    crate::diagnostics::emit(
                        "mcp",
                        "http_failed",
                        "warn",
                        json!({"http_status":response.status().as_u16(),"error_category":if matches!(response.status().as_u16(),401|403){"authentication"}else{"remote_http"}}),
                    );
                    // A Bearer challenge on 401 is MCP's way to ask for an OAuth sign-in.
                    let challenge = response
                        .headers()
                        .get("www-authenticate")
                        .and_then(|value| value.to_str().ok())
                        .is_some_and(|value| value.to_ascii_lowercase().starts_with("bearer"));
                    if response.status() == reqwest::StatusCode::UNAUTHORIZED && challenge {
                        bail!("MCP server needs sign-in");
                    }
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
                if let Transport::Stdio { child, .. } = &mut self.transport {
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
struct McpTool {
    name: String,
    remote: String,
    description: String,
    schema: Value,
    connection: Arc<tokio::sync::Mutex<Connection>>,
    healthy: Arc<std::sync::atomic::AtomicBool>,
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
                display: None,
                command: Vec::new(),
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
            .map_err(|e| {
                if !e.is::<RequestRejected>() {
                    self.healthy
                        .store(false, std::sync::atomic::Ordering::Relaxed);
                }
                ToolCallError::RuntimeError(e.to_string().into())
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

/// A reusable transport; tool approval hooks remain specific to each run.
pub struct Session {
    connection: Arc<tokio::sync::Mutex<Connection>>,
    metadata: Vec<Value>,
    instructions: Option<String>,
    healthy: Arc<std::sync::atomic::AtomicBool>,
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
        if !self.is_connected() {
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

/// Generated tool name tail: `<16 hex server id><8 hex tool id>_<tool>`. The
/// server id depends only on `namespace`, so a chat grant can cover every tool
/// of one server (see [`crate::tools::mcp_server_id`]); the tool id keeps
/// names unique when long tool names are truncated.
pub fn tool_identity(namespace: &str, component: &str) -> String {
    use sha2::{Digest, Sha256};
    let server = Sha256::digest(namespace.as_bytes());
    let tool = Sha256::digest([namespace.as_bytes(), b"\0", component.as_bytes()].concat());
    format!(
        "{}{}_{}",
        server[..8]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>(),
        tool[..4]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>(),
        component.chars().take(32).collect::<String>()
    )
}
