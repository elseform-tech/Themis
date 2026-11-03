//! Memory across chats through the shared server and the `themis` CLI, with a scripted model
//! (no real model is called). The memory models (`memory.models`) keep memory on their own:
//! after a turn the pass runs on the first model, falls back to the next when it is rate
//! limited or fails, records which model did the job, and the chat gets one `memory_changed`
//! line; the next turn's request carries the index and the relevant memory in full (and the
//! context meter's Memory part); the chat's model is never offered a memory tool; a job every
//! model failed waits and is retried; the daily cap holds paid models back; memory off sends
//! nothing. Settings › Memory and Settings › Models' list work through the CLI with revisions.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use themis_desktop::server::{Client, Server};
use themis_desktop::state::AppState;
use wiremock::{matchers::method, Mock, MockServer, Request, ResponseTemplate};

const PASS: &str = "You keep an assistant's memory";
const NOTE: &str = "Memory from earlier chats";
const SUBAGENT: &str = "You are a subagent";
const MODELS: &str = r#"{"memory":{"models":["mem-one","mem-two","mem-three"]}}"#;

fn cli(dir: &Path, command: &str, args: Value) -> Result<Value, String> {
    use std::io::Write;
    use std::process::Stdio;
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
        .arg("--data-dir")
        .arg(dir)
        .args(["call", command, "-"])
        .env_remove("OPENCODE_KEY")
        .env("HOME", dir)
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
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned());
    }
    Ok(serde_json::from_slice(&output.stdout).unwrap_or(Value::Null))
}

fn text_reply(text: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "choices":[{"message":{"role":"assistant","content":text},"finish_reason":"stop"}],
        "usage":{"prompt_tokens":321,"completion_tokens":12,"total_tokens":333}
    }))
}

fn call(name: &str, arguments: Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call_1","type":"function","function":{"name":name,"arguments":arguments.to_string()}}]},"finish_reason":"tool_calls"}]}))
}

