use super::*;

/// A plugin named `name` holding the connections of an MCP configuration: a standard `.mcp.json`
/// (`{"mcpServers": {...}}`), OpenCode's `{"mcp": {...}}`, or the bare map of servers. Every
/// connection starts switched off; with `allow_partial`, one this cannot convert is listed in
/// `import_issues` instead of failing the whole configuration.
pub fn mcp_spec(name: &str, value: &Value, allow_partial: bool) -> anyhow::Result<PluginSpec> {
    let (servers, opencode) = if let Some(servers) = value.get("mcpServers") {
        if value
            .as_object()
            .is_some_and(|v| v.keys().any(|key| key != "mcpServers"))
        {
            bail!("Only MCP configuration is imported; remove unrelated root settings");
        }
        (servers, false)
    } else if let Some(servers) = value.get("mcp") {
        if value.as_object().is_some_and(|v| {
            v.keys()
                .any(|key| !["mcp", "$schema"].contains(&key.as_str()))
        }) {
            bail!("Only OpenCode MCP configuration is imported; remove unrelated root settings");
        }
        (servers, true)
    } else {
        (value, false)
    };
    let mut spec = PluginSpec {
        package_kind: Some("mcp".into()),
        name: name.into(),
        ..Default::default()
    };
    import_mcp_components(
        servers,
        opencode,
        &mut spec,
        allow_partial,
        &mut LiteralSecrets::refused(),
    )?;
    if spec.mcp.is_empty() && spec.import_issues.is_empty() {
        bail!("MCP configuration has no connections");
    }
    Ok(spec)
}
pub(super) fn import_mcp_components(
    value: &Value,
    opencode: bool,
    spec: &mut PluginSpec,
    allow_partial: bool,
    literals: &mut LiteralSecrets,
) -> anyhow::Result<()> {
    let root = if opencode { "mcp" } else { "mcpServers" };
    for (name, raw) in value.as_object().context("Invalid MCP configuration")? {
        match import_mcp_server(name, raw, opencode, literals) {
            Ok(server) => {
                for (field, note) in [
                    (
                        "timeout",
                        "its timeout is not used; Themis's own timeouts apply",
                    ),
                    (
                        "autoApprove",
                        "its automatic approvals are not used; Themis approvals apply",
                    ),
                ] {
                    if raw.get(field).is_some() {
                        spec.notices.push(format!("MCP connection {name}: {note}"));
                    }
                }
                // The same server is often declared both in plugin.json and in .mcp.json.
                let same = |existing: &McpServer| {
                    serde_json::to_value(existing).ok() == serde_json::to_value(&server).ok()
                };
                if spec.mcp.get(name).is_some_and(same) {
                    continue;
                }
                if spec.mcp.insert(name.clone(), server).is_some() {
                    bail!("Duplicate MCP connection '{name}'");
                }
            }
            Err(error) => {
                if allow_partial {
                    if let Some(issue) = error.downcast_ref::<UnsupportedMcp>() {
                        spec.import_issues.push(ImportComponent {
                            kind: "mcp".into(),
                            name: name.clone(),
                            status: "unsupported".into(),
                            field: Some(format!("{root}.{}", issue.field)),
                            reason: Some(issue.reason.into()),
                            remedy: Some(issue.remedy.into()),
                        });
                        continue;
                    }
                }
                return Err(error.context(format!("{root}.{name}")));
            }
        }
    }
    Ok(())
}
const LITERAL_REFUSED: &str = "Imported credentials/environment values must use ${VARIABLE} or {env:VARIABLE}; literal values are unsupported";

/// One literal value a plugin's configuration held and a secret now stands for.
#[derive(Debug, Clone)]
pub(super) struct ReplacedLiteral {
    /// The field it was written in; `None` when it was a `:-default` inside a reference.
    field: Option<String>,
    literal: String,
    /// What takes its place in the plugin's files.
    replacement: String,
    secret: String,
}

