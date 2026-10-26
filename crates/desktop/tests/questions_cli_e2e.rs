//! Questions to the user (`ask_user`) through the real shared server and the
//! `themis` CLI, with a scripted model: a question that keeps the run going, an
//! answer during the run and after it, a repeated topic, and in goal mode a
//! blocking question that makes the goal wait for its answer only.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use themis_desktop::{server::Server, state::AppState, types::ProviderKind};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const CHECK_MARKER: &str = "You decide whether an agent has reached a goal";
const ANSWER_MARKER: &str = "[Answer to your question — from the user]";
const REACHED_TURN: &str = "A separate check found the goal reached";
const OPEN_QUESTIONS: &str = "Open questions you asked the user";
const DEADLINE: Duration = Duration::from_secs(60);

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

/// Waits (polling the CLI) until `done` holds.
fn until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + DEADLINE;
    while !done() {
        assert!(Instant::now() < deadline, "{what}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[derive(Clone, Default)]
struct Gate(Arc<AtomicBool>);

impl Gate {
    fn open(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
    fn wait(&self) {
        let waited = Instant::now();
        while !self.0.load(Ordering::SeqCst) && waited.elapsed() < DEADLINE {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

fn reply(text: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "choices": [{"message": {"role": "assistant", "content": text}, "finish_reason": "stop"}]
    }))
}

fn tools(calls: &[(&str, &str, Value)]) -> ResponseTemplate {
    let calls: Vec<Value> = calls
        .iter()
        .map(|(id, name, args)| json!({"id": id, "type": "function", "function": {"name": name, "arguments": args.to_string()}}))
        .collect();
    ResponseTemplate::new(200).set_body_json(json!({
        "choices": [{"message": {"role": "assistant", "content": null, "tool_calls": calls}, "finish_reason": "tool_calls"}]
    }))
}

fn messages(body: &Value) -> &[Value] {
    body["messages"].as_array().map_or(&[], Vec::as_slice)
}

fn is_check(body: &Value) -> bool {
    messages(body)
        .first()
        .and_then(|m| m["content"].as_str())
        .is_some_and(|text| text.contains(CHECK_MARKER))
}

/// The last user message, and how many tool results came after it.
fn last_user(body: &Value) -> (String, usize) {
    let all = messages(body);
    let index = all.iter().rposition(|m| m["role"] == "user").unwrap_or(0);
    let text = all
        .get(index)
        .and_then(|m| m["content"].as_str())
        .unwrap_or_default()
        .to_owned();
    let results = all[index..].iter().filter(|m| m["role"] == "tool").count();
    (text, results)
}

fn mentions(body: &Value, needle: &str) -> bool {
    messages(body).iter().any(|m| {
        m["content"]
            .as_str()
            .is_some_and(|text| text.contains(needle))
    })
}

async fn bodies(llm: &MockServer) -> Vec<Value> {
    llm.received_requests()
        .await
        .unwrap()
        .iter()
        .map(|request| request.body_json().unwrap_or(Value::Null))
        .collect()
}

struct App {
    llm: MockServer,
    data: tempfile::TempDir,
    _project: tempfile::TempDir,
    thread: String,
    serving: tokio::task::JoinHandle<()>,
}

async fn app(respond: impl Fn(&Value) -> ResponseTemplate + Send + Sync + 'static) -> App {
    let llm = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(move |request: &Request| respond(&request.body_json().unwrap_or(Value::Null)))
        .mount(&llm)
        .await;
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
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
        .unwrap()
        .id;
    let server = Server::bind(state, data.path()).await.unwrap();
    let serving = tokio::spawn(async move {
        let _ = server.run().await;
    });
    App {
        llm,
        data,
        _project: project,
        thread,
        serving,
    }
}

fn running(data: &Path, thread: &str) -> bool {
    call(data, "get_thread", json!({"threadId": thread}))["running"] == true
}

#[tokio::test(flavor = "multi_thread")]
async fn a_question_keeps_the_run_going_and_its_answer_arrives_during_or_after_it() {
    let gate = Gate::default();
    let held = gate.clone();
    let app = app(move |body| {
        let (text, results) = last_user(body);
        if text.starts_with(ANSWER_MARKER) {
            return reply("Switched to the Apache license.");
        }
        if text.contains("add a license") {
            return if results == 0 {
                tools(&[("l1", "ask_user", json!({"question": "Which license?", "recommended": "MIT", "why": "the usual choice", "topic": "license"}))])
            } else {
                reply("Added an MIT license.")
            };
        }
        if text.contains(ANSWER_MARKER) || mentions(body, ANSWER_MARKER) {
            return reply("Re-indented with tabs.");
        }
        match results {
            0 => tools(&[("q1", "ask_user", json!({"question": "Tabs or spaces?", "choices": ["Tabs", "Spaces"], "recommended": "Spaces", "why": "the usual choice", "topic": "indent"}))]),
            // Asked again on the same open topic: the question is updated, not added.
            1 => tools(&[("q2", "ask_user", json!({"question": "Tabs or spaces for indentation?", "choices": ["Tabs", "Spaces"], "recommended": "Spaces", "why": "the usual choice", "topic": "indent"}))]),
            _ => {
                // The run keeps working while the user answers.
                held.wait();
                tools(&[("d1", "list_dir", json!({"path": "."}))])
            }
        }
    })
    .await;
    let (data, thread) = (app.data.path().to_owned(), app.thread.clone());
    tokio::task::spawn_blocking({
        let (data, thread) = (data.clone(), thread.clone());
        move || {
            call(
                &data,
                "set_thread_approval_mode",
                json!({"threadId": thread, "mode": "yolo"}),
            );
            call(
                &data,
                "send_message",
                json!({"threadId": thread, "text": "format the file"}),
            );
            let mut questions = Value::Null;
            until("the question was never asked twice", || {
                questions = call(&data, "list_questions", json!({"threadId": thread}));
                questions[0]["question"] == "Tabs or spaces for indentation?"
            });
            assert_eq!(
                questions.as_array().unwrap().len(),
                1,
                "one topic, one question: {questions}"
            );
            assert_eq!(questions[0]["assumption"], "Spaces");
            // The recommended choice comes first.
            assert_eq!(questions[0]["choices"], json!(["Spaces", "Tabs"]));
            assert_eq!(questions[0]["reasons"]["Spaces"], "the usual choice");
            assert!(running(&data, &thread), "the question did not stop the run");
            let id = questions[0]["id"].as_str().unwrap().to_owned();
            let answered = call(
                &data,
                "answer_question",
                json!({"threadId": thread, "questionId": id, "answer": "Tabs"}),
            );
            assert_eq!(answered["answer"], "Tabs");
            gate.open();
            until("the run never finished", || !running(&data, &thread));
        }
    })
    .await
    .unwrap();
    let all = bodies(&app.llm).await;
    let answered: Vec<&Value> = all
        .iter()
        .filter(|body| mentions(body, ANSWER_MARKER))
        .collect();
    assert!(
        answered
            .iter()
            .any(|body| mentions(body, "Answer: Tabs") && mentions(body, "You assumed: Spaces")),
        "the answer reached the running run"
    );
    // Delivered once: as a steer, not also as a later message.
    let starts: Vec<_> = all
        .iter()
        .filter(|body| last_user(body).0.starts_with(ANSWER_MARKER))
        .collect();
    assert!(starts.is_empty(), "no extra run for an answer the run read");

    // An answer after the run ends goes out as the next message.
    tokio::task::spawn_blocking({
        let (data, thread) = (data.clone(), thread.clone());
        move || {
            call(
                &data,
                "send_message",
                json!({"threadId": thread, "text": "add a license"}),
            );
            until("the license run never finished", || {
                !running(&data, &thread)
            });
            let questions = call(&data, "list_questions", json!({"threadId": thread}));
            let open: Vec<&Value> = questions
                .as_array()
                .unwrap()
                .iter()
                .filter(|q| q["answer"].is_null())
                .collect();
            assert_eq!(open.len(), 1, "{questions}");
            let id = open[0]["id"].as_str().unwrap().to_owned();
            call(
                &data,
                "answer_question",
                json!({"threadId": thread, "questionId": id.clone(), "answer": "Apache-2.0"}),
            );
            until("the answer never ran", || {
                !running(&data, &thread)
                    && call(&data, "get_thread_history", json!({"threadId": thread}))
                        .to_string()
                        .contains("Switched to the Apache license.")
            });
            // Answering twice is refused.
            let again = std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
                .arg("--data-dir")
                .arg(&data)
                .args([
                    "call",
                    "answer_question",
                    &json!({"threadId": thread, "questionId": id, "answer": "MIT"}).to_string(),
                ])
                .output()
                .unwrap();
            assert!(!again.status.success());
        }
    })
    .await
    .unwrap();
    let all = bodies(&app.llm).await;
    assert!(all.iter().any(|body| {
        let (text, _) = last_user(body);
        text.starts_with(ANSWER_MARKER)
            && text.contains("Question: Which license?")
            && text.contains("Answer: Apache-2.0")
    }));
    // The chat's history keeps both questions and their answers.
    let history = tokio::task::spawn_blocking(move || {
        call(&data, "get_thread_history", json!({"threadId": thread})).to_string()
    })
    .await
    .unwrap();
    assert!(history.contains("question_asked") && history.contains("question_answered"));
    app.serving.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_blocking_question_ends_the_run_and_the_goal_waits_for_that_answer_only() {
    let app = app(|body| {
        if is_check(body) {
            return reply(r#"{"verdict":"reached","reason":"The key is configured.","progress":"configured"}"#);
        }
        let (text, results) = last_user(body);
        if text.contains(REACHED_TURN) {
            return reply("I've reached the goal; I assumed MIT for the license.");
        }
        if text.starts_with(ANSWER_MARKER) {
            return reply("Noted your answer.");
        }
        if text.contains("[Goal — runtime, not from the user] Turn") {
            return reply("Configured the key.");
        }
        if results == 0 {
            return tools(&[
                ("a", "ask_user", json!({"question": "Dark mode by default?", "recommended": "Yes", "why": "the usual choice"})),
                ("b", "ask_user", json!({"question": "Which license?", "recommended": "MIT", "why": "the usual choice"})),
                ("c", "ask_user", json!({"question": "Which API key name?", "blocking": true})),
            ]);
        }
        reply("THIS RUN SHOULD HAVE ENDED")
    })
    .await;
    let (data, thread) = (app.data.path().to_owned(), app.thread.clone());
    tokio::task::spawn_blocking({
        let (data, thread) = (data.clone(), thread.clone());
        move || {
            call(&data, "send_message", json!({"threadId": thread, "text": "/goal configure the API key"}));
            until("the goal never waited", || {
                call(&data, "get_goal", json!({"threadId": thread}))["status"] == "waiting"
            });
            let goal = call(&data, "get_goal", json!({"threadId": thread}));
            assert_eq!(goal["reason"], "Which API key name?", "{goal}");
            let questions = call(&data, "list_questions", json!({"threadId": thread}));
            let id = |text: &str| {
                questions.as_array().unwrap().iter().find(|q| q["question"] == text).unwrap()["id"].as_str().unwrap().to_owned()
            };
            // Another question's answer runs, but the goal keeps waiting.
            call(&data, "answer_question", json!({"threadId": thread, "questionId": id("Dark mode by default?"), "answer": "No"}));
            until("the answer run never finished", || {
                !running(&data, &thread)
                    && call(&data, "get_thread_history", json!({"threadId": thread})).to_string().contains("Noted your answer.")
            });
            assert_eq!(call(&data, "get_goal", json!({"threadId": thread}))["status"], "waiting");
            // The blocking question's answer resumes it.
            call(&data, "answer_question", json!({"threadId": thread, "questionId": id("Which API key name?"), "answer": "THEMIS_KEY"}));
            until("the goal never reached", || {
                call(&data, "get_goal", json!({"threadId": thread}))["status"] == "reached" && !running(&data, &thread)
                    && call(&data, "get_thread_history", json!({"threadId": thread})).to_string().contains("I've reached the goal")
            });
        }
    })
    .await
    .unwrap();
    let all = bodies(&app.llm).await;
    assert!(
        !all.iter()
            .any(|body| last_user(body).1 > 0 && last_user(body).0.contains("/goal")),
        "the blocking question ended its run"
    );
    let history = tokio::task::spawn_blocking({
        let (data, thread) = (data.clone(), thread.clone());
        move || call(&data, "get_thread_history", json!({"threadId": thread})).to_string()
    })
    .await
    .unwrap();
    assert!(history.contains("I need your answer to the question above before I can go on."));
    assert!(!history.contains("THIS RUN SHOULD HAVE ENDED"));
    // The goal turn and the reached turn both list the question still open.
    let goal_turn = all
        .iter()
        .find(|body| {
            last_user(body)
                .0
                .contains("[Goal — runtime, not from the user] Turn 2.")
        })
        .expect("goal turn 2");
    let (turn_text, _) = last_user(goal_turn);
    assert!(
        turn_text.contains(OPEN_QUESTIONS) && turn_text.contains("Which license? (assuming: MIT)"),
        "{turn_text}"
    );
    assert!(
        !turn_text.contains("Dark mode by default?"),
        "answered questions are not listed"
    );
    let reached = all
        .iter()
        .find(|body| last_user(body).0.contains(REACHED_TURN))
        .expect("reached turn");
    assert!(last_user(reached)
        .0
        .contains("Which license? (assuming: MIT)"));
    app.serving.abort();
}

/// In YOLO a deny rule for `ask_user` still holds, as for every other tool: the
/// question is not asked and the run goes on.
#[tokio::test(flavor = "multi_thread")]
async fn yolo_keeps_a_deny_rule_for_ask_user() {
    let app = app(|body| {
        if last_user(body).1 == 0 {
            tools(&[(
                "q",
                "ask_user",
                json!({"question": "Tabs?", "recommended": "Spaces", "why": "usual"}),
            )])
        } else {
            reply("Went on without asking.")
        }
    })
    .await;
    let (data, thread) = (app.data.path().to_owned(), app.thread.clone());
    std::fs::write(
        data.join("runtime.jsonc"),
        r#"{"approval":{"rules":[{"tool":"ask_user","action":"deny"}]}}"#,
    )
    .unwrap();
    tokio::task::spawn_blocking(move || {
        call(
            &data,
            "set_thread_approval_mode",
            json!({"threadId": thread, "mode": "yolo"}),
        );
        call(
            &data,
            "send_message",
            json!({"threadId": thread, "text": "format the file"}),
        );
        until("the run never finished", || !running(&data, &thread));
        let questions = call(&data, "list_questions", json!({"threadId": thread}));
        assert_eq!(questions, json!([]), "{questions}");
    })
    .await
    .unwrap();
    app.serving.abort();
}
