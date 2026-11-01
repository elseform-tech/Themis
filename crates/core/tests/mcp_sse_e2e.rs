//! The older MCP transport, HTTP with server-sent events, against a real socket: the server names
//! where to send messages on a stream it keeps open, and answers on that stream. What a secret was
//! allowed for is a place, so the address it names is held to the same host and port.

use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use themis_core::plugins::connections::{Connection, McpServer};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

#[derive(Default)]
struct Seen {
    stream: Option<TcpStream>,
    posts: Vec<(String, Value, String)>,
    stream_auth: String,
}

async fn read_request(socket: &mut TcpStream) -> Option<(String, String, Vec<u8>)> {
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 4096];
    let end = loop {
        if let Some(at) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break at;
        }
        let count = socket.read(&mut chunk).await.ok()?;
        if count == 0 {
            return None;
        }
        bytes.extend_from_slice(&chunk[..count]);
    };
    let head = String::from_utf8_lossy(&bytes[..end]).into_owned();
    let length = head
        .lines()
        .find_map(|line| {
            line.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .map(|v| v.trim().parse::<usize>().unwrap_or(0))
        })
        .unwrap_or(0);
    let mut body = bytes[end + 4..].to_vec();
    while body.len() < length {
        let count = socket.read(&mut chunk).await.ok()?;
        if count == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..count]);
    }
    let first = head.lines().next().unwrap_or_default().to_owned();
    Some((first, head, body))
}

/// A server of this transport. `endpoint` is what it names as where to send; `answers` it answers
/// (`false` leaves the stream silent, then closes it).
async fn sse_server(endpoint: &str, answers: bool) -> (String, Arc<Mutex<Seen>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(Seen::default()));
    let shared = Arc::clone(&seen);
    let endpoint = endpoint.to_owned();
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let (shared, endpoint) = (Arc::clone(&shared), endpoint.clone());
            tokio::spawn(async move {
                let Some((first, head, body)) = read_request(&mut socket).await else {
                    return;
                };
                if first.starts_with("GET ") {
                    let auth = head
                        .lines()
                        .find(|line| line.to_ascii_lowercase().starts_with("x-tag:"))
                        .unwrap_or_default()
                        .to_owned();
                    socket
                        .write_all(format!(
                            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n: comment\n\nevent: endpoint\ndata: {endpoint}\n\n"
                        ).as_bytes())
                        .await
                        .unwrap();
                    socket.flush().await.unwrap();
                    let mut seen = shared.lock().unwrap();
                    seen.stream_auth = auth;
                    seen.stream = Some(socket);
                    return;
                }
                let message: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
                let auth = head
                    .lines()
                    .find(|line| line.to_ascii_lowercase().starts_with("x-tag:"))
                    .unwrap_or_default()
                    .to_owned();
                let path = first
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or_default()
                    .to_owned();
                let id = message["id"].clone();
                let method = message["method"].as_str().unwrap_or_default().to_owned();
                let stream = {
                    let mut seen = shared.lock().unwrap();
                    seen.posts.push((path, message, auth));
                    seen.stream.take()
                };
                socket
                    .write_all(b"HTTP/1.1 202 Accepted\r\ncontent-length: 0\r\n\r\n")
                    .await
                    .unwrap();
                let Some(mut stream) = stream else { return };
                if !answers {
                    if method == "initialize" {
                        drop(stream);
                    } else {
                        shared.lock().unwrap().stream = Some(stream);
                    }
                    return;
                }
                let result = match method.as_str() {
                    "initialize" => json!({"protocolVersion": "2024-11-05", "capabilities": {}}),
                    "tools/list" => json!({"tools": [{"name": "lookup", "description": "d",
                        "inputSchema": {"type": "object"}}]}),
                    _ => json!({}),
                };
                if !id.is_null() {
                    // Something else on the stream first, split across writes, as servers do.
                    let note = json!({"jsonrpc": "2.0", "method": "notifications/progress"});
                    let answer = json!({"jsonrpc": "2.0", "id": id, "result": result});
                    let text = format!(
                        "event: message\ndata: {note}\n\nevent: message\ndata: {answer}\n\n"
                    );
                    let (one, two) = text.split_at(text.len() / 2);
                    stream.write_all(one.as_bytes()).await.unwrap();
                    stream.flush().await.unwrap();
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                    stream.write_all(two.as_bytes()).await.unwrap();
                    stream.flush().await.unwrap();
                }
                shared.lock().unwrap().stream = Some(stream);
            });
        }
    });
    (address, seen)
}

