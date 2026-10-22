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
        ("resource", resource.to_owned()),
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

/// `resource_metadata="..."` from a `WWW-Authenticate: Bearer ...` header.
fn resource_metadata(header: &str) -> Option<String> {
    let start = header.find("resource_metadata=")? + "resource_metadata=".len();
    let rest = header[start..].trim_start_matches('"');
    let end = rest.find(['"', ',', ' ']).unwrap_or(rest.len());
    Some(rest[..end].to_owned()).filter(|url| !url.is_empty())
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
    let scopes: Vec<String> = metadata
        .as_ref()
        .and_then(|m| m["scopes_supported"].as_array())
        .map(|scopes| {
            scopes
                .iter()
                .filter_map(|s| s.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    let issuer_url =
        reqwest::Url::parse(&issuer).context("The authorization server URL is not valid")?;
    let server = match get_json(
        client,
        &well_known(&issuer_url, "oauth-authorization-server"),
    )
    .await
    {
        Some(server) => server,
        None => get_json(client, &well_known(&issuer_url, "openid-configuration"))
            .await
            .context("The MCP server's authorization server publishes no OAuth metadata")?,
    };
    let endpoint = |name: &str| -> anyhow::Result<String> {
        let value = server[name]
            .as_str()
            .with_context(|| format!("The authorization server names no {name}"))?;
        let parsed = reqwest::Url::parse(value).with_context(|| format!("Invalid {name}"))?;
        if parsed.scheme() != "https"
            && !matches!(parsed.host_str(), Some("127.0.0.1" | "localhost"))
        {
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
        registration_endpoint: server["registration_endpoint"].as_str().map(str::to_owned),
        scopes,
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

/// Signs in to the MCP server at `resource`: discovery, client registration, the person
/// signing in through `open` (their browser), and the code exchange. Gives up after `limit`.
pub async fn sign_in(
    resource: &str,
    open: impl FnOnce(&str) -> Result<(), String>,
    limit: Duration,
) -> anyhow::Result<()> {
    let client = client()?;
    let challenge_header = needs_sign_in(resource).await?.unwrap_or_default();
    let server = discover(&client, resource, &challenge_header).await?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let redirect = format!(
        "http://127.0.0.1:{}/callback",
        listener.local_addr()?.port()
    );
    // A client of its own for this sign-in (the redirect's port differs each time).
    let registration = server.registration_endpoint.as_deref().context(
        "This server's sign-in needs a client set up by hand, which Themis does not support yet",
    )?;
    let registered: Value = client
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
        .context("Registering Themis with the authorization server failed")?
        .error_for_status()
        .context("The authorization server refused to register Themis")?
        .json()
        .await?;
    let client_id = registered["client_id"]
        .as_str()
        .context("The registration answer has no client_id")?
        .to_owned();
    let client_secret = registered["client_secret"].as_str().map(str::to_owned);
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
            .append_pair("resource", resource);
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
        ("resource", resource.to_owned()),
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
        },
    )?;
    save(resource, &tokens)
}

/// Waits for the browser to come back to `/callback` with this sign-in's `state`, answers
/// it with a short page, and returns the code.
async fn receive_code(listener: &tokio::net::TcpListener, state: &str) -> anyhow::Result<String> {
    loop {
        let (mut stream, _) = listener.accept().await?;
        let mut request = vec![0u8; 8192];
        let read = stream.read(&mut request).await.unwrap_or(0);
        let head = String::from_utf8_lossy(&request[..read]).into_owned();
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn the_pkce_challenge_is_rfc_7636_s256() {
        // RFC 7636 appendix B.
        assert_eq!(
            challenge_for("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }
}
