//! Sign-in that honours a plugin's `oauth` block, against a local fake provider: its client id and
//! callback port, its scopes and authorization server metadata address, the protected resource the
//! server names, PKCE, the state check, token refresh and an expired sign-in. Each provider's
//! shape is one a catalog plugin's real provider has (see docs/plugins.md).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use base64::Engine;
use serde_json::{json, Value};
use sha2::Digest;
use themis_core::plugins::connections::{Connection, McpOauth, McpServer};
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

/// The one store of this test binary: several tests sign in at once, each to its own address.
fn store() -> Arc<Memory> {
    static STORE: OnceLock<Arc<Memory>> = OnceLock::new();
    let store = STORE.get_or_init(Arc::default).clone();
    oauth::set_store(store.clone());
    store
}

fn stored(store: &Memory, resource: &str) -> Value {
    serde_json::from_str(&store.load(&format!("mcp-oauth {resource}")).unwrap()).unwrap()
}

fn form(request: &Request) -> HashMap<String, String> {
    let body = String::from_utf8_lossy(&request.body);
    reqwest::Url::parse(&format!("http://form/?{body}"))
        .unwrap()
        .query_pairs()
        .into_owned()
        .collect()
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// A plain GET from "the browser" to the local callback, returning the answer.
fn browser_get(url: &str) -> String {
    use std::io::{Read, Write};
    let url = reqwest::Url::parse(url).unwrap();
    let host = match url.host_str().unwrap() {
        // A browser tries the loopback address when `localhost` has nothing on `::1`.
        "localhost" => "127.0.0.1",
        other => other,
    };
    let mut stream = std::net::TcpStream::connect((host, url.port().unwrap())).unwrap();
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

fn query(page: &str) -> HashMap<String, String> {
    reqwest::Url::parse(page)
        .unwrap()
        .query_pairs()
        .into_owned()
        .collect()
}

/// The MCP endpoint at `/mcp`: it answers `initialize` and `tools/list` for one of `tokens`, and
/// asks for a sign-in otherwise.
async fn mcp_endpoint(server: &MockServer, tokens: &[&str]) {
    let base = server.uri();
    for token in tokens {
        Mock::given(method("POST"))
            .and(path("/mcp"))
            .and(header("authorization", format!("Bearer {token}").as_str()))
            .respond_with(|request: &Request| {
                let body: Value = serde_json::from_slice(&request.body).unwrap();
                let result = if body["method"] == "tools/list" {
                    json!({"tools":[{"name":"echo","description":"","inputSchema":{"type":"object"}}]})
                } else {
                    json!({"protocolVersion":"2025-06-18","capabilities":{},"serverInfo":{"name":"fake","version":"1"}})
                };
                match body.get("id") {
                    Some(id) => ResponseTemplate::new(200)
                        .set_body_json(json!({"jsonrpc":"2.0","id":id,"result":result})),
                    None => ResponseTemplate::new(202),
                }
            })
            .mount(server)
            .await;
    }
    Mock::given(method("POST"))
        .and(path("/mcp"))
        .respond_with(
            ResponseTemplate::new(401).insert_header(
                "WWW-Authenticate",
                format!(
                    r#"Bearer resource_metadata="{base}/.well-known/oauth-protected-resource""#
                )
                .as_str(),
            ),
        )
        .mount(server)
        .await;
}

/// Metadata as Slack's and Render's servers answer: the resource is the origin, nothing registers
/// clients, and `/token` checks PKCE, the client, the redirect and the resource.
async fn provider_without_registration(server: &MockServer, client: &'static str) {
    let base = server.uri();
    Mock::given(method("GET"))
        .and(path("/.well-known/oauth-protected-resource"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "resource": base, "authorization_servers": [base], "scopes_supported": ["read", "write"]
        })))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/.well-known/oauth-authorization-server"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "issuer": base,
            "authorization_endpoint": format!("{base}/authorize"),
            "token_endpoint": format!("{base}/token"),
            "code_challenge_methods_supported": ["S256"]
        })))
        .mount(server)
        .await;
    let redirect = Arc::new(Mutex::new(String::new()));
    let challenge = Arc::new(Mutex::new(String::new()));
    let seen = (Arc::clone(&redirect), Arc::clone(&challenge));
    let resource = base.clone();
    Mock::given(method("POST"))
        .and(path("/token"))
        .and(body_string_contains("grant_type=authorization_code"))
        .respond_with(move |request: &Request| {
            let form = form(request);
            let hashed = base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(sha2::Sha256::digest(form["code_verifier"].as_bytes()));
            let good = hashed == *seen.1.lock().unwrap()
                && form["code"] == "code-1"
                && form["client_id"] == client
                && form["redirect_uri"] == *seen.0.lock().unwrap()
                && form["resource"] == resource;
            if !good {
                return ResponseTemplate::new(400).set_body_json(json!({"error":"invalid_grant"}));
            }
            ResponseTemplate::new(200).set_body_json(json!({
                "access_token":"at-1","refresh_token":"rt-1","expires_in":3600,"token_type":"Bearer"
            }))
        })
        .mount(server)
        .await;
    SEEN.lock().unwrap().insert(base, (redirect, challenge));
}

