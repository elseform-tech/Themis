//! Sign-in for remote MCP servers, as the MCP authorization spec describes it: OAuth 2.1 with
//! PKCE, the server's protected-resource metadata (RFC 9728) naming its authorization server
//! (RFC 8414 metadata), dynamic client registration (RFC 7591) and the `resource` parameter
//! (RFC 8707). Sign-in opens the person's own browser; a one-time listener on 127.0.0.1
//! receives the code. Tokens are kept by the app's [`TokenStore`] (the Keychain on macOS),
//! never in configuration, and are refreshed when they expire.

use std::sync::{Arc, OnceLock, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::Digest;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::connections::McpOauth;

/// Where the app keeps tokens (the Keychain in the desktop app; memory in tests).
pub trait TokenStore: Send + Sync {
    fn load(&self, key: &str) -> Option<String>;
    fn save(&self, key: &str, value: &str) -> Result<(), String>;
    fn delete(&self, key: &str) -> Result<(), String>;
}

fn slot() -> &'static RwLock<Option<Arc<dyn TokenStore>>> {
    static STORE: OnceLock<RwLock<Option<Arc<dyn TokenStore>>>> = OnceLock::new();
    STORE.get_or_init(Default::default)
}

/// Sets where tokens are kept, once at startup.
pub fn set_store(store: Arc<dyn TokenStore>) {
    *slot().write().unwrap_or_else(|e| e.into_inner()) = Some(store);
}

fn store() -> Option<Arc<dyn TokenStore>> {
    slot().read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Reads one entry of the credential store (also used for saved keys).
pub(crate) fn load_entry(key: &str) -> Option<String> {
    store()?.load(key)
}

/// What one sign-in left behind for a server (its URL is the key).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Tokens {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    /// Seconds since the Unix epoch.
    #[serde(default)]
    expires_at: Option<u64>,
    client_id: String,
    #[serde(default)]
    client_secret: Option<String>,
    token_endpoint: String,
    /// The resource the sign-in named, which a refresh names again.
    #[serde(default)]
    resource: Option<String>,
}

fn key(resource: &str) -> String {
    format!("mcp-oauth {}", resource.trim_end_matches('/'))
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

fn load(resource: &str) -> Option<Tokens> {
    serde_json::from_str(&store()?.load(&key(resource))?).ok()
}

fn save(resource: &str, tokens: &Tokens) -> anyhow::Result<()> {
    let store = store().context("No place to keep sign-in tokens")?;
    store
        .save(&key(resource), &serde_json::to_string(tokens)?)
        .map_err(|e| anyhow::anyhow!(e))
}

/// Whether this server has a sign-in on this computer.
pub fn signed_in(resource: &str) -> bool {
    load(resource).is_some()
}

/// Forgets this server's sign-in.
pub fn sign_out(resource: &str) -> Result<(), String> {
    match store() {
        Some(store) => store.delete(&key(resource)),
        None => Ok(()),
    }
}

fn client() -> anyhow::Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(concat!("Themis/", env!("CARGO_PKG_VERSION")))
        .build()?)
}

/// A current access token for this server, refreshed first when it is about to expire.
/// `None` when it has no sign-in (or the refresh was refused: sign in again).
pub async fn access_token(resource: &str) -> Option<String> {
    let tokens = load(resource)?;
    let fresh = tokens.expires_at.is_none_or(|at| at > now() + 60);
    if fresh {
        return Some(tokens.access_token);
    }
    let refresh = tokens.refresh_token.clone()?;
    let mut form = vec![
        ("grant_type", "refresh_token".to_owned()),
        ("refresh_token", refresh),
        ("client_id", tokens.client_id.clone()),
        (
            "resource",
            tokens
                .resource
                .clone()
                .unwrap_or_else(|| resource.to_owned()),
        ),
    ];
    if let Some(secret) = &tokens.client_secret {
        form.push(("client_secret", secret.clone()));
    }
    let answer = client()
        .ok()?
        .post(&tokens.token_endpoint)
        .form(&form)
        .send()
        .await
        .ok()?;
    if !answer.status().is_success() {
        return None;
    }
    let body: Value = answer.json().await.ok()?;
    let renewed = tokens_from(&body, &tokens).ok()?;
    save(resource, &renewed).ok()?;
    Some(renewed.access_token)
}

