//! A tool call may take far longer than the protocol's own requests, as long as the server's
//! setting allows; talking to a real HTTP MCP server that answers slowly.
use serde_json::{json, Value};
use std::time::{Duration, Instant};
use themis_core::plugins::connections::{tool_call_deadline, Connection, McpServer};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

async fn answers_tool_calls_after(delay: Duration) -> MockServer {
    let server = MockServer::builder().start().await;
    Mock::given(method("POST"))
        .respond_with(move |request: &Request| {
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            let answer = match body.get("id") {
                Some(id) if body["method"] == "initialize" => {
                    json!({"jsonrpc":"2.0","id":id,"result":{"protocolVersion":"2025-03-26"}})
                }
                Some(id) => json!({"jsonrpc":"2.0","id":id,"result":{"content":[]}}),
                None => return ResponseTemplate::new(202),
            };
            let response = ResponseTemplate::new(200).set_body_json(answer);
            if body["method"] == "tools/call" {
                response.set_delay(delay)
            } else {
                response
            }
        })
        .mount(&server)
        .await;
    server
}

fn server(url: String, tool_timeout_secs: Option<u64>) -> McpServer {
    McpServer {
        url: Some(url),
        tool_timeout_secs,
        ..Default::default()
    }
}

#[test]
fn slow_tools_get_minutes_by_default_and_the_setting_wins() {
    assert_eq!(tool_call_deadline(None), Duration::from_secs(300));
    assert_eq!(tool_call_deadline(Some(45)), Duration::from_secs(45));
}

#[test]
fn the_setting_stays_within_bounds() {
    let url = "https://example.com/mcp".to_owned();
    assert!(server(url.clone(), Some(1)).validate().is_ok());
    assert!(server(url.clone(), Some(3600)).validate().is_ok());
    assert!(server(url.clone(), Some(0)).validate().is_err());
    assert!(server(url, Some(3601)).validate().is_err());
}

#[tokio::test]
async fn a_tool_call_ends_at_the_servers_setting() {
    let mock = answers_tool_calls_after(Duration::from_secs(20)).await;
    let root = tempfile::tempdir().unwrap();
    let mut connection =
        Connection::connect(&server(format!("{}/mcp", mock.uri()), Some(1)), root.path())
            .await
            .unwrap();
    let started = Instant::now();
    let error = connection
        .request("tools/call", json!({"name":"slow","arguments":{}}))
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "MCP request timed out");
    assert!(started.elapsed() < Duration::from_secs(10));
}

#[tokio::test]
async fn a_tool_call_inside_the_setting_returns_its_answer() {
    let mock = answers_tool_calls_after(Duration::from_secs(2)).await;
    let root = tempfile::tempdir().unwrap();
    let mut connection =
        Connection::connect(&server(format!("{}/mcp", mock.uri()), Some(5)), root.path())
            .await
            .unwrap();
    let answer = connection
        .request("tools/call", json!({"name":"slow","arguments":{}}))
        .await
        .unwrap();
    assert_eq!(answer["content"], json!([]));
}

/// A real local server: `pid` answers with its process id, `slow` never answers in time.
#[cfg(unix)]
mod local_server {
    use super::*;
    use std::sync::Arc;
    use themis_core::plugins::connections::Session;
    use themis_core::tools::AllowAllHook;

    const SERVER: &str = r#"
import json, os, subprocess, sys, time
for line in sys.stdin:
    message = json.loads(line)
    if "id" not in message:
        continue
    method = message["method"]
    if method == "initialize":
        result = {"protocolVersion": "2025-03-26", "capabilities": {}}
    elif method == "tools/list":
        schema = {"type": "object", "properties": {}}
        result = {"tools": [{"name": "pid", "inputSchema": schema}, {"name": "slow", "inputSchema": schema}, {"name": "helper", "inputSchema": schema}]}
    elif message["params"]["name"] == "helper":
        helper = subprocess.Popen(["sleep", "120"])
        result = {"content": [{"type": "text", "text": str(helper.pid)}]}
    elif message["params"]["name"] == "slow":
        time.sleep(30)
        result = {"content": []}
    else:
        result = {"content": [{"type": "text", "text": str(os.getpid())}]}
    print(json.dumps({"jsonrpc": "2.0", "id": message["id"], "result": result}), flush=True)
"#;