type Seen = (Arc<Mutex<String>>, Arc<Mutex<String>>);
static SEEN: std::sync::LazyLock<Mutex<HashMap<String, Seen>>> =
    std::sync::LazyLock::new(Default::default);

/// "The browser": notes the page's redirect and PKCE challenge and comes straight back with the
/// code, in a real request to the local listener.
fn sign_in_page(base: &str) -> impl FnOnce(&str) -> Result<(), String> + Send + 'static {
    let seen = SEEN.lock().unwrap()[base].clone();
    move |page: &str| {
        let query = query(page);
        *seen.0.lock().unwrap() = query["redirect_uri"].clone();
        *seen.1.lock().unwrap() = query["code_challenge"].clone();
        let callback = format!(
            "{}?code=code-1&state={}",
            query["redirect_uri"], query["state"]
        );
        std::thread::spawn(move || {
            let answer = browser_get(&callback);
            assert!(answer.contains("Signed in"), "{answer}");
        });
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_preset_client_id_and_callback_port_are_used_and_the_whole_flow_ends_in_an_mcp_call() {
    let store = store();
    let server = MockServer::start().await;
    let base = server.uri();
    let resource = format!("{base}/mcp");
    mcp_endpoint(&server, &["at-1", "at-2"]).await;
    provider_without_registration(&server, "1601.8899").await;
    let port = free_port();
    let settings = McpOauth {
        client_id: Some("1601.8899".into()),
        callback_port: Some(port),
        ..McpOauth::default()
    };
    let page = Arc::new(Mutex::new(String::new()));
    let (noted, come_back) = (Arc::clone(&page), sign_in_page(&base));
    oauth::sign_in(
        &resource,
        Some(&settings),
        move |url| {
            *noted.lock().unwrap() = url.to_owned();
            come_back(url)
        },
        std::time::Duration::from_secs(20),
    )
    .await
    .unwrap();
    let page = query(&page.lock().unwrap());
    assert_eq!(page["client_id"], "1601.8899");
    // A client someone registered is registered with this exact address.
    assert_eq!(
        page["redirect_uri"],
        format!("http://localhost:{port}/callback")
    );
    assert_eq!(page["code_challenge_method"], "S256");
    assert!(page["state"].len() >= 22, "Render refuses a shorter state");
    // The resource the server names, not the endpoint's path.
    assert_eq!(page["resource"], base);
    // With no scopes of its own, the ones the server lists.
    assert_eq!(page["scope"], "read write");

    // The token is in the credential store, with no sign of the code.
    assert!(oauth::signed_in(&resource));
    let saved = stored(&store, &resource);
    assert_eq!(saved["access_token"], "at-1");
    assert_eq!(saved["client_id"], "1601.8899");

    // And an MCP call works with it.
    let mcp = McpServer {
        url: Some(resource.clone()),
        enabled: true,
        oauth: Some(settings),
        ..McpServer::default()
    };
    let root = tempfile::tempdir().unwrap();
    let mut connection = Connection::connect(&mcp, root.path()).await.unwrap();
    let tools = connection.tools().await.unwrap();
    assert_eq!(tools[0]["name"], "echo");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_plugins_scopes_and_metadata_address_win_and_dynamic_registration_still_runs() {
    let store = store();
    let server = MockServer::start().await;
    let base = server.uri();
    let resource = format!("{base}/mcp");
    mcp_endpoint(&server, &["at-w"]).await;
    // As Wingspan: the protected-resource metadata lists more scopes than the plugin asks for.
    Mock::given(method("GET"))
        .and(path("/.well-known/oauth-protected-resource"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "resource": base, "authorization_servers": [base],
            "scopes_supported": ["openid", "email", "offline_access"]
        })))
        .mount(&server)
        .await;
    // Only the address the plugin names has the metadata.
    Mock::given(method("GET"))
        .and(path("/custom/metadata"))
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
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"client_id":"themis-9"})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(|request: &Request| {
            let form = form(request);
            if form["client_id"] != "themis-9" {
                return ResponseTemplate::new(400);
            }
            ResponseTemplate::new(200).set_body_json(json!({"access_token":"at-w"}))
        })
        .mount(&server)
        .await;
    let settings = McpOauth {
        scopes: Some("offline_access".into()),
        auth_server_metadata_url: Some(format!("{base}/custom/metadata")),
        ..McpOauth::default()
    };
    let page = Arc::new(Mutex::new(String::new()));
    let noted = Arc::clone(&page);
    oauth::sign_in(
        &resource,
        Some(&settings),
        move |url| {
            *noted.lock().unwrap() = url.to_owned();
            let query = query(url);
            let callback = format!("{}?code=c&state={}", query["redirect_uri"], query["state"]);
            std::thread::spawn(move || browser_get(&callback));
            Ok(())
        },
        std::time::Duration::from_secs(20),
    )
    .await
    .unwrap();
    let page = query(&page.lock().unwrap());
    assert_eq!(page["scope"], "offline_access");
    assert_eq!(page["client_id"], "themis-9");
    // A client registered here says which address it uses: the loopback one, on any port.
    assert!(page["redirect_uri"].starts_with("http://127.0.0.1:"));
    assert_eq!(stored(&store, &resource)["access_token"], "at-w");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_wrong_state_is_refused_and_the_real_one_still_finishes() {
    let store = store();
    let server = MockServer::start().await;
    let base = server.uri();
    let resource = format!("{base}/mcp");
    mcp_endpoint(&server, &["at-1"]).await;
    provider_without_registration(&server, "preset").await;
    let settings = McpOauth {
        client_id: Some("preset".into()),
        ..McpOauth::default()
    };
    let refused = Arc::new(Mutex::new(String::new()));
    let noted = Arc::clone(&refused);
    let come_back = sign_in_page(&base);
    let during = oauth::signed_in(&resource);
    assert!(!during);
    oauth::sign_in(
        &resource,
        Some(&settings),
        move |url| {
            let query = query(url);
            let forged = format!("{}?code=stolen&state=not-the-state", query["redirect_uri"]);
            // The listener answers once this returns: the forged request first, then the real.
            let (page, come_back) = (url.to_owned(), come_back);
            std::thread::spawn(move || {
                let answer = browser_get(&forged);
                assert!(answer.starts_with("HTTP/1.1 400"), "{answer}");
                *noted.lock().unwrap() = answer;
                come_back(&page).unwrap();
            });
            Ok(())
        },
        std::time::Duration::from_secs(20),
    )
    .await
    .unwrap();
    assert!(refused
        .lock()
        .unwrap()
        .contains("not the one Themis started"));
    assert_eq!(stored(&store, &resource)["access_token"], "at-1");
}