fn tokens_from(body: &Value, previous: &Tokens) -> anyhow::Result<Tokens> {
    let access_token = body["access_token"]
        .as_str()
        .filter(|token| !token.is_empty())
        .context("The sign-in answer has no access token")?
        .to_owned();
    Ok(Tokens {
        access_token,
        refresh_token: body["refresh_token"]
            .as_str()
            .map(str::to_owned)
            .or_else(|| previous.refresh_token.clone()),
        expires_at: body["expires_in"].as_u64().map(|seconds| now() + seconds),
        ..previous.clone()
    })
}

/// The authorization server's endpoints for one MCP server.
#[derive(Debug)]
struct AuthServer {
    authorization_endpoint: String,
    token_endpoint: String,
    registration_endpoint: Option<String>,
    scopes: Vec<String>,
    /// What the token is for: the protected resource the server names, or its own address.
    resource: String,
    /// The hosts a person's client secret may go to: the server's and the authorization servers
    /// it declares. Not a plugin's metadata address, nor where that metadata points.
    secret_hosts: Vec<String>,
}

/// `https://host[:port]/.well-known/<name>` with the URL's path after it (RFC 8414 / 9728).
fn well_known(url: &reqwest::Url, name: &str) -> String {
    format!(
        "{}://{}{}/.well-known/{name}{}",
        url.scheme(),
        url.host_str().unwrap_or_default(),
        port_suffix(url),
        url.path().trim_end_matches('/')
    )
}

fn port_suffix(url: &reqwest::Url) -> String {
    url.port()
        .map(|port| format!(":{port}"))
        .unwrap_or_default()
}

/// The value of `name="..."` in a `WWW-Authenticate: Bearer ...` header. It starts a parameter, so
/// `name` inside another one's value doesn't count.
fn challenge_value(header: &str, name: &str) -> Option<String> {
    let wanted = format!("{name}=");
    let start = header
        .match_indices(&wanted)
        .find(|(at, _)| *at == 0 || header[..*at].ends_with([' ', ',']))?
        .0
        + wanted.len();
    let rest = &header[start..];
    let value = match rest.strip_prefix('"') {
        Some(quoted) => &quoted[..quoted.find('"')?],
        None => &rest[..rest.find([',', ' ']).unwrap_or(rest.len())],
    };
    Some(value.to_owned()).filter(|value| !value.is_empty())
}

/// `resource_metadata="..."` from a `WWW-Authenticate: Bearer ...` header.
fn resource_metadata(header: &str) -> Option<String> {
    challenge_value(header, "resource_metadata")
}

/// `url` without its query and fragment: what names a server.
fn bare(url: &reqwest::Url) -> String {
    let mut bare = url.clone();
    bare.set_query(None);
    bare.set_fragment(None);
    bare.to_string()
}

/// What to ask a token for: the protected resource the server's metadata names when that covers
/// this address (RFC 9728), else the address itself. `None` when it names another server, whose
/// metadata says nothing about this one.
fn resource_for(url: &reqwest::Url, named: Option<&str>) -> Option<String> {
    let own = bare(url);
    let Some(named) = named else {
        return Some(own);
    };
    let parsed = reqwest::Url::parse(named).ok()?;
    let inside = parsed.origin() == url.origin()
        && parsed.query().is_none()
        && url
            .path()
            .trim_end_matches('/')
            .strip_prefix(parsed.path().trim_end_matches('/'))
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'));
    inside.then(|| named.to_owned())
}

/// Whether an address the provider gave may be used: HTTPS, or HTTP on this computer.
fn usable_endpoint(value: &str) -> bool {
    reqwest::Url::parse(value).is_ok_and(|parsed| {
        parsed.scheme() == "https"
            || (parsed.scheme() == "http"
                && matches!(parsed.host_str(), Some("127.0.0.1" | "localhost")))
    })
}

async fn get_json(client: &reqwest::Client, url: &str) -> Option<Value> {
    let answer = client
        .get(url)
        .header("Accept", "application/json")
        .send()
        .await
        .ok()?;
    if !answer.status().is_success() {
        return None;
    }
    answer.json().await.ok()
}

/// Whether the server asks for a sign-in: an `initialize` without a token is answered with
/// 401. Returns its `WWW-Authenticate` header (possibly empty) when it does.
pub async fn needs_sign_in(resource: &str) -> anyhow::Result<Option<String>> {
    let answer = client()?
        .post(resource)
        .header("Accept", "application/json, text/event-stream")
        .json(&json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"Themis","version":env!("CARGO_PKG_VERSION")}}}))
        .send()
        .await
        .context("The MCP server could not be reached")?;
    if answer.status() != reqwest::StatusCode::UNAUTHORIZED {
        return Ok(None);
    }
    Ok(Some(
        answer
            .headers()
            .get("www-authenticate")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned(),
    ))
}