fn system(body: &Value) -> String {
    body["messages"][0]["content"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

fn last(body: &Value) -> Value {
    body["messages"]
        .as_array()
        .and_then(|messages| messages.last().cloned())
        .unwrap_or(Value::Null)
}

fn last_user(body: &Value) -> String {
    body["messages"]
        .as_array()
        .and_then(|messages| messages.iter().rev().find(|m| m["role"] == "user"))
        .and_then(|m| m["content"].as_str())
        .unwrap_or_default()
        .to_owned()
}

/// What a memory model answers, by what the person said that turn.
fn pass_answer(body: &Value) -> String {
    let user = body["messages"][1]["content"].as_str().unwrap_or_default();
    let person = user.split("<<<person").nth(1).unwrap_or_default();
    if person.contains("always use pnpm") {
        json!({"memories":[{"action":"save","scope":"global","name":"uses-pnpm","type":"user",
            "description":"Uses pnpm, not npm","body":"The person uses pnpm for JavaScript projects."}]})
        .to_string()
    } else if person.contains("forget the pnpm") {
        json!({"memories":[{"action":"delete","scope":"global","name":"uses-pnpm"}]}).to_string()
    } else if person.contains("deadline") {
        json!({"memories":[{"action":"save","scope":"project","name":"launch-deadline","type":"project",
            "description":"Launch is on Friday","body":"The launch is on Friday."}]})
        .to_string()
    } else {
        r#"{"memories":[]}"#.to_owned()
    }
}

/// How each memory model behaves.
#[derive(Default)]
struct Script {
    /// `mem-one` answers 429.
    first_rate_limited: AtomicBool,
    /// Every memory model answers 500.
    all_down: AtomicBool,
    /// Memory models answer after a moment.
    slow_memory: AtomicBool,
}

struct Harness {
    data: tempfile::TempDir,
    project: tempfile::TempDir,
    events: tokio::net::TcpStream,
    _serving: tokio::task::JoinHandle<Result<(), String>>,
    _provider: MockServer,
    requests: Arc<Mutex<Vec<Value>>>,
    script: Arc<Script>,
    /// Every live `memory_working` step seen, in order, with `changed` for each memory line.
    steps: Vec<String>,
}

impl Harness {
    async fn start(user_config: &str) -> Self {
        let data = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        std::fs::write(data.path().join("runtime.jsonc"), user_config).unwrap();
        let provider = MockServer::builder().start().await;
        let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
        let script = Arc::new(Script::default());
        let (seen, behave) = (Arc::clone(&requests), Arc::clone(&script));
        Mock::given(method("POST"))
            .respond_with(move |request: &Request| {
                let body: Value = request.body_json().unwrap();
                seen.lock().unwrap().push(body.clone());
                let model = body["model"].as_str().unwrap_or_default().to_owned();
                if model.starts_with("mem-") {
                    if behave.all_down.load(Ordering::SeqCst) {
                        return ResponseTemplate::new(500).set_body_string("down");
                    }
                    if model == "mem-one" && behave.first_rate_limited.load(Ordering::SeqCst) {
                        return ResponseTemplate::new(429).set_body_string("slow down");
                    }
                    let answer = text_reply(&pass_answer(&body));
                    if behave.slow_memory.load(Ordering::SeqCst) {
                        return answer.set_delay(Duration::from_millis(1200));
                    }
                    return answer;
                }
                if system(&body).starts_with(SUBAGENT) {
                    return text_reply("Sub done.");
                }
                // A goal's turn: complete it once, with evidence.
                let offers_update = body["tools"]
                    .as_array()
                    .is_some_and(|tools| tools.iter().any(|tool| tool["function"]["name"] == "update_goal"));
                let updated = body["messages"].as_array().is_some_and(|messages| {
                    messages
                        .iter()
                        .any(|message| message["tool_calls"].to_string().contains("update_goal"))
                });
                if offers_update && !updated {
                    return call(
                        "update_goal",
                        json!({"status":"complete","evidence":[{"requirement":"notes.txt exists","proof":"ls notes.txt: notes.txt"}]}),
                    );
                }
                // A run that keeps working a moment: a read, answered slowly, then the reply.
                if last_user(&body).contains("keep working") && last(&body)["role"] == "user" {
                    return call("list_dir", json!({"directory_path":"."}))
                        .set_delay(Duration::from_millis(1500));
                }
                if last_user(&body).starts_with("spawn-one") && last(&body)["role"] == "user" {
                    return call(
                        "spawn_subagents",
                        json!({"say":"Starting a helper.","subagents":[{"name":"Helper","brief":"Look around."}]}),
                    );
                }
                text_reply("Done.")
            })
            .mount(&provider)
            .await;
        let state = AppState::new_for_test(data.path().join("settings.json"));
        state
            .set_secret("go".into(), "synthetic-test-key".into())
            .await
            .unwrap();
        state.set_go_base_url_override(Some(provider.uri()));
        state.set_memory_jobs_for_test(true, 300, 100);
        let server = Server::bind(state, data.path()).await.unwrap();
        let serving = tokio::spawn(server.run());
        let events = Client::new(data.path().into()).subscribe().await.unwrap();
        Self {
            data,
            project,
            events,
            _serving: serving,
            _provider: provider,
            requests,
            script,
            steps: Vec::new(),
        }
    }

    fn cli(&self, command: &str, args: Value) -> Value {
        cli(self.data.path(), command, args).unwrap_or_else(|error| panic!("{command}: {error}"))
    }

    fn new_chat(&self) -> String {
        let thread = self.cli(
            "create_thread",
            json!({"projectRoot":self.project.path(),"provider":"go","model":"test-model"}),
        );
        thread["id"].as_str().unwrap().to_owned()
    }

    /// Sends one message, waits for its run to end, then up to `settle` for a memory line
    /// (stopping at the first); the memory lines and context budgets it saw.
    async fn send(&mut self, id: &str, text: &str, settle: Duration) -> (Vec<Value>, Vec<Value>) {
        self.cli(
            "send_message",
            json!({"threadId":id,"text":text,"reasoningEffort":null}),
        );
        let (lines, budgets) = self.watch(id, settle, true).await;
        (lines, budgets)
    }

    /// Thread events for `id`: until its run ends (when `run`), then up to `settle` for the
    /// first memory line.
    async fn watch(&mut self, id: &str, settle: Duration, run: bool) -> (Vec<Value>, Vec<Value>) {
        let mut lines = Vec::new();
        let mut budgets = Vec::new();
        let mut finished = !run;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
        loop {
            let wait = if finished {
                settle
            } else {
                deadline - tokio::time::Instant::now()
            };
            let Ok(item) = tokio::time::timeout(wait, Client::next_event(&mut self.events)).await
            else {
                assert!(finished, "the run did not end");
                return (lines, budgets);
            };
            let item = item.unwrap();
            if item["name"] != "thread-event" || item["payload"]["thread_id"] != id {
                continue;
            }
            let event = &item["payload"]["event"];
            let kind = event["kind"].as_str().unwrap_or_default();
            match kind {
                "memory_working" => {
                    self.steps
                        .push(event["step"].as_str().unwrap_or_default().to_owned());
                    if finished && event["step"] != "checking" {
                        return (lines, budgets);
                    }
                }
                "memory_changed" => {
                    self.steps.push("changed".into());
                    lines.push(event.clone());
                    if finished {
                        return (lines, budgets);
                    }
                }
                "context_budget" => budgets.push(event["budget"].clone()),
                "finished" | "failed" | "incomplete" if !finished => {
                    assert_eq!(kind, "finished", "{event}");
                    finished = true;
                }
                _ => {}
            }
        }
    }

    fn memory_requests(&self) -> Vec<Value> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|body| {
                body["model"]
                    .as_str()
                    .unwrap_or_default()
                    .starts_with("mem-")
            })
            .cloned()
            .collect()
    }

    /// The first request of the run that `text` started.
    fn first_request_of(&self, text: &str) -> Value {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|body| {
                body["model"] == "test-model"
                    && last_user(body).starts_with(text)
                    && last(body)["role"] == "user"
            })
            .cloned()
            .unwrap_or_else(|| panic!("no request for {text}"))
    }

    /// Every diagnostics line of `memory`/`event`, and all the log text.
    fn logs(&self, event: &str) -> (Vec<Value>, String) {
        let mut text = String::new();
        if let Ok(entries) = std::fs::read_dir(self.data.path().join("logs")) {
            for entry in entries.flatten() {
                text.push_str(&std::fs::read_to_string(entry.path()).unwrap_or_default());
            }
        }
        let lines = text
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .filter(|line| line["service"] == "memory" && line["event"] == event)
            .map(|line| line["details"].clone())
            .collect();
        (lines, text)
    }

    /// Waits until there are `count` `memory`/`event` lines.
    async fn logged(&self, event: &str, count: usize) -> Vec<Value> {
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        loop {
            let (lines, _) = self.logs(event);
            if lines.len() >= count || std::time::Instant::now() > deadline {
                assert!(lines.len() >= count, "memory/{event}: {lines:?}");
                return lines;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

fn tool_names(body: &Value) -> Vec<String> {
    body["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|tool| tool["function"]["name"].as_str().map(str::to_owned))
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_memory_models_keep_memory_with_fallback_and_the_next_turn_reads_it() {
    let mut h = Harness::start(MODELS).await;
    h.script.first_rate_limited.store(true, Ordering::SeqCst);
    let id = h.new_chat();

    // A preference said in passing: the first model is rate limited, the second saves it.
    let (lines, _) = h
        .send(
            &id,
            "By the way, I always use pnpm. Please fix the build.",
            Duration::from_secs(10),
        )
        .await;
    assert_eq!(lines.len(), 1, "{lines:?}");
    let changes = lines[0]["changes"].as_array().unwrap();
    assert_eq!(changes.len(), 1, "{lines:?}");
    assert_eq!(changes[0]["action"], "saved");
    assert_eq!(changes[0]["scope"], "global");
    assert_eq!(changes[0]["name"], "uses-pnpm");
    assert_eq!(changes[0]["model"], "mem-two");
    assert_eq!(changes[0]["model_index"], 1);
    // The chat saw the pass working before its line, shimmering under the reply.
    assert_eq!(h.steps, ["checking", "changed"]);
    let saved = std::fs::read_to_string(h.data.path().join("memory/global/uses-pnpm.md")).unwrap();
    assert!(saved.contains("type: user"), "{saved}");
    // The pass asked mem-one, then mem-two, never mem-three; it read the turn as data.
    let asked: Vec<String> = h
        .memory_requests()
        .iter()
        .map(|body| body["model"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(asked, ["mem-one", "mem-two"]);
    let pass = &h.memory_requests()[1];
    assert!(system(pass).starts_with(PASS), "{}", system(pass));
    let user = pass["messages"][1]["content"].as_str().unwrap();
    assert!(
        user.contains("<<<person\nBy the way, I always use pnpm."),
        "{user}"
    );
    // After the turn: the reply that ended it; a first message answered no earlier reply.
    assert!(user.contains("<<<reply-answered\n"), "{user}");
    assert!(user.contains("<<<assistant-reply\nDone.\n>>>"), "{user}");
    assert!(pass["tools"].is_null(), "the pass offers no tools");
    // Which model did the job, as metadata only.
    let jobs = h.logged("job", 2).await;
    assert_eq!(
        (&jobs[0]["model_index"], &jobs[0]["outcome"]),
        (&json!(0), &json!("rate_limited"))
    );
    assert_eq!(
        (&jobs[1]["model_index"], &jobs[1]["outcome"]),
        (&json!(1), &json!("answered"))
    );
    assert_eq!(jobs[1]["prompt_tokens"], 321);
    assert_eq!(jobs[1]["kind"], "pass");
    let done = h.logged("job_done", 1).await;
    assert_eq!(done[0]["outcome"], "changed");
    assert_eq!(done[0]["counts"]["saved"], 1);
    // The chat's model is offered no memory tool, and is told nothing about saving.
    let first = h.first_request_of("By the way");
    assert!(!tool_names(&first).is_empty());
    assert!(
        !tool_names(&first).iter().any(|name| name == "memory"),
        "{:?}",
        tool_names(&first)
    );

    // The next turn's request carries the index and, sharing a word with it, the memory in full;
    // the meter shows a Memory part, and the parts still add up.
    h.script.first_rate_limited.store(false, Ordering::SeqCst);
    let next = h.new_chat();
    let (_, budgets) = h
        .send(
            &next,
            "Which package manager, pnpm or npm?",
            Duration::from_secs(3),
        )
        .await;
    let request = h.first_request_of("Which package manager");
    let note = system(&request);
    let preamble = themis_core::prompts::SYSTEM_MEMORY
        .split("{{global}}")
        .next()
        .unwrap();
    assert!(note.contains(preamble), "{note}");
    assert!(note.contains("Everywhere:\n- uses-pnpm (user, "), "{note}");
    assert!(
        note.contains("in full:\n### uses-pnpm (user, ")
            && note.contains("The person uses pnpm for JavaScript projects."),
        "{note}"
    );
    assert!(!tool_names(&request).iter().any(|name| name == "memory"));
    let budget = budgets.first().expect("a context budget");
    let breakdown = &budget["breakdown"];
    assert!(breakdown["memory"].as_u64().unwrap() > 0, "{budget}");
    let parts: u64 = [
        "system",
        "checkpoint",
        "attachments",
        "conversation",
        "tool_activity",
        "reasoning",
        "memory",
    ]
    .iter()
    .map(|key| breakdown[*key].as_u64().unwrap_or(0))
    .sum();
    assert_eq!(parts, budget["used_tokens"].as_u64().unwrap(), "{budget}");
    // That turn held nothing new: the model answered, nothing changed, no line.
    let done = h.logged("job_done", 2).await;
    assert_eq!(done[1]["outcome"], "nothing");

    // A subagent starts with the same memory block and no memory tool either.
    h.send(&next, "spawn-one", Duration::from_millis(500)).await;
    let child = tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let found = h
                .requests
                .lock()
                .unwrap()
                .iter()
                .find(|body| system(body).starts_with(SUBAGENT))
                .cloned();
            if let Some(found) = found {
                return found;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("the subagent asked its model");
    assert!(
        system(&child).contains("- uses-pnpm (user, "),
        "{}",
        system(&child)
    );
    assert!(!tool_names(&child).iter().any(|name| name == "memory"));

    // A trivial turn: no pass at all.
    let before = h.memory_requests().len();
    h.send(&id, "ok", Duration::from_millis(500)).await;
    let skipped = h.logged("pass_skipped", 1).await;
    assert!(
        skipped.iter().any(|line| line["reason"] == "trivial"),
        "{skipped:?}"
    );
    assert_eq!(h.memory_requests().len(), before);

    // Asked to forget: the memory model removes it, and the line says so.
    let (lines, _) = h
        .send(
            &id,
            "Please forget the pnpm preference.",
            Duration::from_secs(10),
        )
        .await;
    assert_eq!(lines[0]["changes"][0]["action"], "removed", "{lines:?}");
    assert_eq!(lines[0]["changes"][0]["model_index"], 0);
    assert!(!h.data.path().join("memory/global/uses-pnpm.md").exists());

    // The memory lines are saved with the chat.
    let history = h.cli("get_thread_history", json!({"threadId": id}));
    let saved: Vec<&str> = history
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["envelope"]["event"]["kind"] == "memory_changed")
        .map(|item| {
            item["envelope"]["event"]["changes"][0]["action"]
                .as_str()
                .unwrap()
        })
        .collect();
    assert_eq!(saved, ["saved", "removed"], "{history}");
    // Diagnostics never hold what was said or saved.
    let (_, text) = h.logs("job");
    for private in ["pnpm", "JavaScript", "package manager"] {
        assert!(!text.contains(private), "diagnostics hold {private:?}");
    }
}

/// The pass runs once a turn's reply is done; when it finishes while the chat's next run is
/// already working, that run gets what changed at its next step, as a plain runtime note.
#[tokio::test(flavor = "multi_thread")]
async fn a_memory_saved_while_the_next_run_works_reaches_that_run() {
    let mut h = Harness::start(MODELS).await;
    h.script.slow_memory.store(true, Ordering::SeqCst);
    let id = h.new_chat();
    h.send(
        &id,
        "By the way, I always use pnpm.",
        Duration::from_millis(50),
    )
    .await;
    // The next message lands as soon as the first reply is done, while its pass still runs.
    let (_lines, _) = h
        .send(
            &id,
            "Now please keep working on the build.",
            Duration::from_secs(10),
        )
        .await;
    let main: Vec<Value> = h
        .requests
        .lock()
        .unwrap()
        .iter()
        .filter(|body| {
            !body["model"]
                .as_str()
                .unwrap_or_default()
                .starts_with("mem-")
        })
        .cloned()
        .collect();
    let told = main
        .iter()
        .find(|body| {
            body["messages"]
                .to_string()
                .contains("Memory changed while you work")
        })
        .expect("the running turn was told");
    let note = told["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| {
            message["content"]
                .as_str()
                .is_some_and(|text| text.starts_with("Memory changed while you work"))
        })
        .expect("the note is its own message, not wrapped as a subagent report")
        .clone();
    let text = note["content"].as_str().unwrap();
    assert!(
        text.contains("saved uses-pnpm (global): Uses pnpm, not npm"),
        "{text}"
    );
    assert!(
        text.contains("The person uses pnpm"),
        "with what it says: {text}"
    );
    assert!(
        !text.contains("[memory-note]"),
        "the marker never reaches the model"
    );
}

/// Events start a pass on their own: a completed goal hands the memory models its objective
/// and evidence, so what was built and verified can be kept without the person's words.
#[tokio::test(flavor = "multi_thread")]
async fn a_completed_goal_starts_a_pass_with_its_evidence() {
    let mut h = Harness::start(MODELS).await;
    h.script.slow_memory.store(true, Ordering::SeqCst);
    let id = h.new_chat();
    h.send(&id, "/goal write notes.txt", Duration::from_millis(300))
        .await;
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    let pass = loop {
        let found = h.memory_requests().into_iter().find(|body| {
            body["messages"]
                .to_string()
                .contains("What just happened: a goal was completed")
        });
        if let Some(body) = found {
            break body;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no pass for the completed goal"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    let user = pass["messages"][1]["content"].as_str().unwrap();
    assert!(user.contains("Objective: write notes.txt"), "{user}");
    assert!(
        user.contains("- notes.txt exists: ls notes.txt: notes.txt"),
        "{user}"
    );
    let events = h.logged("event", 1).await;
    assert_eq!(events[0]["event"], "a goal was completed");
    // The turn's pass and the goal's run one after the other, never at once, so the second
    // reads what the first saved instead of saving the same thing again.
    h.logged("job", 2).await;
    let (_, text) = h.logs("job");
    let mut spans: Vec<(i64, i64)> = text
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|line| line["service"] == "memory" && line["event"] == "job")
        .map(|line| {
            let end = line["timestamp"].as_i64().unwrap();
            (end - line["details"]["elapsed_ms"].as_i64().unwrap(), end)
        })
        .collect();
    spans.sort();
    assert!(spans.len() >= 2, "{spans:?}");
    for pair in spans.windows(2) {
        assert!(
            pair[1].0 + 20 >= pair[0].1,
            "memory jobs overlapped: {spans:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_job_every_memory_model_failed_waits_and_is_retried_and_settings_edit_the_list() {
    let mut h = Harness::start(MODELS).await;
    h.script.all_down.store(true, Ordering::SeqCst);
    let id = h.new_chat();
    let (lines, _) = h
        .send(
            &id,
            "Remember the deadline is Friday.",
            Duration::from_millis(200),
        )
        .await;
    assert!(lines.is_empty(), "{lines:?}");
    // The chat's line said the memory models were checking, then that the job waits.
    assert_eq!(h.steps, ["checking", "waiting"]);
    // All three were asked, each failed, and the job waits; Settings › Memory says so.
    let waiting = h.logged("job_waiting", 1).await;
    assert_eq!(
        waiting[0]["reasons"],
        json!(["provider_error", "provider_error", "provider_error"])
    );
    let settings = h.cli("get_memory_settings", json!({}));
    assert_eq!(settings["waiting"]["jobs"], 1, "{settings}");
    let root = h.project.path().to_string_lossy().into_owned();
    let listed = h.cli(
        "list_memories",
        json!({"scope":"project","projectRoot":root}),
    );
    assert_eq!(listed["waiting"]["jobs"], 1, "{listed}");
    assert_eq!(listed["memories"].as_array().unwrap().len(), 0);

    // The models come back: once the app is idle the job is tried again, on the first model.
    h.script.all_down.store(false, Ordering::SeqCst);
    let (lines, _) = h.watch(&id, Duration::from_secs(20), false).await;
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert_eq!(lines[0]["changes"][0]["name"], "launch-deadline");
    assert_eq!(lines[0]["changes"][0]["scope"], "project");
    assert_eq!(lines[0]["changes"][0]["model_index"], 0);
    let settings = h.cli("get_memory_settings", json!({}));
    assert!(settings["waiting"].is_null(), "{settings}");
    let listed = h.cli(
        "list_memories",
        json!({"scope":"project","projectRoot":root}),
    );
    assert_eq!(listed["memories"][0]["name"], "launch-deadline", "{listed}");

    // Settings › Models' list: at least three, in order, written in place.
    let revision = settings["revision"].clone();
    assert!(cli(
        h.data.path(),
        "update_memory_settings",
        json!({"models": ["mem-one", "mem-two"], "revision": revision}),
    )
    .is_err());
    assert!(cli(
        h.data.path(),
        "update_memory_settings",
        json!({"models": ["mem-one", "mem-two", "mem-one"], "revision": revision}),
    )
    .is_err());
    let saved = h.cli(
        "update_memory_settings",
        json!({"models": ["mem-three", "mem-one", "mem-two"], "revision": revision}),
    );
    assert_eq!(
        saved["settings"]["models"],
        json!(["mem-three", "mem-one", "mem-two"])
    );
    assert!(
        cli(
            h.data.path(),
            "update_memory_settings",
            json!({"enabled": false, "revision": revision}),
        )
        .is_err(),
        "a stale revision is refused"
    );
    let config = std::fs::read_to_string(h.data.path().join("runtime.jsonc")).unwrap();
    assert!(config.contains("\"mem-three\""), "{config}");

    // The daily cap: at $0 the (unpriced, so paid) models are held back without a request.
    let capped = r#"{"memory":{"models":["mem-three","mem-one","mem-two"],"daily_budget_usd":0}}"#;
    std::fs::write(h.data.path().join("runtime.jsonc"), capped).unwrap();
    let before = h.memory_requests().len();
    h.send(
        &id,
        "Remember the deadline moved to Monday.",
        Duration::from_millis(300),
    )
    .await;
    let waiting = h.logged("job_waiting", 2).await;
    assert_eq!(
        waiting[1]["reasons"],
        json!(["over_budget", "over_budget", "over_budget"]),
        "{waiting:?}"
    );
    assert_eq!(h.memory_requests().len(), before, "no request past the cap");
    std::fs::write(h.data.path().join("runtime.jsonc"), &config).unwrap();

    // Memory off: the agent is told, no memory reaches the request, and no job runs.
    let settings = h.cli("get_memory_settings", json!({}));
    let off = h.cli(
        "update_memory_settings",
        json!({"enabled": false, "revision": settings["revision"]}),
    );
    assert_eq!(off["settings"]["enabled"], false);
    let before = h.memory_requests().len();
    h.send(
        &id,
        "Another deadline note for later.",
        Duration::from_millis(500),
    )
    .await;
    let request = h.first_request_of("Another deadline note");
    assert!(!system(&request).contains(NOTE));
    assert!(system(&request).contains(themis_core::prompts::SYSTEM_MEMORY_OFF));
    assert_eq!(h.memory_requests().len(), before);
    // Memories live in the app data folder, never the project.
    assert!(!h.project.path().join("launch-deadline.md").exists());
    assert!(!h.project.path().join(".themis/memory").exists());
}
