//! Plain header values, harmless queries, a URL that starts with a key and a key's default
//! all reach a real HTTP MCP server as written; a URL whose key leads to plain HTTP elsewhere
//! is refused before anything is sent.
use serde_json::{json, Value};
use std::collections::BTreeMap;
use themis_core::plugins::connections::{Connection, McpServer};
use wiremock::matchers::{header, method, query_param};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

async fn mcp() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(header("x-exa-source", "claude-code-plugin"))
        .and(query_param("client", "claude-code-plugin"))
        .respond_with(|request: &Request| {
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            match body.get("id") {
                Some(id) if body["method"] == "tools/list" => ResponseTemplate::new(200)
                    .set_body_json(json!({"jsonrpc":"2.0","id":id,"result":{"tools":[]}})),
                Some(id) => ResponseTemplate::new(200).set_body_json(json!({"jsonrpc":"2.0","id":id,"result":{"protocolVersion":"2025-06-18","capabilities":{},"serverInfo":{"name":"exa","version":"1"}}})),
                None => ResponseTemplate::new(202),
            }
        })
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(403))
        .mount(&server)
        .await;
    server
}

fn server(url: String, defaults: BTreeMap<String, String>) -> McpServer {
    McpServer {
        url: Some(url),
        header_values: BTreeMap::from([("x-exa-source".into(), "claude-code-plugin".into())]),
        defaults,
        ..Default::default()
    }
}

#[tokio::test]
async fn plain_values_and_url_keys_reach_the_server() {
    let mock = mcp().await;
    let root = tempfile::tempdir().unwrap();

    let direct = server(
        format!("{}/mcp?client=claude-code-plugin", mock.uri()),
        BTreeMap::new(),
    );
    direct.validate().unwrap();
    assert!(Connection::connect(&direct, root.path()).await.is_ok());

    // `${THEMIS_TEST_MCP_BASE}` is unset, so its default is used.
    let keyed = server(
        "${THEMIS_TEST_MCP_BASE}/mcp?client=claude-code-plugin".into(),
        BTreeMap::from([("THEMIS_TEST_MCP_BASE".into(), mock.uri())]),
    );
    keyed.validate().unwrap();
    assert_eq!(keyed.key_names(), ["THEMIS_TEST_MCP_BASE"]);
    assert!(Connection::connect(&keyed, root.path()).await.is_ok());

    let elsewhere = server(
        "${THEMIS_TEST_MCP_BASE}/mcp".into(),
        BTreeMap::from([("THEMIS_TEST_MCP_BASE".into(), "http://example.com".into())]),
    );
    let error = Connection::connect(&elsewhere, root.path())
        .await
        .expect_err("plain HTTP to another host is refused");
    assert!(error.to_string().contains("Use HTTPS"), "{error}");
}
