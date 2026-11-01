//! Signing in to an OAuth-protected MCP server, end to end against a simulated server that
//! follows the MCP authorization spec: 401 challenge, protected-resource metadata,
//! authorization-server metadata, dynamic client registration, PKCE code exchange, then a
//! connection with the token, and refresh once it expires.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use base64::Engine;
use serde_json::{json, Value};
use sha2::Digest;
use themis_core::plugins::connections::{Connection, McpServer};
use themis_core::plugins::oauth::{self, TokenStore};
use wiremock::matchers::{body_string_contains, header, method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

#[derive(Default)]
struct Memory(Mutex<HashMap<String, String>>);
impl TokenStore for Memory {
    fn load(&self, key: &str) -> Option<String> {
        self.0.lock().unwrap().get(key).cloned()
    }
    fn save(&self, key: &str, value: &str) -> Result<(), String> {
        self.0
            .lock()
            .unwrap()
            .insert(key.to_owned(), value.to_owned());
        Ok(())
    }
    fn delete(&self, key: &str) -> Result<(), String> {
        self.0.lock().unwrap().remove(key);
        Ok(())
    }
}

fn form(request: &Request) -> HashMap<String, String> {
    let body = String::from_utf8_lossy(&request.body);
    reqwest::Url::parse(&format!("http://form/?{body}"))
        .unwrap()
        .query_pairs()
        .into_owned()
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn signing_in_to_an_oauth_mcp_server_then_connecting_and_refreshing() {
    let server = MockServer::builder().start().await;
    let base = server.uri();
    let resource = format!("{base}/mcp");
    let store = Arc::new(Memory::default());
    oauth::set_store(store.clone());

    // The MCP endpoint: tools need a token; "at-1" and "at-2" are the ones the server issues.
    for token in ["at-1", "at-2"] {
        Mock::given(method("POST"))
            .and(path("/mcp"))
            .and(header("authorization", format!("Bearer {token}").as_str()))
            .respond_with(move |request: &Request| {
                let body: Value = serde_json::from_slice(&request.body).unwrap();
                match body.get("id") {
                    Some(id) => ResponseTemplate::new(200).set_body_json(json!({"jsonrpc":"2.0","id":id,"result":{"protocolVersion":"2025-06-18","capabilities":{},"serverInfo":{"name":"notes","version":"1"}}})),
                    None => ResponseTemplate::new(202),
                }
            })
            .mount(&server)
            .await;
    }
    Mock::given(method("POST"))
        .and(path("/mcp"))
        .respond_with(
            ResponseTemplate::new(401).insert_header(
                "WWW-Authenticate",
                format!(
                    r#"Bearer resource_metadata="{base}/.well-known/oauth-protected-resource/mcp""#
                )
                .as_str(),
            ),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/.well-known/oauth-protected-resource/mcp"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "resource": resource, "authorization_servers": [base], "scopes_supported": ["notes.read"]
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/.well-known/oauth-authorization-server"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "issuer": base,
            "authorization_endpoint": format!("{base}/authorize"),
            "token_endpoint": format!("{base}/token"),
            "registration_endpoint": format!("{base}/register"),
            "code_challenge_methods_supported": ["S256"]
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/register"))
        .respond_with(|request: &Request| {
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            assert_eq!(body["token_endpoint_auth_method"], "none");
            assert!(body["redirect_uris"][0]
                .as_str()
                .unwrap()
                .starts_with("http://127.0.0.1:"));
            ResponseTemplate::new(201).set_body_json(json!({"client_id": "themis-1"}))
        })
        .mount(&server)
        .await;
    // The code exchange checks PKCE: the verifier must hash to the challenge sent earlier.
    let challenge = Arc::new(Mutex::new(String::new()));
    let seen = Arc::clone(&challenge);
    Mock::given(method("POST"))
        .and(path("/token"))
        .and(body_string_contains("grant_type=authorization_code"))
        .respond_with(move |request: &Request| {
            let form = form(request);
            let hashed = base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(sha2::Sha256::digest(form["code_verifier"].as_bytes()));
            if hashed != *seen.lock().unwrap() || form["code"] != "code-1" || form["client_id"] != "themis-1" {
                return ResponseTemplate::new(400).set_body_json(json!({"error": "invalid_grant"}));
            }
            ResponseTemplate::new(200).set_body_json(json!({
                "access_token": "at-1", "refresh_token": "rt-1", "expires_in": 3600, "token_type": "Bearer"
            }))
        })
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .and(body_string_contains("grant_type=refresh_token"))
        .respond_with(|request: &Request| {
            assert_eq!(form(request)["refresh_token"], "rt-1");
            ResponseTemplate::new(200)
                .set_body_json(json!({"access_token": "at-2", "expires_in": 3600}))
        })
        .mount(&server)
        .await;

    let mcp = McpServer {
        url: Some(resource.clone()),
        enabled: true,
        ..McpServer::default()
    };
    let root = tempfile::tempdir().unwrap();
    // Before sign-in: the server asks for it.
    let error = Connection::connect(&mcp, root.path()).await.unwrap_err();
    assert!(
        format!("{error:#}").contains("MCP server needs sign-in"),
        "{error:#}"
    );
    assert!(!oauth::signed_in(&resource));

    // Sign in: "the browser" follows the sign-in page straight back to the callback.
    let challenge_slot = Arc::clone(&challenge);
    oauth::sign_in(
        &resource,
        None,
        move |page| {
            let page = reqwest::Url::parse(page).unwrap();
            let query: HashMap<String, String> = page.query_pairs().into_owned().collect();
            assert_eq!(query["code_challenge_method"], "S256");
            assert_eq!(query["resource"], resource_for(&page, &query));
            assert_eq!(query["scope"], "notes.read");
            *challenge_slot.lock().unwrap() = query["code_challenge"].clone();
            let callback = format!(
                "{}?code=code-1&state={}",
                query["redirect_uri"], query["state"]
            );
            std::thread::spawn(move || {
                let answer = ureq_get(&callback);
                assert!(answer.contains("Signed in"), "{answer}");
            });
            Ok(())
        },
        std::time::Duration::from_secs(20),
    )
    .await
    .unwrap();
    assert!(oauth::signed_in(&resource));
    assert!(store
        .0
        .lock()
        .unwrap()
        .values()
        .all(|v| !v.contains("code-1")));

    // Signed in: the connection uses the token.
    let connection = Connection::connect(&mcp, root.path()).await;
    assert!(connection.is_ok(), "{:?}", connection.err());

    // An expired token is refreshed with the refresh token, then used.
    let key = store.0.lock().unwrap().keys().next().unwrap().clone();
    let mut saved: Value = serde_json::from_str(&store.load(&key).unwrap()).unwrap();
    saved["expires_at"] = json!(1);
    store.save(&key, &saved.to_string()).unwrap();
    assert_eq!(
        oauth::access_token(&resource).await.as_deref(),
        Some("at-2")
    );
    assert!(Connection::connect(&mcp, root.path()).await.is_ok());

    // Sign out forgets it.
    oauth::sign_out(&resource).unwrap();
    assert!(!oauth::signed_in(&resource));
}

/// The `resource` the sign-in page names: the MCP server's own URL.
fn resource_for(page: &reqwest::Url, _query: &HashMap<String, String>) -> String {
    format!(
        "http://{}:{}/mcp",
        page.host_str().unwrap(),
        page.port().unwrap()
    )
}

/// A plain GET from "the browser" to the local callback.
fn ureq_get(url: &str) -> String {
    use std::io::{Read, Write};
    let url = reqwest::Url::parse(url).unwrap();
    let mut stream =
        std::net::TcpStream::connect((url.host_str().unwrap(), url.port().unwrap())).unwrap();
    let target = format!("{}?{}", url.path(), url.query().unwrap_or_default());
    write!(
        stream,
        "GET {target} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut answer = String::new();
    stream.read_to_string(&mut answer).unwrap();
    answer
}
