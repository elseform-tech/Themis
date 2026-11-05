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
    // Plan records, by event.
    if user.contains("What just happened: a goal was created") && user.contains("notes tool") {
        return json!({"plan":{"op":"create","name":"notes-tool","title":"Notes tool in notes.txt"},
            "memories":[{"action":"save","scope":"global","name":"likes-short-names","type":"user",
            "description":"Likes short file names","body":"The person likes short file names."}]})
        .to_string();
    }
    if user.contains("What just happened: the person changed or added to the goal")
        && user.contains("sort notes by date")
    {
        return json!({"plan":{"op":"change","name":"notes-tool","change":"Notes are sorted by date, not title.",
            "edits":[{"old":"2. Sort notes by title.","new":"2. Sort notes by date."}]},"memories":[],
            "decisions":[{"work":"notes-tool","what":"sort notes by date","why":"newest notes matter most"}],
            "tasks":[{"op":"add","title":"Show the count in the footer","note":"\"show the count\"","work":"notes-tool"}]})
        .to_string();
    }
    if user.contains("What just happened: a goal was completed") && user.contains("notes tool") {
        return json!({"plan":{"op":"done","name":"notes-tool","results":"Built notes.txt; check with: ls notes.txt",
            "evidence":["made up by the model"],
            "lessons":["Write notes.txt with a trailing newline — the line count check reads whole lines"]},
            "memories":[],
            "tasks":[{"op":"update","title":"Show the count in the footer","status":"done","note":"ls notes.txt shows it"}]})
        .to_string();
    }
    // Tasks outside any plan: one added, then done.
    if person.contains("also write a README") {
        return json!({"memories":[],"tasks":[{"op":"add","title":"Write the README","note":"\"also write a README\" with run steps"}]}).to_string();
    }
    if person.contains("README is written") {
        return json!({"memories":[],"tasks":[{"op":"update","title":"Write the README","status":"done","note":"README.md has the run steps"}]}).to_string();
    }
    if person.contains("always use pnpm") {
        json!({"memories":[{"action":"save","scope":"global","name":"uses-pnpm","type":"user",
            "description":"Uses pnpm, not npm","body":"The person uses pnpm for JavaScript projects."}]})
        .to_string()
    } else if person.contains("forget the pnpm") {
        json!({"memories":[{"action":"delete","scope":"global","name":"uses-pnpm"}]}).to_string()
    } else if person.contains("demo day") {
        // Same description, new text: an update all the same.
        json!({"memories":[{"action":"update","scope":"project","name":"launch-deadline","type":"project",
            "description":"Launch is on Friday","body":"The launch is on Friday, with a demo day before it."}]})
        .to_string()
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
                // Auto mode's reviewer (the first memory model): its verdict by the command.
                if system(&body).starts_with("You review one action") {
                    let asked = body["messages"][1]["content"].as_str().unwrap_or_default();
                    let asked = asked.split("<<<action").nth(1).unwrap_or_default();
                    let verdict = if asked.contains("soft.txt") {
                        json!({"verdict":"soft","reason":"it touches a file nobody named","safer":"use notes.txt instead"})
                    } else if asked.contains("hard.txt") {
                        json!({"verdict":"hard","reason":"it was not asked for","safer":"leave it out"})
                    } else {
                        json!({"verdict":"allow","reason":"","safer":""})
                    };
                    return text_reply(&verdict.to_string());
                }
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
                // A turn whose model request fails outright.
                if last_user(&body).contains("this turn fails") {
                    return ResponseTemplate::new(400).set_body_string("bad request");
                }
                // A turn whose first step fails, then a reply.
                if last_user(&body).contains("list the missing folder") && last(&body)["role"] == "user" {
                    return call("list_dir", json!({"directory_path": "no-such-folder"}));
                }
                // Auto mode: the agent runs one harmless command, then says done.
                if let Some(name) = last_user(&body).strip_prefix("auto-step ") {
                    if last(&body)["role"] == "user" {
                        return call("shell", json!({"command": "touch", "args": [name.trim()]}));
                    }
                    return text_reply("Done.");
                }
                // The agent looks a task up.
                if last_user(&body).contains("look up the readme task") && last(&body)["role"] == "user" {
                    return call("search_memory", json!({"query": "readme"}));
                }
                // The agent looks memory up: a search, then one result read in full.
                if last_user(&body).contains("look up the launch") {
                    let tool_text = last(&body)["content"].to_string();
                    if last(&body)["role"] == "user" {
                        return call("search_memory", json!({"query": "when is the launch"}));
                    }
                    if tool_text.contains("matches") {
                        return ResponseTemplate::new(200).set_body_json(json!({"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call_2","type":"function","function":{"name":"search_memory","arguments":json!({"name":"launch-deadline"}).to_string()}}]},"finish_reason":"tool_calls"}]}));
                    }
                }
                // A plan the person can approve.
                if last_user(&body).contains("Plan the notes tool") {
                    return text_reply("1. Store notes in notes.txt.\n2. Sort notes by title.\n3. Show the count.");
                }
                // A goal turn that keeps working a while before it completes.
                if body["messages"].to_string().contains("slow goal") && last(&body)["role"] == "user" {
                    return call("list_dir", json!({"directory_path":"."}))
                        .set_delay(Duration::from_millis(2500));
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
                        json!({"say":"Starting a helper.","subagents":[{"write_paths": [], "name":"Helper","brief":"Look around."}]}),
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
    let events = h.logged("event", 2).await;
    assert!(
        events
            .iter()
            .any(|event| event["event"] == "a goal was completed"),
        "{events:?}"
    );
    // The evidence is in the chat, under the Complete note (the goal bar keeps one line).
    let history = h.cli("get_thread_history", json!({"threadId":id}));
    assert!(
        history
            .to_string()
            .contains("Complete.\\n\\n- notes.txt exists: ls notes.txt: notes.txt"),
        "{history}"
    );
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

/// A goal's plan and what the person adds mid-goal reach memory at once, while the goal's
/// first turn still works, not after it: the created goal hands over the plan reply the
/// person answered, and a steered message its words. The turn's own pass leaves those words
/// out, so nothing is passed twice.
#[tokio::test(flavor = "multi_thread")]
async fn a_new_goal_and_a_mid_goal_change_reach_memory_while_the_turn_works() {
    let mut h = Harness::start(MODELS).await;
    let id = h.new_chat();
    h.send(&id, "Plan the notes tool.", Duration::from_millis(100))
        .await;
    h.cli(
        "send_message",
        json!({"threadId":id,"text":"/goal slow goal: build the notes tool as planned","reasoningEffort":null}),
    );
    let created = h.logged("event", 1).await;
    assert_eq!(created[0]["event"], "a goal was created");
    let message = h.cli(
        "enqueue_message",
        json!({"threadId":id,"text":"Change: notes are sorted by date."}),
    );
    h.cli(
        "steer_queued_message",
        json!({"threadId":id,"messageId":message["id"]}),
    );
    let events = h.logged("event", 2).await;
    assert_eq!(
        events[1]["event"],
        "the person changed or added to the goal while it ran"
    );
    // Both came before the goal's first turn ended.
    let goal = h.cli("get_goal", json!({"threadId":id}));
    assert_eq!(goal["status"], "running", "{goal}");
    let passes = |h: &Harness, needle: &str| {
        h.memory_requests()
            .into_iter()
            .filter(|body| body["messages"][1].to_string().contains(needle))
            .count()
    };
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while passes(&h, "What just happened: a goal was created") == 0
        || passes(&h, "notes are sorted by date") == 0
    {
        assert!(std::time::Instant::now() < deadline, "no event passes");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let body = h
        .memory_requests()
        .into_iter()
        .find(|body| {
            body["messages"][1]
                .to_string()
                .contains("What just happened: a goal was created")
        })
        .unwrap();
    let user = body["messages"][1]["content"].as_str().unwrap();
    assert!(
        user.contains("<<<reply-answered\n1. Store notes in notes.txt."),
        "{user}"
    );
    assert!(user.contains("slow goal: build the notes tool"), "{user}");
    // Once the goal completes, no pass carries the goal's words as the person's own again.
    h.watch(&id, Duration::from_millis(100), true).await;
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert_eq!(passes(&h, "<<<person\n/goal slow goal"), 0);
    assert_eq!(passes(&h, "<<<person\nChange: notes are sorted"), 0);
}

/// The file of plan record `name`, wherever the project's memory folder is.
fn plan_file(data: &Path, name: &str) -> Option<String> {
    fn find(dir: &Path, name: &str) -> Option<std::path::PathBuf> {
        for entry in std::fs::read_dir(dir).ok()?.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if let Some(found) = find(&path, name) {
                    return Some(found);
                }
            } else if path.ends_with(format!("plans/{name}.md")) {
                return Some(path);
            }
        }
        None
    }
    std::fs::read_to_string(find(data, name)?).ok()
}