/// What an import does with a literal value in a credential field. For a plugin it becomes a named
/// secret, the person fills that in (Secret needed, Add secret), and the literal is discarded: not
/// kept in the registry, its files, logs or the check. A configuration the person pastes or types
/// is refused instead, since saying so at once is more use than dropping what they just wrote.
#[derive(Debug, Default)]
pub(super) struct LiteralSecrets {
    plugin: Option<String>,
    pub(super) replaced: Vec<ReplacedLiteral>,
    /// Secrets named so far with the literal each stands for, so two values never share one.
    named: BTreeMap<String, String>,
}

impl LiteralSecrets {
    pub(super) fn refused() -> Self {
        Self::default()
    }

    pub(super) fn for_plugin(plugin: &str) -> Self {
        Self {
            plugin: Some(plugin.to_owned()),
            ..Self::default()
        }
    }

    /// The secret for `literal` in `field` of the server `server`.
    fn secret(&mut self, server: &str, field: &str, literal: &str) -> anyhow::Result<String> {
        use crate::plugins::keys::literal_secret_name;
        let Some(plugin) = self.plugin.clone() else {
            bail!(LITERAL_REFUSED);
        };
        let mut name = literal_secret_name(&plugin, field);
        if self.named.get(&name).is_some_and(|known| known != literal) {
            name = literal_secret_name(&plugin, &format!("{server}_{field}"));
        }
        let mut suffix = 2;
        let base = name.clone();
        while self.named.get(&name).is_some_and(|known| known != literal) {
            name = format!("{base}_{suffix}");
            suffix += 1;
        }
        self.named.insert(name.clone(), literal.to_owned());
        self.replaced.push(ReplacedLiteral {
            field: Some(field.to_owned()),
            literal: literal.to_owned(),
            replacement: format!("${{{name}}}"),
            secret: name.clone(),
        });
        Ok(name)
    }

    /// `${name:-literal}` with a default that may not be kept: the reference stays and the
    /// default goes. Refused outside a plugin.
    fn dropped_default(&mut self, name: &str, literal: &str) -> anyhow::Result<()> {
        if self.plugin.is_none() {
            bail!(LITERAL_REFUSED);
        }
        self.replaced.push(ReplacedLiteral {
            field: None,
            literal: format!(":-{literal}"),
            replacement: String::new(),
            secret: name.to_owned(),
        });
        Ok(())
    }
}

/// Whether a literal is worth hunting for through all of a plugin's text: a real value, not a
/// stand-in, and long enough not to be an everyday word.
fn real_literal(replaced: &ReplacedLiteral) -> bool {
    let bare = replaced.literal.trim_start_matches(":-");
    bare.len() >= 8
        && !crate::plugins::keys::placeholder(replaced.field.as_deref().unwrap_or_default(), bare)
}

/// `text`, a JSON configuration, with each replaced value rewritten; `None` when nothing in it is.
fn scrubbed_config(text: &str, replaced: &[ReplacedLiteral]) -> Option<String> {
    fn walk(
        value: &mut Value,
        key: Option<&str>,
        replaced: &[ReplacedLiteral],
        changed: &mut bool,
    ) {
        match value {
            Value::Object(map) => {
                for (name, inner) in map.iter_mut() {
                    walk(inner, Some(name), replaced, changed);
                }
            }
            Value::Array(items) => {
                for inner in items {
                    walk(inner, key, replaced, changed);
                }
            }
            Value::String(text) => {
                for item in replaced {
                    let here = item.field.is_none() || item.field.as_deref() == key;
                    if here && text.contains(&item.literal) {
                        *text = text.replace(&item.literal, &item.replacement);
                        *changed = true;
                    }
                }
            }
            _ => {}
        }
    }
    let mut value: Value = serde_json::from_str(text).ok()?;
    let mut changed = false;
    walk(&mut value, None, replaced, &mut changed);
    changed
        .then(|| serde_json::to_string_pretty(&value).ok())
        .flatten()
}

