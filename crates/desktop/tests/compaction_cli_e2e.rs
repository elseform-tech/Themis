//! Shared server + `themis` CLI journeys through context compaction: a long chat compacts and
//! keeps working, `recall` cites a saved original after compaction, a greeting after a topic
//! switch neither compacts nor recaps, and a failed checkpoint leaves the working context
//! unchanged. The provider is scripted, so these check the wiring, not live model quality.

use serde_json::{json, Value};
use std::{
    path::Path,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
};
use themis_desktop::{
    server::{Client, Server},
    state::AppState,
};
use wiremock::{matchers::method, Mock, MockServer, Request, ResponseTemplate};

const SECRET: &str = "ORCHID-9931";
const CHECKPOINT: &str = "Goal/constraints: plan the garden; the user asked to remember a code early on (see originals). Unresolved work: finish the garden plan. Parked work: none. Next action: continue the garden plan if asked.";

fn cli(dir: &Path, command: &str, args: Value) -> Value {
    use std::io::Write;
    use std::process::Stdio;
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
        .arg("--data-dir")
        .arg(dir)
        .args(["call", command, "-"])
        .env_remove("OPENCODE_KEY")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(args.to_string().as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

/// Sends one message through the CLI and returns the terminal event plus every event kind.
async fn send(
    data: &Path,
    events: &mut tokio::net::TcpStream,
    id: &str,
    text: &str,
) -> (Value, Vec<String>) {
    cli(
        data,
        "send_message",
        json!({"threadId":id,"text":text,"reasoningEffort":null}),
    );
    tokio::time::timeout(std::time::Duration::from_secs(90), async {
        let mut kinds = Vec::new();
        loop {
            let item = Client::next_event(events).await.unwrap();
            if item["name"] != "thread-event" || item["payload"]["thread_id"] != id {
                continue;
            }
            let event = &item["payload"]["event"];
            let kind = event["kind"].as_str().unwrap_or_default().to_owned();
            kinds.push(kind.clone());
            if ["finished", "failed", "incomplete"].contains(&kind.as_str()) {
                return (event.clone(), kinds);
            }
        }
    })
    .await
    .unwrap()
}

fn is_summary(body: &Value) -> bool {
    body["messages"][0]["content"]
        .as_str()
        .is_some_and(|text| text.contains("You summarize agent context"))
}

fn text_reply(text: &str) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .set_body_json(json!({"choices":[{"message":{"content":text},"finish_reason":"stop"}]}))
}

/// Filler that pushes the 24k-token budget toward a checkpoint within a few messages.
fn filler(topic: &str, index: usize) -> String {
    format!(
        "{topic} note {index}: {}",
        format!("{topic} detail {index} about beds, soil and watering; ").repeat(220)
    )
}

async fn start(
    provider: &MockServer,
    data: &Path,
) -> (
    Client,
    tokio::net::TcpStream,
    tokio::task::JoinHandle<Result<(), String>>,
) {
    let state = AppState::new_for_test(data.join("settings.json"));
    state
        .set_secret("go".into(), "test-key".into())
        .await
        .unwrap();
    state.set_go_base_url_override(Some(provider.uri()));
    let server = Server::bind(state, data).await.unwrap();
    let serving = tokio::spawn(server.run());
    let client = Client::new(data.into());
    let events = client.subscribe().await.unwrap();
    (client, events, serving)
}

fn project() -> tempfile::TempDir {
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(project.path().join(".themis")).unwrap();
    std::fs::write(
        project.path().join(".themis/config.jsonc"),
        r#"{"context_token_budget":24000,"recent_messages":4,"total_turns":8}"#,
    )
    .unwrap();
    project
}

#[tokio::test(flavor = "multi_thread")]
async fn cli_long_chat_compacts_recalls_an_original_and_greets_without_recap() {
    let data = tempfile::tempdir().unwrap();
    let project = project();
    let provider = MockServer::start().await;
    let summaries = Arc::new(AtomicUsize::new(0));
    let ordinary = Arc::new(Mutex::new(Vec::<Value>::new()));
    let (count, sink) = (summaries.clone(), ordinary.clone());
    Mock::given(method("POST"))
        .respond_with(move |request: &Request| {
            let body: Value = request.body_json().unwrap();
            if is_summary(&body) {
                count.fetch_add(1, Ordering::SeqCst);
                // The checkpoint deliberately omits the planted code.
                return text_reply(CHECKPOINT);
            }
            sink.lock().unwrap().push(body.clone());
            let messages = body["messages"].as_array().unwrap();
            let last = messages.last().unwrap();
            if last["role"] == "tool" {
                let result: Value =
                    serde_json::from_str(last["content"].as_str().unwrap()).unwrap();
                let hit = result["hits"].as_array().and_then(|hits| {
                    hits.iter()
                        .find(|hit| hit["excerpt"].as_str().unwrap().contains(SECRET))
                });
                return text_reply(&match hit {
                    Some(hit) => format!(
                        "The launch code is {SECRET}.\nCitation: {} record {}",
                        hit["path"].as_str().unwrap(),
                        hit["record"]
                    ),
                    None => "UNKNOWN".to_owned(),
                });
            }
            let text = last["content"].as_str().unwrap_or_default();
            if text.contains("What was the launch code") {
                return ResponseTemplate::new(200).set_body_json(json!({"choices":[{"message":{"tool_calls":[{"id":"r1","type":"function","function":{"name":"recall","arguments":json!({"query":"launch code"}).to_string()}}]},"finish_reason":"tool_calls"}]}));
            }
            if text.trim() == "hi" {
                return text_reply("Hi! What would you like to do?");
            }
            text_reply("noted")
        })
        .mount(&provider)
        .await;
    let (client, mut events, serving) = start(&provider, data.path()).await;
    let thread = cli(
        data.path(),
        "create_thread",
        json!({"projectRoot":project.path(),"provider":"go","model":"test-model"}),
    );
    let id = thread["id"].as_str().unwrap().to_owned();

    // A long chat: an early detail, then enough material on two topics to compact.
    let (first, _) = send(
        data.path(),
        &mut events,
        &id,
        &format!("Remember for later: the launch code is {SECRET}."),
    )
    .await;
    assert_eq!(first["kind"], "finished", "{first}");
    let mut checkpoints = 0;
    for index in 0..8 {
        let topic = if index < 4 { "Release" } else { "Garden" };
        let (done, kinds) = send(data.path(), &mut events, &id, &filler(topic, index)).await;
        assert_eq!(done["kind"], "finished", "message {index}: {done}");
        checkpoints += kinds.iter().filter(|k| *k == "context_checkpoint").count();
    }
    assert!(checkpoints >= 1, "the long chat never compacted");
    assert!(summaries.load(Ordering::SeqCst) >= 1);
    // It keeps working after compaction: the latest request carries the checkpoint, not the
    // early message.
    let latest = ordinary.lock().unwrap().last().unwrap().clone();
    let wire = serde_json::to_string(&latest["messages"]).unwrap();
    assert!(wire.contains("Earlier context checkpoint"));
    assert!(
        !wire.contains(SECRET),
        "the early detail is only in originals"
    );

    // Recall retrieves the original with a citation that points at the planted record.
    let (answer, kinds) = send(
        data.path(),
        &mut events,
        &id,
        "What was the launch code I gave you at the start?",
    )
    .await;
    assert_eq!(answer["kind"], "finished", "{answer}");
    assert!(kinds.contains(&"tool_started".to_owned()), "{kinds:?}");
    let result = answer["result"].as_str().unwrap();
    assert!(result.contains(SECRET), "{result}");
    let citation = result.split("Citation: ").nth(1).unwrap();
    let (path, record) = citation.split_once(" record ").unwrap();
    let line = std::fs::read_to_string(project.path().join(path))
        .unwrap()
        .lines()
        .nth(record.trim().parse::<usize>().unwrap() - 1)
        .unwrap()
        .to_owned();
    let original: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(original["role"], "User");
    assert_eq!(original["evidence_kind"], "original");
    assert!(original["content"].as_str().unwrap().contains(SECRET));

    // A bare greeting after the topic switch: no checkpoint, no anchors, and the wire
    // system text asks for a reply in kind rather than a recap of parked work.
    let before = summaries.load(Ordering::SeqCst);
    let (greeting, kinds) = send(data.path(), &mut events, &id, "hi").await;
    assert_eq!(greeting["kind"], "finished", "{greeting}");
    assert_eq!(greeting["result"], "Hi! What would you like to do?");
    assert!(
        !kinds.contains(&"context_checkpoint".to_owned()),
        "{kinds:?}"
    );
    assert_eq!(summaries.load(Ordering::SeqCst), before);
    let request = ordinary.lock().unwrap().last().unwrap().clone();
    let system = request["messages"][0]["content"].as_str().unwrap();
    assert!(system.contains(
        "A greeting, thanks or small talk gets a short reply in kind: do not recap, summarize or resume earlier or parked work unless the user asks about it"
    ));
    let wire = serde_json::to_string(&request["messages"]).unwrap();
    assert!(!wire.contains("Original exchange anchors"), "{wire}");
    assert!(wire.contains(
        "mention or resume this work only when the user's message continues or asks about it"
    ));

    client.call("shutdown", json!({})).await.unwrap();
    serving.await.unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn cli_failed_checkpoint_keeps_the_working_context_and_the_next_run_still_has_it() {
    let data = tempfile::tempdir().unwrap();
    let project = project();
    let provider = MockServer::start().await;
    let summaries = Arc::new(Mutex::new(Vec::<Value>::new()));
    let sink = summaries.clone();
    Mock::given(method("POST"))
        .respond_with(move |request: &Request| {
            let body: Value = request.body_json().unwrap();
            if is_summary(&body) {
                let mut seen = sink.lock().unwrap();
                seen.push(body);
                // The first checkpoint request fails permanently; later ones succeed.
                if seen.len() == 1 {
                    return ResponseTemplate::new(400).set_body_json(
                        json!({"error":{"message":"bad request","type":"invalid_request_error"}}),
                    );
                }
                return text_reply(CHECKPOINT);
            }
            text_reply("noted")
        })
        .mount(&provider)
        .await;
    let (client, mut events, serving) = start(&provider, data.path()).await;
    let thread = cli(
        data.path(),
        "create_thread",
        json!({"projectRoot":project.path(),"provider":"go","model":"test-model"}),
    );
    let id = thread["id"].as_str().unwrap().to_owned();
    send(
        data.path(),
        &mut events,
        &id,
        &format!("Remember for later: the launch code is {SECRET}."),
    )
    .await;
    let mut failure = None;
    for index in 0..10 {
        let (done, kinds) = send(data.path(), &mut events, &id, &filler("Release", index)).await;
        if done["kind"] == "failed" {
            assert!(
                !kinds.contains(&"context_checkpoint".to_owned()),
                "{kinds:?}"
            );
            failure = Some(done);
            break;
        }
    }
    let failure = failure.expect("the first checkpoint request should fail the run");
    let error = failure["error"].as_str().unwrap();
    assert!(error.contains("working context unchanged"), "{error}");
    assert!(error.contains("Originals saved on disk"), "{error}");
    assert!(!error.contains("/retry-compaction"), "{error}");

    // The next run starts from the unchanged conversation: its checkpoint request still sees
    // the early original, which a lossy replacement would have dropped.
    let (next, kinds) = send(data.path(), &mut events, &id, "Continue the release notes.").await;
    assert_eq!(next["kind"], "finished", "{next}");
    assert!(
        kinds.contains(&"context_checkpoint".to_owned()),
        "{kinds:?}"
    );
    let seen = summaries.lock().unwrap().clone();
    assert!(seen.len() >= 2);
    assert!(serde_json::to_string(&seen[1]).unwrap().contains(SECRET));
    // No duplicate pending copy of the job input is left beside the saved originals.
    let evidence = project.path().join(".themis/context").join(&id);
    assert!(!std::fs::read_dir(&evidence).unwrap().any(|entry| entry
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with("pending-job-")));

    client.call("shutdown", json!({})).await.unwrap();
    serving.await.unwrap().unwrap();
}