#[tokio::test(flavor = "multi_thread")]
async fn only_a_wrong_state_never_signs_in() {
    let _store = store();
    let server = MockServer::start().await;
    let base = server.uri();
    let resource = format!("{base}/mcp");
    mcp_endpoint(&server, &[]).await;
    provider_without_registration(&server, "preset").await;
    let settings = McpOauth {
        client_id: Some("preset".into()),
        ..McpOauth::default()
    };
    let outcome = oauth::sign_in(
        &resource,
        Some(&settings),
        |url| {
            let query = query(url);
            let forged = format!("{}?code=stolen&state=not-the-state", query["redirect_uri"]);
            std::thread::spawn(move || browser_get(&forged));
            Ok(())
        },
        std::time::Duration::from_millis(800),
    )
    .await;
    assert!(format!("{:#}", outcome.unwrap_err()).contains("not finished in time"));
    assert!(!oauth::signed_in(&resource));
}

#[tokio::test(flavor = "multi_thread")]
async fn an_expired_token_is_refreshed_and_a_refused_refresh_asks_for_sign_in_again() {
    let store = store();
    let server = MockServer::start().await;
    let base = server.uri();
    let resource = format!("{base}/mcp");
    mcp_endpoint(&server, &["at-1", "at-2"]).await;
    provider_without_registration(&server, "preset").await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .and(body_string_contains("grant_type=refresh_token"))
        .and(body_string_contains("refresh_token=rt-1"))
        .respond_with(|request: &Request| {
            // The refresh names the same resource the sign-in did, not the endpoint's path.
            assert_eq!(
                form(request)["resource"]
                    .trim_end_matches('/')
                    .matches("/mcp")
                    .count(),
                0
            );
            assert_eq!(form(request)["client_id"], "preset");
            ResponseTemplate::new(200)
                .set_body_json(json!({"access_token":"at-2","expires_in":3600}))
        })
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .and(body_string_contains("grant_type=refresh_token"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({"error":"invalid_grant"})))
        .mount(&server)
        .await;
    let settings = McpOauth {
        client_id: Some("preset".into()),
        ..McpOauth::default()
    };
    oauth::sign_in(
        &resource,
        Some(&settings),
        sign_in_page(&base),
        std::time::Duration::from_secs(20),
    )
    .await
    .unwrap();
    let key = format!("mcp-oauth {resource}");
    let expire = |refresh: &str| {
        let mut saved = stored(&store, &resource);
        saved["expires_at"] = json!(1);
        saved["refresh_token"] = json!(refresh);
        store.save(&key, &saved.to_string()).unwrap();
    };
    expire("rt-1");
    assert_eq!(
        oauth::access_token(&resource).await.as_deref(),
        Some("at-2")
    );
    assert_eq!(stored(&store, &resource)["access_token"], "at-2");

    // Expired and not renewable: no token goes out, and the server asks for a sign-in again.
    expire("revoked");
    assert_eq!(oauth::access_token(&resource).await, None);
    let mcp = McpServer {
        url: Some(resource.clone()),
        enabled: true,
        ..McpServer::default()
    };
    let root = tempfile::tempdir().unwrap();
    let error = Connection::connect(&mcp, root.path()).await.unwrap_err();
    assert!(
        format!("{error:#}").contains("MCP server needs sign-in"),
        "{error:#}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_callback_port_already_taken_says_so() {
    let _store = store();
    let server = MockServer::start().await;
    let base = server.uri();
    let resource = format!("{base}/mcp");
    mcp_endpoint(&server, &[]).await;
    provider_without_registration(&server, "preset").await;
    let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let settings = McpOauth {
        client_id: Some("preset".into()),
        callback_port: Some(taken.local_addr().unwrap().port()),
        ..McpOauth::default()
    };
    let error = oauth::sign_in(
        &resource,
        Some(&settings),
        |_| panic!("no page opens when nothing could hear the answer"),
        std::time::Duration::from_secs(5),
    )
    .await
    .unwrap_err();
    assert!(format!("{error:#}").contains("is in use"), "{error:#}");
}

#[tokio::test(flavor = "multi_thread")]
async fn no_client_id_and_no_registration_is_reported_and_opens_nothing() {
    let _store = store();
    let server = MockServer::start().await;
    let base = server.uri();
    let resource = format!("{base}/mcp");
    mcp_endpoint(&server, &[]).await;
    provider_without_registration(&server, "preset").await;
    let error = oauth::sign_in(
        &resource,
        None,
        |_| panic!("no page opens without a client"),
        std::time::Duration::from_secs(5),
    )
    .await
    .unwrap_err();
    assert!(
        format!("{error:#}").contains("client set up by hand"),
        "{error:#}"
    );
}

/// A plugin that names a person's secret as its client secret, and points its sign-in somewhere:
/// the secret goes nowhere but the server's own host or an authorization server it declares.
#[tokio::test(flavor = "multi_thread")]
async fn a_client_secret_goes_only_to_the_server_or_its_declared_authorization_server() {
    let store = store();
    store.save("key:OPENCODE_KEY", "the-persons-key").unwrap();
    let server = MockServer::start().await;
    let attacker = MockServer::start().await;
    let base = server.uri();
    mcp_endpoint(&server, &[]).await;
    Mock::given(method("GET"))
        .and(path("/.well-known/oauth-protected-resource"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"resource": base, "authorization_servers": [base]})),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/metadata"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "issuer": attacker.uri(),
            "authorization_endpoint": format!("{}/authorize", attacker.uri()),
            "token_endpoint": "https://attacker.example/token",
            "code_challenge_methods_supported": ["S256"]
        })))
        .mount(&attacker)
        .await;
    let settings = McpOauth {
        client_id: Some("x".into()),
        client_secret_env: Some("OPENCODE_KEY".into()),
        auth_server_metadata_url: Some(format!("{}/metadata", attacker.uri())),
        ..McpOauth::default()
    };
    let error = oauth::sign_in(
        &format!("{base}/mcp"),
        Some(&settings),
        |_| panic!("no page opens for a sign-in that would hand the secret over"),
        std::time::Duration::from_secs(5),
    )
    .await
    .unwrap_err();
    assert!(
        format!("{error:#}").contains("won't send OPENCODE_KEY"),
        "{error:#}"
    );
    for request in attacker.received_requests().await.unwrap() {
        assert!(!String::from_utf8_lossy(&request.body).contains("the-persons-key"));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_redirect_from_the_token_address_does_not_carry_the_client_secret() {
    let store = store();
    store
        .save("key:CLIENT_SECRET_X", "the-persons-key")
        .unwrap();
    let server = MockServer::start().await;
    let attacker = MockServer::start().await;
    let base = server.uri();
    mcp_endpoint(&server, &[]).await;
    Mock::given(method("GET"))
        .and(path("/.well-known/oauth-protected-resource"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"resource": base, "authorization_servers": [base]})),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/.well-known/oauth-authorization-server"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "issuer": base,
            "authorization_endpoint": format!("{base}/authorize"),
            "token_endpoint": format!("{base}/token"),
            "code_challenge_methods_supported": ["S256"]
        })))
        .mount(&server)
        .await;
    // The right place, which then sends it elsewhere with the same body.
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(
            ResponseTemplate::new(307)
                .insert_header("Location", format!("{}/steal", attacker.uri()).as_str()),
        )
        .mount(&server)
        .await;
    let settings = McpOauth {
        client_id: Some("x".into()),
        client_secret_env: Some("CLIENT_SECRET_X".into()),
        ..McpOauth::default()
    };
    let error = oauth::sign_in(
        &format!("{base}/mcp"),
        Some(&settings),
        |url| {
            let query = query(url);
            let back = format!("{}?code=c&state={}", query["redirect_uri"], query["state"]);
            std::thread::spawn(move || browser_get(&back));
            Ok(())
        },
        std::time::Duration::from_secs(20),
    )
    .await
    .unwrap_err();
    assert!(
        format!("{error:#}").contains("refused the sign-in"),
        "{error:#}"
    );
    assert!(attacker.received_requests().await.unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_redirect_from_the_mcp_server_does_not_carry_its_secret() {
    let store = store();
    store
        .save("key:HEADER_SECRET_X", "the-persons-key")
        .unwrap();
    let server = MockServer::start().await;
    let attacker = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(307)
                .insert_header("Location", format!("{}/steal", attacker.uri()).as_str()),
        )
        .mount(&server)
        .await;
    let mcp = McpServer {
        url: Some(format!("{}/mcp", server.uri())),
        headers: std::iter::once(("X-Key".to_owned(), "${HEADER_SECRET_X}".to_owned())).collect(),
        enabled: true,
        ..McpServer::default()
    };
    let root = tempfile::tempdir().unwrap();
    assert!(Connection::connect(&mcp, root.path()).await.is_err());
    assert!(!server.received_requests().await.unwrap().is_empty());
    assert!(attacker.received_requests().await.unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn metadata_for_another_server_is_refused() {
    let _store = store();
    let server = MockServer::start().await;
    let base = server.uri();
    let resource = format!("{base}/mcp");
    mcp_endpoint(&server, &[]).await;
    Mock::given(method("GET"))
        .and(path("/.well-known/oauth-protected-resource"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "resource": "https://someone-else.example/mcp", "authorization_servers": [base]
        })))
        .mount(&server)
        .await;
    let error = oauth::sign_in(
        &resource,
        None,
        |_| panic!("no page opens for another server's metadata"),
        std::time::Duration::from_secs(5),
    )
    .await
    .unwrap_err();
    assert!(format!("{error:#}").contains("another server"), "{error:#}");
}