/// `text` as it may be kept: configuration fields rewritten, and a real value gone from anywhere.
fn scrubbed_text(relative: &str, text: &str, replaced: &[ReplacedLiteral]) -> Option<String> {
    let mut out = relative
        .ends_with(".json")
        .then(|| scrubbed_config(text, replaced))
        .flatten();
    for item in replaced.iter().filter(|item| real_literal(item)) {
        let current = out.as_deref().unwrap_or(text);
        if current.contains(&item.literal) {
            out = Some(current.replace(&item.literal, &item.replacement));
        }
    }
    out
}

/// Takes what a plugin's configuration held out of everything of it that will be kept: the
/// registry's copies, and the package's files (only those of a size that could hold configuration,
/// and text), rewritten in memory. The package's hash then describes what is kept.
pub(super) fn scrub_literals(
    replaced: &[ReplacedLiteral],
    spec: &mut PluginSpec,
    entries: &mut [super::package::Entry],
) -> anyhow::Result<()> {
    if replaced.is_empty() {
        return Ok(());
    }
    for (relative, text) in &mut spec.files {
        if let Some(clean) = scrubbed_text(relative, text, replaced) {
            *text = clean;
        }
    }
    for entry in entries.iter_mut() {
        if entry.bytes > super::package::MAX_INLINE_FILE_BYTES {
            continue;
        }
        let Ok(text) = String::from_utf8(fs::read(&entry.path)?) else {
            continue;
        };
        if let Some(clean) = scrubbed_text(&entry.relative, &text, replaced) {
            entry.bytes = clean.len() as u64;
            entry.content = Some(clean.into_bytes());
        }
    }
    if !entries.is_empty() {
        spec.package = Some(super::package::describe(entries)?);
    }
    // Names only: what was written there is gone, and this says where to put the real value.
    let mut told = std::collections::BTreeSet::new();
    for item in replaced {
        if told.insert(item.secret.clone()) {
            spec.notices.push(format!(
                "A value written in this plugin's configuration is not kept; it reads the secret {} instead (Settings › Secrets)",
                item.secret
            ));
        }
    }
    Ok(())
}

/// A value that must name a key (an OAuth secret, a bearer token): no default allowed.
fn environment_reference(value: &str) -> anyhow::Result<String> {
    match crate::plugins::keys::reference(value) {
        Some((name, None)) => Ok(name),
        Some((_, Some(_))) => bail!("Unsupported environment reference"),
        None if value.contains("${") || value.contains("{env:") => {
            bail!("Unsupported environment reference")
        }
        None => bail!(LITERAL_REFUSED),
    }
}
/// `env` entries: references become keys (with any default), plain non-secret values and
/// plugin-folder paths are kept as written, and anything else is a literal (see [`LiteralSecrets`]).
fn import_env(
    name_of_server: &str,
    server: &mut McpServer,
    env: &Value,
    literals: &mut LiteralSecrets,
) -> anyhow::Result<()> {
    for (key, value) in env
        .as_object()
        .context("MCP environment must be an object")?
    {
        let value = value
            .as_str()
            .context("MCP environment must contain strings")?;
        let names_plugin_root = crate::plugins::keys::PLUGIN_ROOTS
            .iter()
            .any(|root| value.contains(root));
        match crate::plugins::keys::reference(value).filter(|_| !names_plugin_root) {
            Some((name, default)) => {
                add_defaults(
                    server,
                    default.map(|d| (name.clone(), d)).into_iter().collect(),
                    literals,
                )?;
                server.env.insert(key.clone(), name);
            }
            None if crate::plugins::keys::safe_plugin_value(key, value)
                && !(literals.plugin.is_some()
                    && crate::plugins::keys::placeholder(key, value)) =>
            {
                server.env_values.insert(key.clone(), value.to_owned());
            }
            // A template that doesn't read as one reference says nothing to keep or replace.
            None if value.contains("${") || value.contains("{env:") => {
                environment_reference(value)?;
            }
            None => {
                let secret = literals.secret(name_of_server, key, value)?;
                server.env.insert(key.clone(), secret);
            }
        }
    }
    Ok(())
}