/// Waits up to 20 s for plan record `name` to hold `wanted`; its text.
async fn plan_with(data: &Path, name: &str, wanted: &str) -> String {
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        let text = plan_file(data, name).unwrap_or_default();
        if text.contains(wanted) {
            return text;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no `{wanted}` in: {text}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// A plan the person approved gets a plan record, at once when a goal is made from it: the
/// plan word for word, tied to the chat. A change while the goal runs rewrites only the step it
/// touches and adds its line; when the goal completes Themis copies the goal's evidence as the
/// agent gave it and the pass adds results and lessons. A later chat about the work gets the
/// record in full, and ordinary memories still come from the same passes.
#[tokio::test(flavor = "multi_thread")]
async fn an_approved_plan_keeps_a_record_through_changes_to_done() {
    let mut h = Harness::start(MODELS).await;
    let id = h.new_chat();
    h.send(&id, "Plan the notes tool.", Duration::from_millis(100))
        .await;
    h.cli(
        "send_message",
        json!({"threadId":id,"text":"/goal slow goal: build the notes tool as planned","reasoningEffort":null}),
    );
    let record = plan_with(h.data.path(), "notes-tool", "## Plan").await;
    assert!(record.contains("status: in progress"), "{record}");
    assert!(record.contains(&format!("chat: {id}")), "{record}");
    assert!(
        record.contains(
            "## Plan\n1. Store notes in notes.txt.\n2. Sort notes by title.\n3. Show the count."
        ),
        "{record}"
    );
    // The same pass still keeps an ordinary memory.
    let global = h.cli("list_memories", json!({"scope":"global"}));
    assert!(global.to_string().contains("likes-short-names"), "{global}");

    // A change while the goal runs: only the step it touches is rewritten.
    let message = h.cli(
        "enqueue_message",
        json!({"threadId":id,"text":"Change: sort notes by date."}),
    );
    h.cli(
        "steer_queued_message",
        json!({"threadId":id,"messageId":message["id"]}),
    );
    let record = plan_with(h.data.path(), "notes-tool", "sorted by date").await;
    assert!(
        record.contains("## Decisions\n- sort notes by date — because newest notes matter most ("),
        "{record}"
    );
    // Themis made a task of each plan step; the model added one more.
    assert!(
        record.contains("## Tasks\n- [open] Store notes in notes.txt. ("),
        "{record}"
    );
    assert!(
        record.contains("- [open] Show the count in the footer — \"show the count\" ("),
        "{record}"
    );
    assert!(
        record.contains("1. Store notes in notes.txt.\n2. Sort notes by date.\n3. Show the count."),
        "{record}"
    );
    assert!(
        record.contains("## Changes\n- Notes are sorted by date, not title."),
        "{record}"
    );

    // The goal completes: the evidence as the agent gave it, then results and lessons.
    h.watch(&id, Duration::from_millis(100), true).await;
    let record = plan_with(h.data.path(), "notes-tool", "trailing newline").await;
    assert!(record.contains("status: done"), "{record}");
    assert!(
        record.contains("## Evidence\n- notes.txt exists: ls notes.txt: notes.txt"),
        "{record}"
    );
    assert!(!record.contains("made up by the model"), "{record}");
    assert!(
        record.contains("## Results\nBuilt notes.txt; check with: ls notes.txt"),
        "{record}"
    );

    // A new chat about the work gets the record in full.
    let other = h.new_chat();
    h.send(
        &other,
        "How do I check the notes tool works?",
        Duration::from_millis(100),
    )
    .await;
    let request = h
        .requests
        .lock()
        .unwrap()
        .iter()
        .rev()
        .find(|body| {
            !body["model"]
                .as_str()
                .unwrap_or_default()
                .starts_with("mem-")
                && last_user(body).contains("How do I check the notes tool")
        })
        .cloned()
        .unwrap();
    let system = system(&request);
    assert!(system.contains("- notes-tool (done, "), "{system}");
    // Its decisions and how its tasks ended; results stay in the file.
    assert!(
        system.contains("## Decisions\n- sort notes by date"),
        "{system}"
    );
    // Finished work leaves nothing open: its plan steps are closed with it.
    assert!(
        system.contains("- 4 done: ") && system.contains("Show the count in the footer"),
        "{system}"
    );
    // The step the change replaced was cancelled, not closed as done.
    assert!(
        system.contains(
            "- [cancelled] Sort notes by title. — the plan changed: Notes are sorted by date"
        ),
        "{system}"
    );
    assert!(!system.contains("- [open] "), "{system}");
    assert!(!system.contains("## Results\nBuilt notes.txt"), "{system}");
    assert!(
        system.contains("## Lessons\n- Write notes.txt with a trailing newline"),
        "{system}"
    );
}

/// Tasks outside any plan are kept in the project's tasks file: the next run sees an open one,
/// and once done it is counted, not dropped; a new chat in the project sees it too.
#[tokio::test(flavor = "multi_thread")]
async fn tasks_are_kept_across_turns_and_chats() {
    let mut h = Harness::start(MODELS).await;
    let id = h.new_chat();
    h.send(
        &id,
        "Fix the title, and also write a README.",
        Duration::from_secs(10),
    )
    .await;
    let tasks_file = |h: &Harness| {
        std::fs::read_dir(h.data.path().join("memory/projects"))
            .unwrap()
            .flatten()
            .map(|entry| entry.path().join("plans/TASKS.md"))
            .find(|path| path.exists())
            .map(|path| std::fs::read_to_string(path).unwrap())
            .unwrap_or_default()
    };
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !tasks_file(&h).contains("Write the README") {
        assert!(std::time::Instant::now() < deadline, "no task saved");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let main_system = |h: &Harness, asked: &str| {
        h.requests
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|body| body["model"] == "test-model" && last_user(body).contains(asked))
            .map(system)
            .unwrap()
    };
    let other = h.new_chat();
    h.send(
        &other,
        "What is left to do here?",
        Duration::from_millis(100),
    )
    .await;
    let seen = main_system(&h, "What is left to do here?");
    // The agent is told where its own choices go: the code, not memory.
    assert!(
        seen.contains("Themis does not keep your own choices in memory"),
        "{seen}"
    );
    assert!(
        seen.contains("- [open] Write the README — \"also write a README\" with run steps ("),
        "{seen}"
    );
    h.send(&other, "The README is written.", Duration::from_secs(10))
        .await;
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !tasks_file(&h).contains("[done] Write the README") {
        assert!(std::time::Instant::now() < deadline, "task not marked done");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    h.send(&id, "Anything else open?", Duration::from_millis(100))
        .await;
    let seen = main_system(&h, "Anything else open?");
    assert!(seen.contains("- 1 done: Write the README"), "{seen}");
    // The agent can find a task by its words and read the whole list.
    h.send(
        &id,
        "Please look up the readme task.",
        Duration::from_millis(100),
    )
    .await;
    let results: Vec<String> = h
        .requests
        .lock()
        .unwrap()
        .iter()
        .filter(|body| body["model"] == "test-model")
        .map(last)
        .filter(|message| message["role"] == "tool")
        .map(|message| message["content"].to_string())
        .collect();
    assert!(
        results
            .iter()
            .any(|result| result.contains("[done] Write the README") && result.contains("task")),
        "{results:?}"
    );
    assert!(!seen.contains("[open] Write the README"), "{seen}");
}

/// The pass reads what the turn ran into: its failed steps and the agent's notes along the
/// way, so a lesson can be kept where the work went wrong. The note that ended the turn is
/// its reply, read once, not again in the log.
#[tokio::test(flavor = "multi_thread")]
async fn the_pass_reads_what_the_turn_ran_into() {
    let mut h = Harness::start(MODELS).await;
    let id = h.new_chat();
    h.send(
        &id,
        "Please list the missing folder for me.",
        Duration::from_secs(5),
    )
    .await;
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    let pass = loop {
        if let Some(body) = h.memory_requests().into_iter().find(|body| {
            body["messages"][1]
                .to_string()
                .contains("list the missing folder")
        }) {
            break body;
        }
        assert!(std::time::Instant::now() < deadline, "no pass");
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    let user = pass["messages"][1]["content"].as_str().unwrap();
    assert!(user.contains("<<<ran-into\n- list_dir failed:"), "{user}");
    assert!(!user.contains("- noted: Done."), "{user}");
}

/// The agent can look memory up itself: a query finds index lines, a name reads one in full.
/// It is offered only while memory is on.
#[tokio::test(flavor = "multi_thread")]
async fn the_agent_searches_memory_and_reads_one_in_full() {
    let mut h = Harness::start(MODELS).await;
    let id = h.new_chat();
    h.send(
        &id,
        "The deadline for launch is Friday.",
        Duration::from_secs(10),
    )
    .await;
    h.send(
        &id,
        "Please look up the launch date.",
        Duration::from_secs(5),
    )
    .await;
    let main: Vec<Value> = h
        .requests
        .lock()
        .unwrap()
        .iter()
        .filter(|body| body["model"] == "test-model")
        .cloned()
        .collect();
    let offered = main.last().unwrap()["tools"].to_string();
    assert!(offered.contains("\"search_memory\""), "{offered}");
    let results: Vec<String> = main
        .iter()
        .map(last)
        .filter(|message| message["role"] == "tool")
        .map(|message| message["content"].to_string())
        .collect();
    assert!(
        results
            .iter()
            .any(|result| result.contains("launch-deadline (project")),
        "the search found it: {results:?}"
    );
    assert!(
        results
            .iter()
            .any(|result| result.contains("The launch is on Friday.")),
        "the read gave it in full: {results:?}"
    );
}

/// Auto mode never prompts: where Custom would ask, the reviewer model (the first memory model
/// by default) allows, soft-blocks with a reason and a safer way the agent reads, or hard-blocks
/// and asks the person in a question box; their "Allow once" lets exactly that action run once.
#[tokio::test(flavor = "multi_thread")]
async fn auto_mode_reviews_instead_of_asking() {
    let mut h = Harness::start(MODELS).await;
    let git = std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(h.project.path())
        .status()
        .unwrap();
    assert!(git.success());
    let id = h.new_chat();
    h.cli(
        "set_thread_approval_mode",
        json!({"threadId": id, "mode": "auto"}),
    );
    let tool_result = |h: &Harness| -> String {
        h.requests
            .lock()
            .unwrap()
            .iter()
            .rev()
            .filter(|body| body["model"] == "test-model")
            .map(last)
            .find(|message| message["role"] == "tool")
            .map(|message| message["content"].to_string())
            .unwrap_or_default()
    };
    let root = h.project.path().to_owned();
    let file = |name: &str| root.join(name).exists();

    h.send(&id, "auto-step fine.txt", Duration::from_millis(100))
        .await;
    assert!(file("fine.txt"), "allowed: {}", tool_result(&h));

    h.send(&id, "auto-step soft.txt", Duration::from_millis(100))
        .await;
    assert!(!file("soft.txt"));
    let soft = tool_result(&h);
    assert!(
        soft.contains("Auto mode's reviewer stopped this: it touches a file nobody named")
            && soft.contains("use notes.txt instead"),
        "{soft}"
    );
    let questions = h.cli("list_questions", json!({"threadId": id}));
    assert_eq!(
        questions.as_array().map(Vec::len),
        Some(0),
        "a soft block asks nobody: {questions}"
    );

    h.send(&id, "auto-step hard.txt", Duration::from_millis(100))
        .await;
    assert!(!file("hard.txt"));
    let hard = tool_result(&h);
    assert!(
        hard.contains("Auto mode's reviewer blocked this: it was not asked for"),
        "{hard}"
    );
    let questions = h.cli("list_questions", json!({"threadId": id}));
    let question = &questions[0];
    assert!(
        question["question"]
            .as_str()
            .unwrap()
            .starts_with("Auto mode blocked an action"),
        "{question}"
    );
    // The recommended answer (the safer way) comes first, as in every question box.
    assert_eq!(
        question["choices"],
        json!(["Do it the safer way", "Allow once", "Skip it"]),
        "{question}"
    );
    assert!(
        question["question"]
            .as_str()
            .unwrap()
            .contains("blocked an action: touch hard.txt."),
        "{question}"
    );
    // Each answer says what happens next, with the command itself.
    assert_eq!(
        question["reasons"],
        json!({
            "Allow once": "Runs `touch hard.txt` once, exactly as written.",
            "Do it the safer way": "Instead: leave it out",
            "Skip it": "Leaves `touch hard.txt` out; the agent finishes the rest and tells you what it skipped.",
        }),
        "{question}"
    );

    // The person allows it once: exactly that action runs, once.
    h.cli(
        "answer_question",
        json!({"threadId": id, "questionId": question["id"], "answer": "Allow once"}),
    );
    h.watch(&id, Duration::from_millis(100), true).await;
    h.send(&id, "auto-step hard.txt", Duration::from_millis(100))
        .await;
    assert!(file("hard.txt"), "allowed once: {}", tool_result(&h));
    std::fs::remove_file(root.join("hard.txt")).unwrap();
    h.send(&id, "auto-step hard.txt", Duration::from_millis(100))
        .await;
    assert!(!file("hard.txt"), "only once: {}", tool_result(&h));
}

/// An update that keeps a memory's description but changes its text is applied: the memory
/// was shown to the model in full, so the change is known to be real, not a repeat.
#[tokio::test(flavor = "multi_thread")]
async fn an_update_with_the_same_description_changes_the_text() {
    let mut h = Harness::start(MODELS).await;
    let id = h.new_chat();
    h.send(
        &id,
        "The deadline for launch is Friday.",
        Duration::from_secs(10),
    )
    .await;
    let (lines, _) = h
        .send(
            &id,
            "We add a demo day before the launch deadline.",
            Duration::from_secs(10),
        )
        .await;
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert_eq!(lines[0]["changes"][0]["action"], "updated", "{lines:?}");
    let root = h.project.path().to_string_lossy().into_owned();
    let listed = h.cli(
        "list_memories",
        json!({"scope":"project","projectRoot":root}),
    );
    assert!(
        listed.to_string().contains("with a demo day before it"),
        "{listed}"
    );
}

/// A turn that fails still had the person's words: its pass runs all the same.
#[tokio::test(flavor = "multi_thread")]
async fn a_failed_turn_still_keeps_what_the_person_said() {
    let h = Harness::start(MODELS).await;
    let id = h.new_chat();
    h.cli(
        "send_message",
        json!({"threadId":id,"text":"By the way, I always use pnpm, and this turn fails.","reasoningEffort":null}),
    );
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let pass = loop {
        let found = h.memory_requests().into_iter().find(|body| {
            body["messages"]
                .to_string()
                .contains("I always use pnpm, and this turn fails")
        });
        if let Some(body) = found {
            break body;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no pass after the failed turn"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    assert!(pass["tools"].is_null());
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