fn server(url: String) -> McpServer {
    McpServer {
        url: Some(url),
        sse: true,
        header_values: [("x-tag".to_owned(), "themis".to_owned())].into(),
        ..McpServer::default()
    }
}

#[tokio::test]
async fn it_connects_lists_tools_and_answers_on_the_stream() {
    let (address, seen) = sse_server("/messages?session=abc", true).await;
    let root = tempfile::tempdir().unwrap();
    let mut connection = Connection::connect(&server(format!("{address}/sse")), root.path())
        .await
        .unwrap();
    let tools = connection.tools().await.unwrap();
    assert_eq!(tools[0]["name"], "lookup");
    let posts = seen.lock().unwrap().posts.clone();
    assert_eq!(posts[0].1["method"], "initialize");
    assert_eq!(posts[1].1["method"], "notifications/initialized");
    assert_eq!(posts[2].1["method"], "tools/list");
    for (path, _, _) in &posts {
        assert_eq!(
            path, "/messages?session=abc",
            "the address it named, as named"
        );
    }
}

/// A stream that ends mid-request fails it, rather than waiting out the clock.
#[tokio::test]
async fn a_stream_that_ends_fails_the_request_at_once() {
    let (address, _) = sse_server("/messages", false).await;
    let root = tempfile::tempdir().unwrap();
    let started = std::time::Instant::now();
    let error = Connection::connect(&server(format!("{address}/sse")), root.path())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("disconnected"), "{error:#}");
    assert!(started.elapsed() < std::time::Duration::from_secs(10));
}

/// The address the server names is a place too: a secret allowed for this server isn't to reach
/// another. Nothing is sent to one, whatever way it is written.
#[tokio::test]
async fn it_never_posts_to_an_address_off_the_server() {
    let elsewhere = MockServer::builder().start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(202))
        .mount(&elsewhere)
        .await;
    let other_port = format!("{}/messages", elsewhere.uri());
    for endpoint in [
        other_port.as_str(),
        "https://evil.example/messages",
        "//evil.example/messages",
        "http://user:pass@127.0.0.1/messages",
    ] {
        let (address, seen) = sse_server(endpoint, true).await;
        let root = tempfile::tempdir().unwrap();
        let error = Connection::connect(&server(format!("{address}/sse")), root.path())
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("leaves the server"),
            "{endpoint}: {error:#}"
        );
        assert!(seen.lock().unwrap().posts.is_empty(), "{endpoint}");
    }
    assert!(elsewhere.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn a_sign_in_challenge_a_redirect_and_a_plain_answer_are_each_what_they_are() {
    let root = tempfile::tempdir().unwrap();
    let challenge = MockServer::builder().start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(401).insert_header("www-authenticate", "Bearer realm=\"OAuth\""),
        )
        .mount(&challenge)
        .await;
    let error = Connection::connect(&server(format!("{}/sse", challenge.uri())), root.path())
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "MCP server needs sign-in");

    let moved = MockServer::builder().start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(307).insert_header("location", "https://evil.example/sse"),
        )
        .mount(&moved)
        .await;
    let error = Connection::connect(&server(format!("{}/sse", moved.uri())), root.path())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("307"), "{error:#}");
    assert_eq!(moved.received_requests().await.unwrap().len(), 1);

    let plain = MockServer::builder().start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("hello"))
        .mount(&plain)
        .await;
    let error = Connection::connect(&server(format!("{}/sse", plain.uri())), root.path())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("event stream"), "{error:#}");
}

/// What a definition says to send goes with the stream and with each message alike.
#[tokio::test]
async fn a_header_reaches_the_stream_and_the_messages() {
    let (address, seen) = sse_server("/messages", true).await;
    let root = tempfile::tempdir().unwrap();
    let mcp = server(format!("{address}/sse"));
    let mut connection = Connection::connect(&mcp, root.path()).await.unwrap();
    connection.tools().await.unwrap();
    let seen = seen.lock().unwrap();
    assert!(seen.stream_auth.contains("themis"), "{}", seen.stream_auth);
    assert!(seen
        .posts
        .iter()
        .all(|(_, _, auth)| auth.contains("themis")));
}