/// A literal header value's scheme word and its credential: `Bearer abc` splits, `abc` doesn't.
fn split_scheme(value: &str) -> (&str, &str) {
    ["Bearer ", "Token ", "Basic "]
        .iter()
        .find_map(|scheme| value.strip_prefix(scheme).map(|rest| (*scheme, rest)))
        .filter(|(_, rest)| !rest.trim().is_empty())
        .unwrap_or(("", value))
}

/// Headers: plain non-secret values are kept, `Authorization: Bearer ${NAME}` becomes the
/// bearer key, every other reference must be a `prefix${NAME}` template, and anything else is a
/// literal (see [`LiteralSecrets`]).
fn import_headers(
    name_of_server: &str,
    server: &mut McpServer,
    headers: &Value,
    literals: &mut LiteralSecrets,
) -> anyhow::Result<()> {
    for (key, value) in headers
        .as_object()
        .context("MCP headers must be an object")?
    {
        let value = value
            .as_str()
            .context("MCP header must reference a key as ${NAME}")?;
        if !value.contains("${") && !value.contains("{env:") {
            let stand_in =
                literals.plugin.is_some() && crate::plugins::keys::placeholder(key, value);
            if crate::plugins::keys::safe_literal(key, value) && !stand_in {
                server.header_values.insert(key.clone(), value.to_owned());
                continue;
            }
            if literals.plugin.is_none() {
                crate::plugins::keys::header_template(value)?;
            }
            let (scheme, credential) = split_scheme(value);
            let secret = literals.secret(name_of_server, key, credential)?;
            if key.eq_ignore_ascii_case("authorization")
                && scheme == "Bearer "
                && server.bearer_env.is_none()
            {
                server.bearer_env = Some(secret);
            } else {
                server
                    .headers
                    .insert(key.clone(), format!("{scheme}${{{secret}}}"));
            }
            continue;
        }
        // `{env:NAME}` (OpenCode) and `${env:NAME}` read the same as `${NAME}`.
        let (template, defaults) = crate::plugins::keys::normalize_references(value)
            .context("MCP header values must be an optional prefix followed by one ${NAME}")?;
        add_defaults(server, defaults, literals)?;
        let bearer = template
            .strip_prefix("Bearer ")
            .filter(|token| token.starts_with("${"));
        match bearer {
            Some(token)
                if key.eq_ignore_ascii_case("authorization") && server.bearer_env.is_none() =>
            {
                server.bearer_env = Some(environment_reference(token)?);
            }
            _ => {
                crate::plugins::keys::header_template(&template)?;
                server.headers.insert(key.clone(), template);
            }
        }
    }
    Ok(())
}
/// Defaults from `${NAME:-value}`: plain, non-secret values only.
fn add_defaults(
    server: &mut McpServer,
    defaults: Vec<(String, String)>,
    literals: &mut LiteralSecrets,
) -> anyhow::Result<()> {
    for (name, default) in defaults {
        // `${NAME:-}` says the value is optional; it holds nothing to refuse.
        if !default.is_empty() && !crate::plugins::keys::safe_literal(&name, &default) {
            literals.dropped_default(&name, &default)?;
            continue;
        }
        server.defaults.insert(name, default);
    }
    Ok(())
}
/// Whether `default` for `key` is the whole of the URL's host and can't be one: Datadog writes
/// `https://${DD_MCP_DOMAIN:-not-setup}/`, which means the value is needed, not that a computer
/// called `not-setup` is the server.
fn unusable_host_default(url: &str, key: &str, default: &str) -> bool {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let host = rest.split('/').next().unwrap_or(rest);
    host == format!("${{{key}}}") && !default.contains('.') && default != "localhost"
}
fn import_mcp_server(
    name: &str,
    raw: &Value,
    opencode: bool,
    literals: &mut LiteralSecrets,
) -> anyhow::Result<McpServer> {
    if !safe_name(name) {
        bail!("Invalid MCP connection name");
    }
    let fields = raw
        .as_object()
        .context("MCP connection must be an object")?;
    let allowed: &[&str] = if opencode {
        &[
            "type",
            "command",
            "environment",
            "url",
            "headers",
            "enabled",
            "oauth",
        ]
    } else {
        &[
            "type",
            "command",
            "args",
            "env",
            "url",
            "headers",
            "enabled",
            "bearer_env",
            "cwd",
            "oauth",
            "tool_timeout_secs",
            // Accepted without effect: descriptions and notes, Claude's own switch (imported
            // servers start disabled), its timeout and auto-approval (Themis's apply).
            "description",
            "note",
            "_note",
            "_comment",
            "_meta",
            "disabled",
            "timeout",
            "autoApprove",
        ]
    };
    let mut compatibility_error = None;
    if let Some(field) = fields.keys().find(|key| !allowed.contains(&key.as_str())) {
        compatibility_error = Some(unsupported_mcp(
            format!("{name}.{field}"),
            "Unsupported MCP field",
            "Remove this field or configure the component manually",
        ));
    }
    if let Some(enabled) = raw.get("enabled") {
        if !enabled.is_boolean() {
            bail!("MCP enabled must be a boolean");
        }
    }
    if let Some(kind) = raw.get("type") {
        let kind = kind
            .as_str()
            .context("MCP transport type must be a string")?;
        let supported = if opencode {
            ["local", "remote"].contains(&kind)
        } else {
            // `url` is a remote server named by its address, which speaks the current transport.
            ["stdio", "http", "streamable-http", "sse", "url"].contains(&kind)
        };
        if !supported {
            compatibility_error = Some(unsupported_mcp(
                format!("{name}.type"),
                "Unsupported MCP transport",
                "Use stdio, Streamable HTTP or SSE",
            ));
        }
    }
    let mut server = McpServer {
        sse: !opencode && raw.get("type").and_then(Value::as_str) == Some("sse"),
        ..McpServer::default()
    };
    if let Some(block) = raw.get("oauth") {
        match read_oauth(name, block, opencode, literals) {
            Ok(oauth) => server.oauth = oauth,
            Err(error) if error.is::<UnsupportedMcp>() => compatibility_error = Some(error),
            Err(error) => return Err(error),
        }
    }
    if let Some(cwd) = raw.get("cwd") {
        server.cwd = Some(cwd.as_str().context("MCP cwd must be text")?.to_owned());
    }
    if let Some(secs) = raw.get("tool_timeout_secs") {
        server.tool_timeout_secs = Some(
            secs.as_u64()
                .context("MCP tool_timeout_secs must be a whole number of seconds")?,
        );
    }
    if let Some(command) = raw.get("command") {
        if opencode {
            let command = command
                .as_array()
                .context("OpenCode command must be an array")?;
            server.command = Some(
                command
                    .first()
                    .and_then(Value::as_str)
                    .context("Empty MCP command")?
                    .into(),
            );
            server.args = command[1..]
                .iter()
                .map(|v| {
                    v.as_str()
                        .map(str::to_owned)
                        .context("MCP command arguments must be strings")
                })
                .collect::<anyhow::Result<_>>()?;
        } else {
            server.command = Some(
                command
                    .as_str()
                    .context("MCP command must be a string")?
                    .into(),
            );
        }
    }
    if let Some(args) = raw.get("args") {
        server.args = serde_json::from_value(args.clone()).context("Invalid MCP arguments")?;
    }
    if let Some(url) = raw.get("url") {
        let url = url.as_str().context("MCP URL must be a string")?;
        let (url, defaults) = crate::plugins::keys::normalize_references(url)
            .context("MCP URL references must be ${NAME}")?;
        let defaults = defaults
            .into_iter()
            .filter(|(key, default)| !unusable_host_default(&url, key, default))
            .collect();
        // A plugin that ships an address left empty (Informatica's): each person has their own,
        // and until it is given there is nothing to connect to.
        let url = match literals.plugin.as_deref() {
            Some(plugin) if url.trim().is_empty() => format!(
                "${{{}}}",
                crate::plugins::keys::literal_secret_name(plugin, &format!("{name}_URL"))
            ),
            _ => url,
        };
        server.url = Some(url);
        add_defaults(&mut server, defaults, literals)?;
    }
    if let Some(env) = raw.get(if opencode { "environment" } else { "env" }) {
        import_env(name, &mut server, env, literals)?;
    }
    if let Some(bearer) = raw.get("bearer_env") {
        server.bearer_env = Some(
            bearer
                .as_str()
                .context("Invalid bearer environment reference")?
                .into(),
        );
    }
    if let Some(headers) = raw.get("headers") {
        import_headers(name, &mut server, headers, literals)?;
    }
    if let Some(kind) = raw.get("type").and_then(Value::as_str) {
        if (["local", "stdio"].contains(&kind) && server.command.is_none())
            || (["remote", "http", "streamable-http", "sse", "url"].contains(&kind)
                && server.url.is_none())
        {
            bail!("MCP transport conflicts with connection configuration");
        }
    }
    if server.command.is_some() && server.bearer_env.is_some() {
        bail!("Stdio authentication must use environment references");
    }
    // Validate credentials and connection shape even when a component is unsupported.
    // Partial installation must never turn an unsafe configuration into a saved resource.
    server.validate()?;
    if let Some(error) = compatibility_error {
        return Err(error);
    }
    Ok(server)
}
/// Preset clients a provider registered for Claude (those of the render, mongodb-atlas and slack
/// plugins). Themis doesn't sign in as another app; a provider's own client for Themis would go
/// through like any other.
const CLIENTS_REGISTERED_FOR_CLAUDE: &[&str] = &[
    "claude",
    "https://claude.ai/oauth/claude-code-client-metadata",
    "1601185624273.8899143856786",
];

