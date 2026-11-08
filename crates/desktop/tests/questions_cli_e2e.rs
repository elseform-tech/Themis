//! Questions to the user (`ask_user`) through the real shared server and the
//! `themis` CLI, with a scripted model: a question that keeps the run going, an
//! answer during the run and after it, a repeated topic, and in goal mode a
//! blocking question that makes the goal wait for its answer only. Several
//! questions in one call (pages of one box, answered together), choices that
//! must say what they do, and the question the agent asks after the person's Deny.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use themis_desktop::{server::Server, state::AppState, types::ProviderKind};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const ANSWER_MARKER: &str = "[Answer to your question — from the user]";
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

/// What a choice says besides its answer.
fn choice_details(answer: &str) -> Value {
    json!({"does": format!("re-indent with {answer}"), "advantages": format!("{answer} read well"), "disadvantages": format!("{answer} churn the diff"), "consequences": "easy to undo"})
}

/// A choice as `ask_user` takes it: the answer and all it means.
fn choice(answer: &str) -> Value {
    let mut choice = choice_details(answer);
    choice["answer"] = json!(answer);
    choice
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
    let llm = MockServer::builder().start().await;
    Mock::given(method("POST"))
        .respond_with(move |request: &Request| respond(&request.body_json().unwrap_or(Value::Null)))
        .mount(&llm)
        .await;
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    // A Git project, so Custom asks before a write (the after-Deny case).
    std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(project.path())
        .status()
        .expect("git");
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
            0 => tools(&[("q1", "ask_user", json!({"question": "Tabs or spaces?", "choices": [choice("Tabs"), choice("Spaces")], "recommended": "Spaces", "why": "the usual choice", "topic": "indent"}))]),
            // Asked again on the same open topic: the question is updated, not added.
            1 => tools(&[("q2", "ask_user", json!({"question": "Tabs or spaces for indentation?", "choices": [choice("Tabs"), choice("Spaces")], "recommended": "Spaces", "why": "the usual choice", "topic": "indent"}))]),
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
            // Each choice keeps what it does, its advantages, disadvantages and consequences.
            assert_eq!(questions[0]["details"]["Tabs"], choice_details("Tabs"));
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
    // With the options the agent offered and their reasons, so the decision keeps its details.
    assert!(
        answered.iter().any(|body| mentions(
            body,
            "Options you offered: Spaces (recommended: the usual choice): re-indent with Spaces; Tabs: re-indent with Tabs"
        )),
        "the answer names the options"
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
        let (text, results) = last_user(body);
        if text.starts_with(ANSWER_MARKER) {
            return reply("Noted your answer.");
        }
        if text.contains("[Goal — runtime, not from the user] Turn") {
            return if results == 0 {
                tools(&[(
                    "done",
                    "update_goal",
                    json!({"status": "complete", "evidence": [{"requirement": "the API key is configured", "proof": ".env sets THEMIS_KEY"}]}),
                )])
            } else {
                reply("I've reached the goal; I assumed MIT for the license.")
            };
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
    let root = app._project.path().to_string_lossy().into_owned();
    tokio::task::spawn_blocking({
        let (data, thread) = (data.clone(), thread.clone());
        move || {
            // The chat list carries what the sidebar's "Waiting for you" needs without the
            // chat's history: open question boxes and where the goal stands.
            let listed = |data: &std::path::Path| {
                call(data, "list_threads", json!({"projectRoot": root}))
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|info| info["id"] == thread.as_str())
                    .cloned()
                    .unwrap()
            };
            assert!(listed(&data).get("open_questions").is_none(), "no questions yet");
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
            let info = listed(&data);
            assert_eq!(info["open_questions"], 3, "{info}");
            assert_eq!(info["goal_status"], "waiting", "{info}");
            // Another question's answer runs, but the goal keeps waiting.
            call(&data, "answer_question", json!({"threadId": thread, "questionId": id("Dark mode by default?"), "answer": "No"}));
            assert_eq!(listed(&data)["open_questions"], 2);
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
            let info = listed(&data);
            assert_eq!(info["goal_status"], "reached", "{info}");
            assert_eq!(info["open_questions"], 1, "the license question is still unanswered: {info}");
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
    // The goal turn lists the question still open.
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

/// A reworded second question learns what is still open, and a message the person writes in
/// the chat instead of the box answers both: they close, and the agent is told which ones.
#[tokio::test(flavor = "multi_thread")]
async fn a_chat_message_answers_open_questions_and_a_new_question_sees_what_is_open() {
    let app = app(|body| {
        let (text, results) = last_user(body);
        if text.contains("Go with playful") {
            return reply("Playful it is.");
        }
        match results {
            0 => tools(&[(
                "q1",
                "ask_user",
                json!({"question": "Which tone for the README?", "recommended": "friendly", "why": "versatile"}),
            )]),
            1 => tools(&[(
                "q2",
                "ask_user",
                json!({"question": "What style should CONTRIBUTING use?", "recommended": "match the README", "why": "consistent"}),
            )]),
            _ => reply("Drafting on my assumptions."),
        }
    })
    .await;
    let (data, thread) = (app.data.path().to_owned(), app.thread.clone());
    let ask = data.clone();
    let asked = thread.clone();
    tokio::task::spawn_blocking(move || {
        call(
            &ask,
            "send_message",
            json!({"threadId": asked, "text": "write the docs"}),
        );
        until("the first run never finished", || !running(&ask, &asked));
        let open = call(&ask, "list_questions", json!({"threadId": asked}));
        assert_eq!(open.as_array().map(Vec::len), Some(2), "{open}");
        call(
            &ask,
            "send_message",
            json!({"threadId": asked, "text": "Go with playful"}),
        );
        until("the answer run never finished", || !running(&ask, &asked));
        let questions = call(&ask, "list_questions", json!({"threadId": asked}));
        for question in questions.as_array().unwrap() {
            assert_eq!(question["answer"], "Go with playful", "{questions}");
        }
    })
    .await
    .unwrap();
    let all = bodies(&app.llm).await;
    // The second question's result named the first as still open.
    let second = all
        .iter()
        .find(|body| last_user(body).1 == 2)
        .expect("the turn after the second question");
    let note = messages(second)
        .iter()
        .rev()
        .find(|m| m["role"] == "tool")
        .and_then(|m| m["content"].as_str())
        .unwrap_or_default();
    assert!(
        note.contains("Still open") && note.contains("Which tone for the README?"),
        "{note}"
    );
    // The chat answer reached the agent with the questions it closed.
    let answered = all
        .iter()
        .find(|body| last_user(body).0.contains("Go with playful"))
        .expect("the answer turn");
    let (text, _) = last_user(answered);
    assert!(text.contains("they are now closed"), "{text}");
    assert!(
        text.contains("Which tone for the README?")
            && text.contains("What style should CONTRIBUTING use?"),
        "{text}"
    );
    assert!(!mentions(answered, ANSWER_MARKER));
    app.serving.abort();
}

/// A message steered into a running run (Enter) answers the open question too: it closes, and
/// the run reads which question the message answered right after the message itself.
#[tokio::test(flavor = "multi_thread")]
async fn a_steered_message_answers_an_open_question() {
    let gate = Gate::default();
    let held = gate.clone();
    let app = app(move |body| {
        if mentions(body, "they are now closed") {
            return reply("Using tabs as you said.");
        }
        match last_user(body).1 {
            0 => tools(&[(
                "q1",
                "ask_user",
                json!({"question": "Tabs or spaces?", "recommended": "Spaces", "why": "usual"}),
            )]),
            _ => {
                held.wait();
                tools(&[("r1", "read_file", json!({"file_path": "missing.txt"}))])
            }
        }
    })
    .await;
    let (data, thread) = (app.data.path().to_owned(), app.thread.clone());
    tokio::task::spawn_blocking(move || {
        call(
            &data,
            "send_message",
            json!({"threadId": thread, "text": "format the file"}),
        );
        until("the question was never asked", || {
            call(&data, "list_questions", json!({"threadId": thread}))
                .as_array()
                .is_some_and(|open| !open.is_empty())
        });
        let queued = call(
            &data,
            "enqueue_message",
            json!({"threadId": thread, "text": "use tabs please"}),
        );
        call(
            &data,
            "steer_queued_message",
            json!({"threadId": thread, "messageId": queued["id"]}),
        );
        let questions = call(&data, "list_questions", json!({"threadId": thread}));
        assert_eq!(questions[0]["answer"], "use tabs please", "{questions}");
        gate.open();
        until("the run never finished", || !running(&data, &thread));
    })
    .await
    .unwrap();
    let all = bodies(&app.llm).await;
    assert!(
        all.iter()
            .any(|body| mentions(body, "they are now closed") && mentions(body, "Tabs or spaces?")),
        "the run never read which question the steer answered"
    );
    app.serving.abort();
}

/// Two questions asked in one call: pages of one box.
fn two_pages() -> Value {
    json!({"questions": [
        {"question": "Tabs or spaces?", "choices": [choice("Tabs"), choice("Spaces")], "recommended": "Spaces", "topic": "indent"},
        {"question": "Which license?", "recommended": "MIT", "why": "the usual choice", "topic": "license"}
    ]})
}

/// The content of the last tool result in a request.
fn last_tool_result(body: &Value) -> String {
    messages(body)
        .iter()
        .rev()
        .find(|m| m["role"] == "tool")
        .and_then(|m| m["content"].as_str())
        .unwrap_or_default()
        .to_owned()
}

/// Answers several of the chat's questions at once, as the box does after its last page.
fn answer_all(data: &Path, thread: &str, answers: &[(&Value, &str)]) -> Value {
    let answers: Vec<Value> = answers
        .iter()
        .map(|(question, answer)| json!({"questionId": question["id"], "answer": answer}))
        .collect();
    call(
        data,
        "answer_question",
        json!({"threadId": thread, "answers": answers}),
    )
}

/// Several questions in one call are stored as one group (the box's pages), each its own
/// question with its rich choices; the run goes on, and the answers given together reach the
/// agent as one message naming each question and its answer.
#[tokio::test(flavor = "multi_thread")]
async fn several_questions_are_pages_of_one_box_and_their_answers_arrive_together() {
    let app = app(|body| {
        let (text, results) = last_user(body);
        if text.starts_with(ANSWER_MARKER) {
            return reply("Using your answers.");
        }
        match results {
            0 => tools(&[("p", "ask_user", two_pages())]),
            _ => reply("Working on my recommendations."),
        }
    })
    .await;
    let (data, thread) = (app.data.path().to_owned(), app.thread.clone());
    tokio::task::spawn_blocking(move || {
        call(
            &data,
            "send_message",
            json!({"threadId": thread, "text": "set up the project"}),
        );
        until("the first run never finished", || !running(&data, &thread));
        let questions = call(&data, "list_questions", json!({"threadId": thread}));
        let all = questions.as_array().unwrap();
        assert_eq!(all.len(), 2, "{questions}");
        assert!(all[0]["group"].is_string(), "{questions}");
        assert_eq!(all[0]["group"], all[1]["group"], "{questions}");
        assert_eq!(all[0]["choices"], json!(["Spaces", "Tabs"]));
        assert_eq!(all[0]["details"]["Spaces"], choice_details("Spaces"));
        assert_eq!(all[1]["assumption"], "MIT");
        let answered = answer_all(
            &data,
            &thread,
            &[(&all[0], "Tabs"), (&all[1], "Apache-2.0")],
        );
        assert_eq!(answered.as_array().map(Vec::len), Some(2), "{answered}");
        until("the answers never ran", || {
            !running(&data, &thread)
                && call(&data, "get_thread_history", json!({"threadId": thread}))
                    .to_string()
                    .contains("Using your answers.")
        });
        let questions = call(&data, "list_questions", json!({"threadId": thread}));
        assert_eq!(questions[0]["answer"], "Tabs");
        assert_eq!(questions[1]["answer"], "Apache-2.0");
    })
    .await
    .unwrap();
    let all = bodies(&app.llm).await;
    // The model is offered the tool as the prompt file describes it, with pages and rich choices.
    let tool = all[0]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["function"]["name"] == "ask_user")
        .expect("ask_user offered")["function"]
        .clone();
    assert_eq!(
        tool["description"],
        themis_core::prompts::ASK_USER_DESCRIPTION
    );
    let parameters = &tool["parameters"]["properties"];
    assert_eq!(parameters["questions"]["maxItems"], 4, "{tool}");
    assert_eq!(
        parameters["choices"]["items"]["required"],
        json!([
            "answer",
            "does",
            "advantages",
            "disadvantages",
            "consequences"
        ])
    );
    assert_eq!(
        parameters["questions"]["items"]["properties"]["choices"],
        parameters["choices"]
    );
    // The call's result named both questions and the run went on with both recommendations.
    let after = all
        .iter()
        .find(|body| last_user(body).1 == 1)
        .expect("the turn after asking");
    let note = last_tool_result(after);
    assert!(note.contains("\"ids\""), "{note}");
    assert!(
        note.contains("Tabs or spaces?: Spaces; Which license?: MIT"),
        "{note}"
    );
    // One message carries both answers.
    let answer = all
        .iter()
        .map(|body| last_user(body).0)
        .find(|text| text.starts_with(ANSWER_MARKER))
        .expect("the answers reached the agent");
    assert_eq!(answer.matches(ANSWER_MARKER).count(), 1, "{answer}");
    for part in [
        "Question: Tabs or spaces?",
        "Answer: Tabs",
        "Question: Which license?",
        "Answer: Apache-2.0",
    ] {
        assert!(answer.contains(part), "{part}: {answer}");
    }
    app.serving.abort();
}

/// A choice that does not say what it does, its advantages, disadvantages and consequences is
/// refused with the missing fields named, and nothing is asked.
#[tokio::test(flavor = "multi_thread")]
async fn a_choice_that_does_not_say_what_it_does_is_refused() {
    let app = app(|body| match last_user(body).1 {
        0 => tools(&[(
            "q",
            "ask_user",
            json!({"question": "Tabs or spaces?", "choices": ["Tabs", {"answer": "Spaces", "does": "re-indent"}], "recommended": "Spaces", "why": "usual"}),
        )]),
        _ => reply("I'll ask again with the details."),
    })
    .await;
    let (data, thread) = (app.data.path().to_owned(), app.thread.clone());
    tokio::task::spawn_blocking(move || {
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
    let all = bodies(&app.llm).await;
    let refused = last_tool_result(
        all.iter()
            .find(|body| last_user(body).1 == 1)
            .expect("the turn after asking"),
    );
    assert!(
        refused.contains("is missing `does`, `advantages`, `disadvantages`, `consequences`"),
        "{refused}"
    );
    app.serving.abort();
}

/// A set ends the run when any of its pages blocks; the answers to all its pages, given
/// together, start the next turn as one message.
#[tokio::test(flavor = "multi_thread")]
async fn a_set_with_a_blocking_page_ends_the_run_until_it_is_answered() {
    let app = app(|body| {
        let (text, results) = last_user(body);
        if text.starts_with(ANSWER_MARKER) {
            return reply("Got both answers.");
        }
        if results == 0 {
            return tools(&[(
                "p",
                "ask_user",
                json!({"questions": [
                    {"question": "Which API key name?", "blocking": true},
                    {"question": "Dark mode by default?", "recommended": "Yes", "why": "the usual choice"}
                ]}),
            )]);
        }
        reply("THIS RUN SHOULD HAVE ENDED")
    })
    .await;
    let (data, thread) = (app.data.path().to_owned(), app.thread.clone());
    tokio::task::spawn_blocking(move || {
        call(
            &data,
            "send_message",
            json!({"threadId": thread, "text": "configure the API"}),
        );
        until("the run never ended", || !running(&data, &thread));
        let history = call(&data, "get_thread_history", json!({"threadId": thread})).to_string();
        assert!(history.contains("I need your answer to the question above before I can go on."));
        assert!(!history.contains("THIS RUN SHOULD HAVE ENDED"));
        let questions = call(&data, "list_questions", json!({"threadId": thread}));
        let all = questions.as_array().unwrap();
        assert_eq!(all.len(), 2, "{questions}");
        assert_eq!(all[0]["blocking"], true, "{questions}");
        assert_eq!(all[1]["blocking"], false, "{questions}");
        answer_all(&data, &thread, &[(&all[0], "THEMIS_KEY"), (&all[1], "No")]);
        until("the answers never ran", || {
            !running(&data, &thread)
                && call(&data, "get_thread_history", json!({"threadId": thread}))
                    .to_string()
                    .contains("Got both answers.")
        });
    })
    .await
    .unwrap();
    let all = bodies(&app.llm).await;
    let answer = all
        .iter()
        .map(|body| last_user(body).0)
        .find(|text| text.starts_with(ANSWER_MARKER))
        .expect("the answers reached the agent");
    assert!(
        answer.contains("Answer: THEMIS_KEY") && answer.contains("Answer: No"),
        "{answer}"
    );
    app.serving.abort();
}

/// A message the person writes in the chat closes every page of an open set, as it closes any
/// open question.
#[tokio::test(flavor = "multi_thread")]
async fn a_chat_message_closes_every_page_of_a_set() {
    let app = app(|body| {
        let (text, results) = last_user(body);
        if text.contains("Go with your picks") {
            return reply("Going with them.");
        }
        match results {
            0 => tools(&[("p", "ask_user", two_pages())]),
            _ => reply("Working on my recommendations."),
        }
    })
    .await;
    let (data, thread) = (app.data.path().to_owned(), app.thread.clone());
    tokio::task::spawn_blocking(move || {
        call(
            &data,
            "send_message",
            json!({"threadId": thread, "text": "set up the project"}),
        );
        until("the first run never finished", || !running(&data, &thread));
        call(
            &data,
            "send_message",
            json!({"threadId": thread, "text": "Go with your picks"}),
        );
        until("the answer run never finished", || !running(&data, &thread));
        let questions = call(&data, "list_questions", json!({"threadId": thread}));
        assert_eq!(questions.as_array().map(Vec::len), Some(2), "{questions}");
        for question in questions.as_array().unwrap() {
            assert_eq!(question["answer"], "Go with your picks", "{questions}");
        }
    })
    .await
    .unwrap();
    let all = bodies(&app.llm).await;
    let (text, _) = last_user(
        all.iter()
            .find(|body| last_user(body).0.contains("Go with your picks"))
            .expect("the answer turn"),
    );
    assert!(text.contains("they are now closed"), "{text}");
    assert!(
        text.contains("Tabs or spaces?") && text.contains("Which license?"),
        "{text}"
    );
    app.serving.abort();
}

/// After the person presses Deny on an approval (Custom), the agent is told to run nothing
/// else for that step and to ask how to go on: a safer way, skip it, or stop. Following it,
/// it asks a blocking question with those choices and the run waits.
#[tokio::test(flavor = "multi_thread")]
async fn the_persons_deny_has_the_agent_ask_how_to_go_on() {
    let way = |answer: &str| json!({"answer": answer, "does": format!("{answer} (notes.txt)"), "advantages": "a", "disadvantages": "b", "consequences": "easy to undo"});
    let choices = json!([
        way("Write the notes to the chat instead"),
        way("Skip this step and carry on"),
        way("Stop here")
    ]);
    let app = app(move |body| {
        let results = last_user(body).1;
        if results == 0 {
            return tools(&[(
                "w",
                "write_file",
                json!({"file_path": "notes.txt", "content": "notes\n"}),
            )]);
        }
        if results == 1 && last_tool_result(body).contains("pressed Deny") {
            return tools(&[(
                "q",
                "ask_user",
                json!({"question": "You denied writing notes.txt. How should I go on?", "blocking": true, "choices": choices}),
            )]);
        }
        reply("THIS RUN SHOULD HAVE ENDED")
    })
    .await;
    let (data, thread) = (app.data.path().to_owned(), app.thread.clone());
    let project = app._project.path().to_owned();
    tokio::task::spawn_blocking(move || {
        call(
            &data,
            "set_thread_approval_mode",
            json!({"threadId": thread, "mode": "custom"}),
        );
        call(
            &data,
            "send_message",
            json!({"threadId": thread, "text": "write the notes"}),
        );
        let mut approval = Value::Null;
        until("no approval was asked for", || {
            let pending = call(&data, "get_pending_approvals", json!({}));
            approval = pending
                .as_array()
                .and_then(|all| {
                    all.iter()
                        .find(|a| a["thread_id"] == thread.as_str() && a["tool"] == "write_file")
                })
                .cloned()
                .unwrap_or(Value::Null);
            !approval.is_null()
        });
        call(
            &data,
            "approve_action",
            json!({"threadId": thread, "approvalId": approval["approval_id"], "decision": "deny"}),
        );
        until("the run never ended", || !running(&data, &thread));
        let history = call(&data, "get_thread_history", json!({"threadId": thread})).to_string();
        assert!(!history.contains("THIS RUN SHOULD HAVE ENDED"), "{history}");
        let questions = call(&data, "list_questions", json!({"threadId": thread}));
        assert_eq!(questions[0]["blocking"], true, "{questions}");
        assert_eq!(
            questions[0]["choices"],
            json!([
                "Write the notes to the chat instead",
                "Skip this step and carry on",
                "Stop here"
            ])
        );
        assert!(!project.join("notes.txt").exists());
    })
    .await
    .unwrap();
    let all = bodies(&app.llm).await;
    let denied = last_tool_result(
        all.iter()
            .find(|body| last_user(body).1 == 1)
            .expect("the turn after the Deny"),
    );
    assert!(
        denied.contains(themis_core::prompts::TOOL_DENIED_BY_PERSON),
        "{denied}"
    );
    assert!(
        !denied.contains(themis_core::prompts::TOOL_REFUSAL),
        "{denied}"
    );
    app.serving.abort();
}
