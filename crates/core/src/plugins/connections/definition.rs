//! A connection's definition: its fields, validation, secrets and where it may connect.

use super::*;

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
    /// A remote server on the older transport: a stream it keeps open, with messages sent to an
    /// address it names there. See [`SseParser`].
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub sse: bool,
    /// How long one of this server's tools may run, in seconds; see [`tool_call_deadline`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_timeout_secs: Option<u64>,
    #[serde(default)]
    pub enabled: bool,
}

/// Starting a server and listing its tools answer quickly, or something is wrong.
pub(super) const PROTOCOL_DEADLINE: Duration = Duration::from_secs(30);

/// Tools do real work: a browser action, a test run, an audit.
const DEFAULT_TOOL_SECS: u64 = 300;

const MAX_TOOL_SECS: u64 = 3600;

/// How long one tool call may run before the server is given up on.
pub fn tool_call_deadline(tool_timeout_secs: Option<u64>) -> Duration {
    Duration::from_secs(tool_timeout_secs.unwrap_or(DEFAULT_TOOL_SECS))
}

impl McpServer {
    pub fn validate(&self) -> anyhow::Result<()> {
        crate::plugins::definitions::none(self.problems())
    }

    /// Every problem [`Self::validate`] finds: each group of checks reports its first.
    pub fn problems(&self) -> Vec<String> {
        [
            self.validate_launch(),
            self.validate_cwd(),
            self.validate_remote(),
            self.validate_plain_values(),
            self.validate_env(),
            self.validate_headers(),
        ]
        .into_iter()
        .filter_map(Result::err)
        .map(|error| format!("{error:#}"))
        .collect()
    }

    /// How it starts: a command or an address, and its tool time limit.
    fn validate_launch(&self) -> anyhow::Result<()> {
        if self
            .tool_timeout_secs
            .is_some_and(|secs| !(1..=MAX_TOOL_SECS).contains(&secs))
        {
            bail!("MCP tool_timeout_secs must be between 1 and {MAX_TOOL_SECS}");
        }
        if self.sse && self.url.is_none() {
            bail!("The SSE transport needs a URL");
        }
        if self.command.is_some() == self.url.is_some() {
            bail!("Specify exactly one MCP command or URL");
        }
        if self.command.as_ref().is_some_and(|c| c.trim().is_empty()) {
            bail!("MCP command is empty");
        }
        Ok(())
    }

    fn validate_cwd(&self) -> anyhow::Result<()> {
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
        Ok(())
    }

    /// A remote server's address and sign-in.
    fn validate_remote(&self) -> anyhow::Result<()> {
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
        Ok(())
    }

    fn validate_env(&self) -> anyhow::Result<()> {
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
        Ok(())
    }

    fn validate_headers(&self) -> anyhow::Result<()> {
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
            .chain(
                self.oauth
                    .as_ref()
                    .and_then(|oauth| oauth.client_secret_env.as_deref()),
            )
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
    /// Where this server's secrets really go, for the person to allow: the host and any port a
    /// connection would reach, read by the URL parser from the address with its values filled in,
    /// or the local program it runs. Refused ([`UNCHECKABLE_DESTINATION`]) when a secret stands
    /// where the host goes, since connecting would already give it away to name lookup, and when
    /// the address isn't one a connection would use.
    pub fn secret_destination(&self) -> anyhow::Result<String> {
        let Some(url) = &self.url else {
            return Ok(format!(
                "the program {}",
                self.command.as_deref().unwrap_or_default()
            ));
        };
        let refused = || anyhow::anyhow!(UNCHECKABLE_DESTINATION);
        let rest = url.split_once("://").map_or(url.as_str(), |(_, rest)| rest);
        let authority = rest.split(['/', '?', '#', '\\']).next().unwrap_or(rest);
        for name in super::keys::template_names(authority) {
            let value = super::keys::value(name).or_else(|| self.defaults.get(name).cloned());
            if names_a_credential(name)
                || value.is_none_or(|value| !super::keys::safe_literal("", &value))
            {
                return Err(refused());
            }
        }
        let filled = self.filled_url().ok_or_else(refused)?;
        check_url(&filled).map_err(|_| refused())?;
        let parsed = reqwest::Url::parse(&filled).map_err(|_| refused())?;
        let host = parsed
            .host_str()
            .ok_or_else(refused)?
            .trim_end_matches('.')
            .to_ascii_lowercase();
        Ok(match parsed.port() {
            Some(port) => format!("{host}:{port}"),
            None => host,
        })
    }
    /// The server's address with its secrets filled in, as a connection reaches it; `None` for a
    /// local server or while a secret it needs is missing.
    pub fn filled_url(&self) -> Option<String> {
        let url = self.url.as_deref()?;
        let known =
            |name: &&str| super::keys::value(name).is_some() || self.defaults.contains_key(*name);
        super::keys::template_names(url).iter().all(known).then(|| {
            super::keys::fill(url, |name| {
                super::keys::value(name)
                    .or_else(|| self.defaults.get(name).cloned())
                    .unwrap_or_default()
            })
        })
    }
}

/// Why a connection's secrets have nowhere to be allowed to.
pub const UNCHECKABLE_DESTINATION: &str = "This connection's address reads a secret where its host goes, or can't be read, so Themis won't connect it.";

/// Whether a name says it holds a credential, in the words that don't also name places
/// (`AUTH0_DOMAIN`, `SESSION_HOST`).
fn names_a_credential(name: &str) -> bool {
    let name = name.to_ascii_uppercase();
    [
        "KEY",
        "TOKEN",
        "SECRET",
        "PASSWORD",
        "PASSWD",
        "PWD",
        "CREDENTIAL",
        "COOKIE",
        "PRIVATE",
    ]
    .iter()
    .any(|word| name.contains(word))
}

/// A remote MCP address: HTTPS (or HTTP to this computer) with no user name, password or
/// fragment. A query is allowed when every parameter is plain and none names a credential.
pub(super) fn check_url(url: &str) -> anyhow::Result<()> {
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
