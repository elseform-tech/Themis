//! A reply with no tool call ends the turn only if it reads as an ending, through the shared
//! server and the `themis` CLI: one that announces a step or writes a tool call as text gets
//! one more request, so the step is taken; a closing reply ends the turn at once.

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use themis_desktop::{server::Server, state::AppState, types::ProviderKind};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const NOTE: &str = "[Runtime note, not from the user] Your last reply named a next step";
const SECRET: &str = "the seeded file says 7421";
const SUMMARY: &str = "I read notes.txt and it holds the number 7421.";

fn call(data: &Path, command: &str, args: Value) -> Value {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
        .arg("--data-dir")
        .arg(data)
        .args(["call", command, &args.to_string()])
        .env_remove("OPENCODE_KEY")
        .output()
        .expect("CLI");
    assert!(
        output.status.success(),
        "{command}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap_or(Value::Null)
}

fn reply(content: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "choices":[{"message":{"role":"assistant","content":content},"finish_reason":"stop"}]
    }))
}

fn read_notes(content: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "choices":[{"message":{
            "role":"assistant",
            "content":content,
            "tool_calls":[{"id":"r","type":"function","function":{"name":"read_file","arguments":"{\"file_path\":\"notes.txt\"}"}}]
        },"finish_reason":"tool_calls"}]
    }))
}

struct Run {
    history: Value,
    requests: Vec<Value>,
    log: String,
}

impl Run {
    fn events(&self, kind: &str) -> Vec<&Value> {
        self.history
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|item| {
                item.pointer("/envelope/event")
                    .filter(|event| event["kind"] == kind)
            })
            .collect()
    }

    fn shown(&self) -> String {
        self.events("assistant_text")
            .iter()
            .map(|event| event["text"].as_str().unwrap().to_owned())
            .collect()
    }

    fn result(&self) -> &str {
        let finished = self.events("finished");
        assert_eq!(finished.len(), 1, "{}", self.history);
        finished[0]["result"].as_str().unwrap()
    }

    fn notes_sent(&self) -> usize {
        self.requests
            .iter()
            .filter(|request| {
                request["messages"].as_array().unwrap().last().unwrap()["content"]
                    .as_str()
                    .is_some_and(|text| text.starts_with(NOTE))
            })
            .count()
    }

    /// Whether the seeded file's content came back to the model from the tool.
    fn tool_ran(&self) -> bool {
        self.requests.iter().any(|request| {
            request["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|message| {
                    message["role"] == "tool"
                        && message["content"]
                            .as_str()
                            .is_some_and(|text| text.contains(SECRET))
                })
        })
    }
}

/// One chat turn against a mock whose answer to the n-th request is `answer(n)`.
async fn run_turn<F>(answer: F) -> Run
where
    F: Fn(usize) -> ResponseTemplate + Send + Sync + 'static,
{
    let llm = MockServer::start().await;
    let served = Arc::new(AtomicUsize::new(0));
    Mock::given(method("POST"))
        .respond_with(move |_: &Request| answer(served.fetch_add(1, Ordering::SeqCst)))
        .mount(&llm)
        .await;
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    std::fs::write(project.path().join("notes.txt"), SECRET).unwrap();
    let state = AppState::new_for_test(data.path().join("settings.json"));
    state
        .set_secret("go".into(), "synthetic-test-key".into())
        .await
        .unwrap();
    state.set_go_base_url_override(Some(llm.uri()));
    let thread = state
        .create_thread(
            project.path().to_string_lossy().into(),
            ProviderKind::Go,
            Some("test-model".into()),
        )
        .await
        .unwrap();
    let server = Server::bind(state.clone(), data.path()).await.unwrap();
    let task = tokio::spawn(server.run());
    let data_dir = data.path().to_owned();
    let thread_id = thread.id.clone();
    let history = tokio::task::spawn_blocking(move || {
        call(
            &data_dir,
            "send_message",
            json!({"threadId": thread_id, "text": "What does notes.txt say?"}),
        );
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        loop {
            let history = call(
                &data_dir,
                "get_thread_history",
                json!({"threadId": thread_id}),
            );
            if history.to_string().contains("\"finished\"") {
                return history;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the run never finished: {history}"
            );
            std::thread::sleep(Duration::from_millis(200));
        }
    })
    .await
    .unwrap();
    let requests = llm
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(|request| request.body_json::<Value>().unwrap())
        .collect();
    let log =
        std::fs::read_to_string(data.path().join("logs/diagnostics-0.jsonl")).unwrap_or_default();
    task.abort();
    Run {
        history,
        requests,
        log,
    }
}