/// A connection's `oauth` block: how its sign-in goes (Integrations signs in after installing).
/// A bare `true` or `false` and an empty block leave it to discovery. A field Themis would not honour is named
/// rather than quietly dropped, and a client's secret must be a reference.
fn read_oauth(
    name: &str,
    block: &Value,
    opencode: bool,
    literals: &mut LiteralSecrets,
) -> anyhow::Result<Option<McpOauth>> {
    if block.is_boolean() {
        return Ok(None);
    }
    let fields = block.as_object().context("MCP oauth must be an object")?;
    let text = |value: &Value, field: &str| -> anyhow::Result<String> {
        value
            .as_str()
            .map(str::to_owned)
            .with_context(|| format!("MCP oauth {field} must be text"))
    };
    let mut oauth = McpOauth::default();
    for (field, value) in fields {
        match field.as_str() {
            "clientId" => {
                let client = text(value, field)?;
                if CLIENTS_REGISTERED_FOR_CLAUDE.contains(&client.as_str()) {
                    return Err(unsupported_mcp(
                        format!("{name}.oauth.{field}"),
                        "This sign-in is set up for Claude, so its provider would see Themis as Claude",
                        "Register a client with the provider and use its clientId",
                    ));
                }
                oauth.client_id = Some(client);
            }
            "clientSecret" => {
                let written = text(value, field)?;
                let reference = environment_reference(&written);
                oauth.client_secret_env = Some(match reference {
                    Ok(reference) => reference,
                    Err(_) if !written.contains("${") && !written.contains("{env:") => {
                        literals.secret(name, "clientSecret", &written)?
                    }
                    Err(error) => return Err(error),
                });
            }
            "scopes" | "scope" => oauth.scopes = Some(read_scopes(value)?),
            "callbackPort" if !opencode => {
                oauth.callback_port = Some(
                    value
                        .as_u64()
                        .and_then(|port| u16::try_from(port).ok())
                        .context("MCP oauth callbackPort must be a port number")?,
                );
            }
            "authServerMetadataUrl" if !opencode => {
                oauth.auth_server_metadata_url = Some(text(value, field)?);
            }
            other => {
                return Err(unsupported_mcp(
                    format!("{name}.oauth.{other}"),
                    "Unsupported MCP oauth field",
                    "Remove this field or configure the component manually",
                ));
            }
        }
    }
    Ok((oauth != McpOauth::default()).then_some(oauth))
}