    async fn session(root: &std::path::Path) -> Session {
        std::fs::write(root.join("server.py"), SERVER).unwrap();
        let server = McpServer {
            command: Some("python3".into()),
            args: vec![root.join("server.py").display().to_string()],
            tool_timeout_secs: Some(1),
            ..Default::default()
        };
        Session::connect(&server, root).await.unwrap()
    }

    async fn call(session: &Session, remote: &str) -> Result<Value, String> {
        let (tools, _, names) = session.tools("fixture", Arc::new(AllowAllHook)).unwrap();
        let wire = names
            .iter()
            .find(|(_, name)| name == remote)
            .unwrap()
            .0
            .clone();
        let tool = tools.iter().find(|tool| tool.name() == wire).unwrap();
        tool.execute(json!({}))
            .await
            .map_err(|error| error.to_string())
    }

    #[tokio::test]
    async fn a_timed_out_server_comes_back_on_the_next_call_and_says_what_was_lost() {
        let root = tempfile::tempdir().unwrap();
        let session = session(root.path()).await;
        let first = call(&session, "pid").await.unwrap();
        assert_eq!(
            call(&session, "pid").await.unwrap(),
            first,
            "a working server is left alone"
        );

        let error = call(&session, "slow").await.unwrap_err();
        assert!(error.contains("restarts on your next call"), "{error}");
        assert!(error.contains("1 second"), "{error}");
        assert!(!session.is_connected(), "stopped until something needs it");

        let second = call(&session, "pid").await.unwrap();
        assert_ne!(first, second, "a new process answers");
        assert!(session.is_connected());
    }

    fn alive(pid: &str) -> bool {
        std::process::Command::new("kill")
            .args(["-0", pid])
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success()
    }

    /// Playwright's server runs a browser: stopping only the server would leave it running.
    #[tokio::test]
    async fn stopping_a_server_stops_what_it_started() {
        let root = tempfile::tempdir().unwrap();
        let session = session(root.path()).await;
        let helper = call(&session, "helper").await.unwrap();
        let helper = helper["content"][0]["text"].as_str().unwrap().to_owned();
        assert!(alive(&helper));

        call(&session, "slow").await.unwrap_err();
        for _ in 0..50 {
            if !alive(&helper) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(
            !alive(&helper),
            "the server's helper {helper} is still running"
        );
    }

    #[tokio::test]
    async fn closing_a_session_stops_what_its_server_started() {
        let root = tempfile::tempdir().unwrap();
        let session = session(root.path()).await;
        let helper = call(&session, "helper").await.unwrap();
        let helper = helper["content"][0]["text"].as_str().unwrap().to_owned();
        drop(session);
        for _ in 0..50 {
            if !alive(&helper) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(
            !alive(&helper),
            "the server's helper {helper} is still running"
        );
    }

    #[tokio::test]
    async fn a_server_that_exits_between_calls_comes_back_too() {
        let root = tempfile::tempdir().unwrap();
        let session = session(root.path()).await;
        let first = call(&session, "pid").await.unwrap();
        let pid = first["content"][0]["text"].as_str().unwrap().to_owned();
        std::process::Command::new("kill")
            .arg(&pid)
            .status()
            .unwrap();
        for _ in 0..50 {
            if !session.is_connected() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(!session.is_connected());
        assert_ne!(call(&session, "pid").await.unwrap(), first);
    }
}

#[test]
fn the_setting_comes_in_from_mcp_json() {
    let dir = tempfile::tempdir().unwrap();
    let store = themis_core::plugins::PluginStore::new(dir.path().into(), None);
    let input = r#"{"mcpServers":{"slow":{"command":"python3","tool_timeout_secs":900}}}"#;
    let spec = store.inspect_mcp_json("fixture", input).unwrap();
    assert_eq!(spec.spec.mcp["slow"].tool_timeout_secs, Some(900));
    let refused = r#"{"mcpServers":{"slow":{"command":"python3","tool_timeout_secs":0}}}"#;
    assert!(store.import_mcp_json("global", "fixture", refused).is_err());
}