/// The first reply only announces a step; the next, sent back with the note, takes it.
#[tokio::test(flavor = "multi_thread")]
async fn cli_an_announced_step_gets_one_more_request_and_the_tool_runs() {
    const ANNOUNCED: &str = "Let me check the console next.";
    let run = run_turn(|request| match request {
        0 => reply(ANNOUNCED),
        1 => read_notes(""),
        _ => reply(SUMMARY),
    })
    .await;
    assert!(run.tool_ran(), "{}", run.history);
    let finished = run.events("tool_finished");
    assert_eq!(finished.len(), 1, "{}", run.history);
    assert_eq!(finished[0]["ok"], true, "{}", run.history);
    assert_eq!(run.notes_sent(), 1);
    assert_eq!(run.requests.len(), 3);
    // The announcement stays in what the model sees, right before the note.
    let sent_back = run.requests[1]["messages"].as_array().unwrap();
    assert_eq!(sent_back[sent_back.len() - 2]["content"], ANNOUNCED);
    assert_eq!(run.result(), SUMMARY);
    // Metadata only: the reason is logged, the model's words are not.
    let record = run
        .log
        .lines()
        .find(|line| line.contains("turn_end_guard"))
        .unwrap_or_else(|| panic!("no guard record in: {}", run.log));
    assert!(record.contains("\"announced\""), "{record}");
    assert!(!run.log.contains(ANNOUNCED), "{record}");
}

/// A model that says the same line again as it makes the call: the person sees it once.
#[tokio::test(flavor = "multi_thread")]
async fn cli_the_reply_to_the_note_does_not_show_the_line_twice() {
    const ANNOUNCED: &str = "Let us aim and shoot the damaged enemy.";
    let run = run_turn(|request| match request {
        0 => reply(ANNOUNCED),
        1 => read_notes(ANNOUNCED),
        _ => reply(SUMMARY),
    })
    .await;
    assert!(run.tool_ran(), "{}", run.history);
    assert_eq!(run.shown().matches(ANNOUNCED).count(), 1, "{}", run.shown());
    assert!(run.shown().contains(SUMMARY), "{}", run.shown());
    assert_eq!(run.result(), SUMMARY);
}

#[tokio::test(flavor = "multi_thread")]
async fn cli_a_tool_call_written_as_text_gets_one_more_request() {
    let run = run_turn(|request| match request {
        0 => reply("Reading it. <|tool_call_begin|>read_file<|tool_call_end|>"),
        1 => read_notes(""),
        _ => reply(SUMMARY),
    })
    .await;
    assert!(run.tool_ran(), "{}", run.history);
    assert_eq!(run.notes_sent(), 1);
    assert!(run.log.contains("\"written_call\""), "{}", run.log);
}

/// A model that keeps announcing and never calls a tool cannot loop.
#[tokio::test(flavor = "multi_thread")]
async fn cli_a_model_that_keeps_announcing_gets_one_extra_request_only() {
    const ANNOUNCED: &str = "Moving view to line up with that enemy now.";
    let run = run_turn(|_| reply(ANNOUNCED)).await;
    assert_eq!(run.requests.len(), 2);
    assert_eq!(run.notes_sent(), 1);
    assert_eq!(run.shown().matches(ANNOUNCED).count(), 1, "{}", run.shown());
    assert_eq!(run.result(), ANNOUNCED);
}

/// Each new stretch of work may be sent back once more.
#[tokio::test(flavor = "multi_thread")]
async fn cli_the_limit_is_per_stretch_of_work() {
    let run = run_turn(|request| match request {
        0 => reply("Let me check the notes."),
        1 => read_notes(""),
        _ => reply("Let me check the console next."),
    })
    .await;
    assert_eq!(run.notes_sent(), 2);
    assert_eq!(run.requests.len(), 4);
}

/// A closing reply ends the turn with the request it took, before and after tools.
#[tokio::test(flavor = "multi_thread")]
async fn cli_a_closing_reply_ends_the_turn() {
    for closing in [
        "I fixed the overlay and the score now rises.",
        "Done. Let me know if you need anything else.",
        "I'll leave that for you to decide.",
    ] {
        let run = run_turn(move |_| reply(closing)).await;
        assert_eq!((run.requests.len(), run.notes_sent()), (1, 0), "{closing}");
        assert_eq!(run.result(), closing);

        let run = run_turn(move |request| match request {
            0 => read_notes("Reading it."),
            _ => reply(closing),
        })
        .await;
        assert_eq!((run.requests.len(), run.notes_sent()), (2, 0), "{closing}");
        assert_eq!(run.result(), closing);
    }
}