/// Scopes as one space-separated string, as written either way.
fn read_scopes(value: &Value) -> anyhow::Result<String> {
    if let Some(text) = value.as_str() {
        return Ok(text.to_owned());
    }
    let words = value
        .as_array()
        .and_then(|words| words.iter().map(Value::as_str).collect::<Option<Vec<_>>>())
        .context("MCP oauth scopes must be text or a list of text")?;
    Ok(words.join(" "))
}

/// One connection from the standard `mcpServers` shape used by Claude Desktop, Claude Code
/// and Cursor: `{command, args, env}` for local servers, `{type: "http", url, headers}` for
/// remote ones. Credentials must be `${VARIABLE}` references, as for any import.
pub fn mcp_server_from_standard(name: &str, raw: &Value) -> anyhow::Result<McpServer> {
    import_mcp_server(name, raw, false, &mut LiteralSecrets::refused())
}

/// A connection in the standard `mcpServers` shape (the reverse of
/// [`mcp_server_from_standard`]); whether it is enabled is left out.
pub fn mcp_server_to_standard(server: &McpServer) -> Value {
    let mut entry = serde_json::Map::new();
    if let Some(command) = &server.command {
        entry.insert("command".into(), json!(command));
        if !server.args.is_empty() {
            entry.insert("args".into(), json!(server.args));
        }
        if let Some(cwd) = &server.cwd {
            entry.insert("cwd".into(), json!(cwd));
        }
    }
    if let Some(url) = &server.url {
        entry.insert(
            "type".into(),
            json!(if server.sse { "sse" } else { "http" }),
        );
        entry.insert("url".into(), json!(url));
        let mut headers: serde_json::Map<String, Value> = server
            .headers
            .iter()
            .map(|(name, template)| (name.clone(), json!(template)))
            .collect();
        if let Some(bearer) = &server.bearer_env {
            headers.insert(
                "Authorization".into(),
                json!(format!("Bearer ${{{bearer}}}")),
            );
        }
        headers.extend(
            server
                .header_values
                .iter()
                .map(|(name, value)| (name.clone(), json!(value))),
        );
        if !headers.is_empty() {
            entry.insert("headers".into(), Value::Object(headers));
        }
        if let Some(oauth) = &server.oauth {
            let mut block = serde_json::Map::new();
            let mut put = |field: &str, value: Option<Value>| {
                if let Some(value) = value {
                    block.insert(field.to_owned(), value);
                }
            };
            put("clientId", oauth.client_id.as_ref().map(|id| json!(id)));
            put("callbackPort", oauth.callback_port.map(|port| json!(port)));
            put(
                "authServerMetadataUrl",
                oauth
                    .auth_server_metadata_url
                    .as_ref()
                    .map(|url| json!(url)),
            );
            put("scopes", oauth.scopes.as_ref().map(|scopes| json!(scopes)));
            put(
                "clientSecret",
                oauth
                    .client_secret_env
                    .as_ref()
                    .map(|name| json!(format!("${{{name}}}"))),
            );
            entry.insert("oauth".into(), Value::Object(block));
        }
    }
    if let Some(secs) = server.tool_timeout_secs {
        entry.insert("tool_timeout_secs".into(), json!(secs));
    }
    if !server.env.is_empty() || !server.env_values.is_empty() {
        let env: serde_json::Map<String, Value> = server
            .env
            .iter()
            .map(|(key, variable)| {
                let value = match server.defaults.get(variable) {
                    Some(default) => format!("${{{variable}:-{default}}}"),
                    None => format!("${{{variable}}}"),
                };
                (key.clone(), json!(value))
            })
            .chain(
                server
                    .env_values
                    .iter()
                    .map(|(key, value)| (key.clone(), json!(value))),
            )
            .collect();
        entry.insert("env".into(), Value::Object(env));
    }
    Value::Object(entry)
}