async fn discover(
    client: &reqwest::Client,
    resource: &str,
    challenge: &str,
    settings: &McpOauth,
) -> anyhow::Result<AuthServer> {
    let url = reqwest::Url::parse(resource).context("The MCP server URL is not valid")?;
    // Its protected-resource metadata names the authorization server and the scopes.
    let metadata_url = resource_metadata(challenge)
        .unwrap_or_else(|| well_known(&url, "oauth-protected-resource"));
    let metadata = match get_json(client, &metadata_url).await {
        Some(metadata) => Some(metadata),
        None => {
            get_json(
                client,
                &format!(
                    "{}://{}{}/.well-known/oauth-protected-resource",
                    url.scheme(),
                    url.host_str().unwrap_or_default(),
                    port_suffix(&url)
                ),
            )
            .await
        }
    };
    let mut secret_hosts: Vec<String> = url.host_str().map(str::to_owned).into_iter().collect();
    secret_hosts.extend(
        metadata
            .as_ref()
            .and_then(|m| m["authorization_servers"].as_array())
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .filter_map(|server| reqwest::Url::parse(server).ok())
            .filter_map(|server| server.host_str().map(str::to_owned)),
    );
    let named = metadata.as_ref().and_then(|m| m["resource"].as_str());
    let token_resource = resource_for(&url, named).with_context(|| {
        "This server's sign-in metadata is for another server, so Themis won't use it"
    })?;
    let issuer = metadata
        .as_ref()
        .and_then(|m| m["authorization_servers"][0].as_str().map(str::to_owned))
        // Servers from before RFC 9728 support act as their own authorization server.
        .unwrap_or_else(|| {
            format!(
                "{}://{}{}",
                url.scheme(),
                url.host_str().unwrap_or_default(),
                port_suffix(&url)
            )
        });
    // The plugin's own choice, then the challenge's, then everything the server lists (the MCP
    // authorization spec's order).
    let scopes: Vec<String> = match settings
        .scopes
        .clone()
        .or_else(|| challenge_value(challenge, "scope"))
    {
        Some(scopes) => scopes.split_whitespace().map(str::to_owned).collect(),
        None => metadata
            .as_ref()
            .and_then(|m| m["scopes_supported"].as_array())
            .map(|scopes| {
                scopes
                    .iter()
                    .filter_map(|s| s.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default(),
    };
    let server = match &settings.auth_server_metadata_url {
        Some(address) => get_json(client, address)
            .await
            .context("The plugin's authorization server metadata could not be read")?,
        None => {
            let issuer_url = reqwest::Url::parse(&issuer)
                .context("The authorization server URL is not valid")?;
            match get_json(
                client,
                &well_known(&issuer_url, "oauth-authorization-server"),
            )
            .await
            {
                Some(server) => server,
                None => get_json(client, &well_known(&issuer_url, "openid-configuration"))
                    .await
                    .context("The MCP server's authorization server publishes no OAuth metadata")?,
            }
        }
    };
    let endpoint = |name: &str| -> anyhow::Result<String> {
        let value = server[name]
            .as_str()
            .with_context(|| format!("The authorization server names no {name}"))?;
        reqwest::Url::parse(value).with_context(|| format!("Invalid {name}"))?;
        if !usable_endpoint(value) {
            bail!("The authorization server's {name} is not https");
        }
        Ok(value.to_owned())
    };
    if let Some(methods) = server["code_challenge_methods_supported"].as_array() {
        if !methods.iter().any(|m| m == "S256") {
            bail!("The authorization server does not support PKCE (S256), which MCP requires");
        }
    }
    Ok(AuthServer {
        authorization_endpoint: endpoint("authorization_endpoint")?,
        token_endpoint: endpoint("token_endpoint")?,
        registration_endpoint: server["registration_endpoint"]
            .as_str()
            .filter(|address| usable_endpoint(address))
            .map(str::to_owned),
        scopes,
        resource: token_resource,
        secret_hosts,
    })
}

fn random_token() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

fn challenge_for(verifier: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(sha2::Sha256::digest(verifier.as_bytes()))
}

/// A client of Themis's own for this sign-in (the redirect's port differs each time): its id and
/// any secret the provider gave it.
async fn register(
    client: &reqwest::Client,
    server: &AuthServer,
    redirect: &str,
) -> anyhow::Result<(String, Option<String>)> {
    let registration = server.registration_endpoint.as_deref().context(
        "This server's sign-in needs a client set up by hand, and its plugin names none",
    )?;
    let answer = client
        .post(registration)
        .json(&json!({
            "client_name": "Themis",
            "redirect_uris": [redirect],
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
            "token_endpoint_auth_method": "none",
        }))
        .send()
        .await
        .context("Registering Themis with the authorization server failed")?;
    if !answer.status().is_success() {
        bail!(
            "The authorization server refused to register Themis (HTTP {})",
            answer.status().as_u16()
        );
    }
    let registered: Value = answer.json().await?;
    let client_id = registered["client_id"]
        .as_str()
        .context("The registration answer has no client_id")?
        .to_owned();
    Ok((
        client_id,
        registered["client_secret"].as_str().map(str::to_owned),
    ))
}

/// Signs in to the MCP server at `resource`: discovery, the plugin's client or a registered one,
/// the person signing in through `open` (their browser), and the code exchange. Gives up after
/// `limit`.
pub async fn sign_in(
    resource: &str,
    settings: Option<&McpOauth>,
    open: impl FnOnce(&str) -> Result<(), String>,
    limit: Duration,
) -> anyhow::Result<()> {
    let settings = settings.cloned().unwrap_or_default();
    let client = client()?;
    let challenge_header = needs_sign_in(resource).await?.unwrap_or_default();
    let server = discover(&client, resource, &challenge_header, &settings).await?;
    if settings.client_id.is_none() && server.registration_endpoint.is_none() {
        bail!("This server's sign-in needs a client set up by hand, and its plugin names none");
    }
    let port = settings.callback_port.unwrap_or(0);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::AddrInUse => anyhow::anyhow!(
                "Port {port}, which this server's sign-in comes back to, is in use. Close what is using it and try again."
            ),
            _ => error.into(),
        })?;
    let port = listener.local_addr()?.port();
    // A preset client was registered with an exact address, usually `localhost`. One
    // registered here says the loopback address, as RFC 8252 recommends.
    let host = if settings.client_id.is_some() {
        "localhost"
    } else {
        "127.0.0.1"
    };
    let redirect = format!("http://{host}:{port}/callback");
    let (client_id, mut client_secret) = match &settings.client_id {
        Some(id) => (id.clone(), None),
        None => register(&client, &server, &redirect).await?,
    };
    if let Some(name) = &settings.client_secret_env {
        // The person allowed this secret for the server. Its token endpoint must be the server or
        // an authorization server it declares; redirects aren't followed, so it stays there.
        let endpoint_host = reqwest::Url::parse(&server.token_endpoint)?
            .host_str()
            .map(str::to_owned);
        if !endpoint_host.is_some_and(|host| server.secret_hosts.contains(&host)) {
            bail!("Themis won't send {name} to this sign-in's token address: it isn't this server's or an authorization server it declares");
        }
        client_secret = Some(
            super::keys::value(name)
                .ok_or_else(|| super::keys::missing_error(std::slice::from_ref(name)))?,
        );
    }
    let verifier = random_token();
    let state = random_token();
    let mut authorize = reqwest::Url::parse(&server.authorization_endpoint)?;
    {
        let mut query = authorize.query_pairs_mut();
        query
            .append_pair("response_type", "code")
            .append_pair("client_id", &client_id)
            .append_pair("redirect_uri", &redirect)
            .append_pair("code_challenge", &challenge_for(&verifier))
            .append_pair("code_challenge_method", "S256")
            .append_pair("state", &state)
            .append_pair("resource", &server.resource);
        if !server.scopes.is_empty() {
            query.append_pair("scope", &server.scopes.join(" "));
        }
    }
    open(authorize.as_str()).map_err(|e| anyhow::anyhow!(e))?;
    let code = tokio::time::timeout(limit, receive_code(&listener, &state))
        .await
        .context("Sign-in was not finished in time")??;
    let mut form = vec![
        ("grant_type", "authorization_code".to_owned()),
        ("code", code),
        ("redirect_uri", redirect.clone()),
        ("client_id", client_id.clone()),
        ("code_verifier", verifier),
        ("resource", server.resource.clone()),
    ];
    if let Some(secret) = &client_secret {
        form.push(("client_secret", secret.clone()));
    }
    let answer = client
        .post(&server.token_endpoint)
        .form(&form)
        .send()
        .await
        .context("The token request failed")?;
    if !answer.status().is_success() {
        bail!(
            "The authorization server refused the sign-in (HTTP {})",
            answer.status().as_u16()
        );
    }
    let body: Value = answer.json().await?;
    let tokens = tokens_from(
        &body,
        &Tokens {
            access_token: String::new(),
            refresh_token: None,
            expires_at: None,
            client_id,
            client_secret,
            token_endpoint: server.token_endpoint,
            resource: Some(server.resource),
        },
    )?;
    save(resource, &tokens)
}

/// Waits for the browser to come back to `/callback` with this sign-in's `state`, answers
/// it with a short page, and returns the code.
async fn receive_code(listener: &tokio::net::TcpListener, state: &str) -> anyhow::Result<String> {
    loop {
        let (mut stream, _) = listener.accept().await?;
        let Ok(head) = tokio::time::timeout(Duration::from_secs(5), read_head(&mut stream)).await
        else {
            // A connection opened ahead of time that never sends a request.
            continue;
        };
        let target = head
            .split_whitespace()
            .nth(1)
            .unwrap_or_default()
            .to_owned();
        let Ok(url) = reqwest::Url::parse(&format!("http://127.0.0.1{target}")) else {
            continue;
        };
        if url.path() != "/callback" {
            let _ = stream
                .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n")
                .await;
            continue;
        }
        let pairs: std::collections::HashMap<String, String> =
            url.query_pairs().into_owned().collect();
        let (ok, page, result) = if pairs.get("state").map(String::as_str) != Some(state) {
            (
                false,
                "This sign-in link is not the one Themis started.",
                None,
            )
        } else if let Some(error) = pairs.get("error") {
            (
                false,
                "Sign-in was cancelled or refused. You can close this tab.",
                Some(Err(error.clone())),
            )
        } else if let Some(code) = pairs.get("code") {
            (
                true,
                "Signed in. You can close this tab and return to Themis.",
                Some(Ok(code.clone())),
            )
        } else {
            (false, "The sign-in answer had no code.", None)
        };
        let body = format!("<!doctype html><meta charset=utf-8><title>Themis</title><body style=\"font:15px system-ui;margin:3em\"><p>{page}</p>");
        let status = if ok { "200 OK" } else { "400 Bad Request" };
        let _ = stream
            .write_all(format!("HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes())
            .await;
        match result {
            Some(Ok(code)) => return Ok(code),
            Some(Err(error)) => bail!("Sign-in was refused: {error}"),
            None => continue,
        }
    }
}

/// Reads a request's head (up to 8 KiB): a browser may send it in more than one piece.
async fn read_head(stream: &mut tokio::net::TcpStream) -> String {
    let mut request = Vec::new();
    let mut chunk = [0u8; 2048];
    while request.len() < 8192 && !request.windows(4).any(|w| w == b"\r\n\r\n") {
        match stream.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(read) => request.extend_from_slice(&chunk[..read]),
        }
    }
    String::from_utf8_lossy(&request).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_callback_waits_for_a_request_that_arrives_in_pieces() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let browser = tokio::spawn(async move {
            let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .unwrap();
            for piece in [
                "GET ",
                "/callback?code=code-1&state=s-1",
                " HTTP/1.1\r\nHost: 127.0.0.1\r\n",
                "\r\n",
            ] {
                stream.write_all(piece.as_bytes()).await.unwrap();
                stream.flush().await.unwrap();
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            let mut answer = String::new();
            stream.read_to_string(&mut answer).await.unwrap();
            answer
        });
        let code = tokio::time::timeout(Duration::from_secs(5), receive_code(&listener, "s-1"))
            .await
            .expect("the code once the whole request arrived")
            .unwrap();
        assert_eq!(code, "code-1");
        assert!(browser.await.unwrap().contains("Signed in"));
    }

    #[tokio::test]
    async fn a_connection_that_never_sends_a_request_does_not_hold_up_the_callback() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        // Browsers open connections ahead of time; this one stays silent.
        let _idle = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        let browser = tokio::spawn(async move {
            let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .unwrap();
            stream
                .write_all(b"GET /callback?code=code-2&state=s-2 HTTP/1.1\r\n\r\n")
                .await
                .unwrap();
            let mut answer = String::new();
            stream.read_to_string(&mut answer).await.unwrap();
            answer
        });
        let code = tokio::time::timeout(Duration::from_secs(10), receive_code(&listener, "s-2"))
            .await
            .expect("the code from the second connection")
            .unwrap();
        assert_eq!(code, "code-2");
        assert!(browser.await.unwrap().contains("Signed in"));
    }

    #[test]
    fn well_known_urls_keep_the_port_and_path() {
        let url = reqwest::Url::parse("https://mcp.example.com/v1/mcp").unwrap();
        assert_eq!(
            well_known(&url, "oauth-protected-resource"),
            "https://mcp.example.com/.well-known/oauth-protected-resource/v1/mcp"
        );
        let local = reqwest::Url::parse("http://127.0.0.1:9000/").unwrap();
        assert_eq!(
            well_known(&local, "oauth-authorization-server"),
            "http://127.0.0.1:9000/.well-known/oauth-authorization-server"
        );
    }

    #[test]
    fn the_resource_metadata_link_is_read_from_the_challenge() {
        assert_eq!(
            resource_metadata(r#"Bearer error="invalid_token", resource_metadata="https://a.example/.well-known/oauth-protected-resource""#).as_deref(),
            Some("https://a.example/.well-known/oauth-protected-resource")
        );
        assert_eq!(resource_metadata("Bearer"), None);
    }

    #[test]
    fn the_challenge_parameters_are_read_exactly() {
        let header = r#"Bearer resource_metadata="https://a/x", scope="read write", realm=z"#;
        assert_eq!(
            challenge_value(header, "scope").as_deref(),
            Some("read write")
        );
        assert_eq!(challenge_value(header, "realm").as_deref(), Some("z"));
        assert_eq!(challenge_value("Bearer x_scope=\"no\"", "scope"), None);
        assert_eq!(challenge_value("Bearer scope=\"open", "scope"), None);
    }

    #[test]
    fn the_token_names_the_resource_the_server_names_when_it_covers_the_address() {
        let url = |text: &str| reqwest::Url::parse(text).unwrap();
        // As the real servers answer: Slack and Wingspan name their origin, Datadog its path.
        for (address, named) in [
            ("https://mcp.slack.com/mcp", "https://mcp.slack.com"),
            (
                "https://api.wingspan.app/v3/mcp",
                "https://api.wingspan.app",
            ),
            ("https://mcp.mongodb.com/", "https://mcp.mongodb.com"),
            (
                "https://d.example/v1/mcp?referrer=x",
                "https://d.example/v1/mcp",
            ),
        ] {
            assert_eq!(
                resource_for(&url(address), Some(named)).as_deref(),
                Some(named)
            );
        }
        // Nothing named: the address, without its query.
        assert_eq!(
            resource_for(&url("https://d.example/v1/mcp?referrer=x"), None).as_deref(),
            Some("https://d.example/v1/mcp")
        );
        for (address, named) in [
            ("https://a.example/mcp", "https://b.example/mcp"),
            ("https://a.example/mcp", "http://a.example/mcp"),
            ("https://a.example/mcpx", "https://a.example/mcp"),
            ("https://a.example/mcp", "https://a.example/mcp/deeper"),
            ("https://a.example/mcp", "not an address"),
        ] {
            assert_eq!(resource_for(&url(address), Some(named)), None, "{named}");
        }
    }

    proptest::proptest! {
        #[test]
        fn any_challenge_and_any_named_resource_read_cleanly(
            header in "\\PC{0,200}",
            named in "\\PC{0,80}",
        ) {
            let _ = challenge_value(&header, "scope");
            let _ = resource_metadata(&header);
            let url = reqwest::Url::parse("https://mcp.example.com/v1/mcp").unwrap();
            if let Some(resource) = resource_for(&url, Some(&named)) {
                // Whatever it is, the token is only ever asked for this server's own origin.
                proptest::prop_assert_eq!(
                    reqwest::Url::parse(&resource).unwrap().origin(),
                    url.origin()
                );
            }
            let _ = usable_endpoint(&named);
        }
    }

    #[test]
    fn the_pkce_challenge_is_rfc_7636_s256() {
        // RFC 7636 appendix B.
        assert_eq!(
            challenge_for("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }
}
