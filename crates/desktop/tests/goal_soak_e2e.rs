//! A long goal through the real shared server and the `themis` CLI, with a scripted
//! model: many turns at a small context budget, so the chat crosses several checkpoints.
//! It checks the harness only (goal continuation, checkpoints, pause and resume, no
//! replayed writes), never what a model would do.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use themis_desktop::server::{Client, Server};
use themis_desktop::state::AppState;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const ITEMS: usize = 14;
const RULE: &str = "RULE-MARKER-7: every file starts with `owner: heron`.";
const GOAL: &str = "GOAL-SOAK write out/item-01.md through out/item-14.md, one per turn";
const CHECK_MARKER: &str = "You decide whether an agent has reached a goal";
const SUMMARY_MARKER: &str = "You summarize agent context";
const TURN_MARKER: &str = "[Goal — runtime, not from the user]";
const SPEC_MARKER: &str = "[Runtime check, not from the user]";
const CHECKPOINT: &str = "Goal/constraints: write the item files in order, one per turn. Unresolved work: the remaining item files. Next action: write the next item file.";
/// Generous: CI runs every test binary at once.
const DEADLINE: Duration = Duration::from_secs(120);

fn cli(data: &Path, command: &str, args: Value) -> Value {
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

fn reply(text: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "choices": [{"message": {"role": "assistant", "content": text}, "finish_reason": "stop"}]
    }))
}

fn system_has(body: &Value, marker: &str) -> bool {
    body["messages"][0]["content"]
        .as_str()
        .is_some_and(|text| text.contains(marker))
}

fn is_check(body: &Value) -> bool {
    system_has(body, CHECK_MARKER)
}

fn is_summary(body: &Value) -> bool {
    system_has(body, SUMMARY_MARKER)
}

fn item_path(item: usize) -> String {
    format!("out/item-{item:02}.md")
}

/// Filler that makes each turn's answer large enough that the 16k budget fills every
/// few turns.
fn padding(item: usize) -> String {
    format!("Item {item} notes: the item lists its owner and fields in order; ").repeat(40)
}

/// The scripted model's memory: which item each turn writes (keyed by the turn's
/// request, so a stream attempt and its plain retry write the same item).
#[derive(Default)]
struct Script {
    turns: HashMap<String, usize>,
    written: usize,
    summaries: usize,
    /// The first goal turn after this many checkpoints waits for `release`.
    hold_after: usize,
    holding: bool,
}

/// Holds a scripted answer until the test opens it, so the pause lands while that
/// turn runs, however slow the machine is.
#[derive(Clone, Default)]
struct Gate(Arc<AtomicBool>);

impl Gate {
    fn open(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    fn wait(&self) {
        let waited = Instant::now();
        while !self.0.load(Ordering::SeqCst) && waited.elapsed() < Duration::from_secs(60) {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

fn respond(
    script: &Mutex<Script>,
    release: &Gate,
    held: &AtomicBool,
    body: &Value,
) -> ResponseTemplate {
    if is_summary(body) {
        script.lock().unwrap().summaries += 1;
        return reply(CHECKPOINT);
    }
    if is_check(body) {
        let written = script.lock().unwrap().written;
        return reply(&if written >= ITEMS {
            json!({"verdict":"reached","reason":format!("All {ITEMS} items written."),"progress":format!("{written} items"),"progressed":true})
        } else {
            json!({"verdict":"not_yet","reason":format!("{written} of {ITEMS} items written."),"progress":format!("{written} items"),"progressed":true})
        }.to_string());
    }
    let messages = body["messages"].as_array().unwrap();
    let last = messages.last().unwrap();
    let text = last["content"].as_str().unwrap_or_default();
    if last["role"] == "tool" {
        return reply("Wrote the item file.");
    }
    if text.contains(SPEC_MARKER) {
        let written = script.lock().unwrap().written;
        return reply(&format!(
            "Checked {} against the rule. {}",
            item_path(written),
            padding(written)
        ));
    }
    if text.contains("RULE-MARKER-7") {
        return reply("Noted the rule.");
    }
    // A goal turn: write the next item.
    let (item, wait) = {
        let mut script = script.lock().unwrap();
        let key = body["messages"].to_string();
        if let Some(item) = script.turns.get(&key) {
            (*item, false)
        } else {
            script.written += 1;
            let item = script.written;
            script.turns.insert(key, item);
            let wait = !script.holding && script.summaries >= script.hold_after;
            script.holding |= wait;
            (item, wait)
        }
    };
    if wait {
        held.store(true, Ordering::SeqCst);
        release.wait();
    }
    let content = format!("owner: heron\n# Item {item:02}\n");
    ResponseTemplate::new(200).set_body_json(json!({"choices":[{"message":{
        "role":"assistant","content":null,
        "tool_calls":[{"id":format!("write-{item:02}"),"type":"function","function":{
            "name":"write_file",
            "arguments":json!({"file_path":item_path(item),"content":content,"append":false}).to_string()
        }}]
    },"finish_reason":"tool_calls"}]}))
}

/// Polls `get_goal` through the CLI until `done` holds.
fn goal_until(data: &Path, thread: &str, what: &str, done: impl Fn(&Value) -> bool) -> Value {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let goal = cli(data, "get_goal", json!({"threadId": thread}));
        if done(&goal) {
            return goal;
        }
        assert!(Instant::now() < deadline, "{what}: goal is {goal}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn running(data: &Path, thread: &str) -> bool {
    cli(data, "get_thread", json!({"threadId": thread}))["running"] == true
}

/// What a finished soak leaves to check: every provider request in order, every event
/// for the chat, and the project folder.
struct Soak {
    bodies: Vec<Value>,
    events: Vec<Value>,
    project: tempfile::TempDir,
    started: Instant,
}

impl Soak {
    fn kinds(&self) -> Vec<&str> {
        self.events
            .iter()
            .map(|event| event["kind"].as_str().unwrap_or_default())
            .collect()
    }

    /// Indexes of the summarizer requests (one per checkpoint attempt).
    fn summaries(&self) -> Vec<usize> {
        (0..self.bodies.len())
            .filter(|&index| is_summary(&self.bodies[index]))
            .collect()
    }
}

fn ordinary(body: &&Value) -> bool {
    !is_check(body) && !is_summary(body)
}

/// Runs the goal to the end through the app server and the CLI: a first message with a
/// rule, `/goal`, a pause during the first turn after the second checkpoint (checked to
/// hold, with the clock stopped), a resume, and `reached` (checked to start no further
/// turn).
async fn soak() -> Soak {
    let started = Instant::now();
    // The system prompt lists the user's skills (~/.agents, ~/.codex, ~/.claude), and at
    // 16k a large personal catalog alone outgrows the budget. An empty home keeps the
    // prompt the same on every machine. Every test here sets it to an empty folder, so
    // changing the process environment races with nothing that matters.
    let home = tempfile::tempdir().unwrap();
    std::env::set_var("HOME", home.path());
    std::env::set_var("USERPROFILE", home.path());
    // The scripted provider gets the synthetic key only, never one from the environment.
    std::env::remove_var("OPENCODE_KEY");
    let llm = MockServer::start().await;
    let script = Arc::new(Mutex::new(Script {
        hold_after: 2,
        ..Script::default()
    }));
    let release = Gate::default();
    let held = Arc::new(AtomicBool::new(false));
    {
        let (script, release, held) = (script.clone(), release.clone(), held.clone());
        Mock::given(method("POST"))
            .respond_with(move |request: &Request| {
                let body: Value = request.body_json().unwrap_or(Value::Null);
                respond(&script, &release, &held, &body)
            })
            .mount(&llm)
            .await;
    }
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(project.path().join(".themis")).unwrap();
    std::fs::write(
        project.path().join(".themis/config.jsonc"),
        r#"{"context_token_budget":16000,"recent_messages":4,"total_turns":8}"#,
    )
    .unwrap();
    let state = AppState::new_for_test(data.path().join("settings.json"));
    state
        .set_secret("go".into(), "synthetic-test-key".into())
        .await
        .unwrap();
    state.set_go_base_url_override(Some(llm.uri()));
    let serving = tokio::spawn(Server::bind(state, data.path()).await.unwrap().run());
    let client = Client::new(data.path().into());
    let mut stream = client.subscribe().await.unwrap();
    let dir = data.path().to_owned();
    let thread = cli(
        &dir,
        "create_thread",
        json!({"projectRoot":project.path(),"provider":"go","model":"test-model"}),
    );
    let id = thread["id"].as_str().unwrap().to_owned();
    cli(
        &dir,
        "set_thread_approval_mode",
        json!({"threadId": id, "mode": "yolo"}),
    );
    // Every event for this chat, in order.
    let events = Arc::new(Mutex::new(Vec::<Value>::new()));
    let collector = {
        let (events, id) = (events.clone(), id.clone());
        tokio::spawn(async move {
            while let Ok(item) = Client::next_event(&mut stream).await {
                if item["name"] == "thread-event" && item["payload"]["thread_id"] == id.as_str() {
                    events
                        .lock()
                        .unwrap()
                        .push(item["payload"]["event"].clone());
                }
            }
        })
    };
    let finished = |events: &Mutex<Vec<Value>>| {
        events
            .lock()
            .unwrap()
            .iter()
            .any(|event| event["kind"] == "finished")
    };

    // An ordinary first message carries the rule; the goal does not repeat it.
    cli(&dir, "send_message", json!({"threadId": id, "text": RULE}));
    let deadline = Instant::now() + DEADLINE;
    while !finished(&events) {
        assert!(
            Instant::now() < deadline,
            "the first message never finished: {:?}",
            events.lock().unwrap()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    cli(
        &dir,
        "send_message",
        json!({"threadId": id, "text": format!("/goal {GOAL}")}),
    );

    // Pause while the first goal turn after the second checkpoint runs.
    let deadline = Instant::now() + DEADLINE;
    while !held.load(Ordering::SeqCst) {
        assert!(
            Instant::now() < deadline,
            "never reached a turn after two checkpoints; goal: {}",
            cli(&dir, "get_goal", json!({"threadId": id}))
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let paused = cli(&dir, "pause_goal", json!({"threadId": id}));
    assert_eq!(paused["status"], "paused", "{paused}");
    release.open();
    let deadline = Instant::now() + DEADLINE;
    while running(&dir, &id) {
        assert!(Instant::now() < deadline, "the held turn never finished");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let written_at_pause = script.lock().unwrap().written;
    let requests_at_pause = llm.received_requests().await.unwrap().len();
    let goal = cli(&dir, "get_goal", json!({"threadId": id}));
    assert_eq!(goal["status"], "paused", "{goal}");
    assert!(
        goal.get("running_since").is_none_or(Value::is_null),
        "{goal}"
    );
    let elapsed = goal["active_ms"].as_u64().unwrap();
    // The pause holds after the turn: no further turn or check, and the clock stops.
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(script.lock().unwrap().written, written_at_pause);
    assert_eq!(
        llm.received_requests().await.unwrap().len(),
        requests_at_pause,
        "nothing is sent while the goal is paused"
    );
    let goal = cli(&dir, "get_goal", json!({"threadId": id}));
    assert_eq!(goal["status"], "paused", "{goal}");
    assert_eq!(
        goal["active_ms"].as_u64().unwrap(),
        elapsed,
        "elapsed time does not grow while paused: {goal}"
    );

    // Resume continues to the end, and the goal stops on reached.
    let resumed = cli(&dir, "resume_goal", json!({"threadId": id}));
    assert_eq!(resumed["status"], "running", "{resumed}");
    let goal = goal_until(&dir, &id, "goal never reached", |goal| {
        goal["status"] == "reached"
    });
    assert!(
        goal["active_ms"].as_u64().unwrap() > elapsed,
        "elapsed time grows again once resumed: {goal}"
    );
    let deadline = Instant::now() + DEADLINE;
    while running(&dir, &id) {
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let requests_at_reached = llm.received_requests().await.unwrap().len();
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(
        llm.received_requests().await.unwrap().len(),
        requests_at_reached,
        "no turn starts after reached"
    );
    let goal = cli(&dir, "get_goal", json!({"threadId": id}));
    assert_eq!(goal["status"], "reached", "{goal}");
    assert_eq!(goal["turns"], ITEMS, "{goal}");
    assert_eq!(script.lock().unwrap().written, ITEMS);

    let bodies = llm
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(|request| request.body_json().unwrap_or(Value::Null))
        .collect();
    collector.abort();
    client.call("shutdown", json!({})).await.unwrap();
    serving.await.unwrap().unwrap();
    let events = events.lock().unwrap().clone();
    Soak {
        bodies,
        events,
        project,
        started,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn goal_soak_e2e() {
    let soak = soak().await;
    let bodies = &soak.bodies;

    // Every turn after the first started by itself: only the rule, the /goal and the
    // resume came from the test.
    for turn in 2..=ITEMS {
        let marker = format!("{TURN_MARKER} Turn {turn}.");
        assert!(
            bodies
                .iter()
                .filter(ordinary)
                .any(|body| body["messages"].to_string().contains(&marker)),
            "goal turn {turn} never started"
        );
    }

    // Every checkpoint succeeded and shrank the next request.
    let kinds = soak.kinds();
    let checkpoints = kinds.iter().filter(|k| **k == "context_checkpoint").count();
    assert!(
        checkpoints >= 4,
        "only {checkpoints} checkpoints: {kinds:?}"
    );
    assert_eq!(
        kinds.iter().filter(|k| **k == "context_compacting").count(),
        checkpoints,
        "every checkpoint that started was installed: {kinds:?}"
    );
    assert!(
        !kinds
            .iter()
            .any(|k| *k == "failed" || *k == "context_suspended"),
        "{kinds:?}"
    );
    let summaries = soak.summaries();
    assert_eq!(summaries.len(), checkpoints, "one summary request each");
    let size = |body: &Value| body["messages"].to_string().len();
    for &at in &summaries {
        let before = bodies[..at].iter().rev().find(ordinary).unwrap();
        let after = bodies[at..].iter().find(ordinary).unwrap();
        assert!(
            size(after) < size(before),
            "checkpoint at request {at}: next request {} bytes, last before {} bytes",
            size(after),
            size(before)
        );
    }
    // The goal is in every model request after a checkpoint.
    for body in bodies[summaries[0]..].iter().filter(ordinary) {
        assert!(
            body["messages"].to_string().contains("GOAL-SOAK"),
            "a request after a checkpoint lacks the goal"
        );
    }
    // Our code hands the first message's rule to the summarizer when it is first
    // summarized, and every later checkpoint is built on the previous one, so a rule the
    // summary kept is handed on. (What the summary model keeps is a model measure.)
    assert!(
        bodies[summaries[0]]["messages"]
            .to_string()
            .contains("RULE-MARKER-7"),
        "the first summarizer request lacks the first message's rule"
    );
    for &at in &summaries[1..] {
        let request = bodies[at]["messages"][1]["content"].as_str().unwrap();
        let previous = request
            .split("PREVIOUS TASK STATE TO UPDATE")
            .nth(1)
            .unwrap_or_default();
        assert!(
            previous.contains("Unresolved work: the remaining item files."),
            "the summarizer request at {at} lacks the previous checkpoint"
        );
    }

    // No item was written twice: one write_file call per item path, and one tool run
    // per item.
    let mut calls: HashMap<String, HashSet<String>> = HashMap::new();
    for body in bodies.iter().filter(ordinary) {
        for message in body["messages"].as_array().unwrap() {
            for call in message["tool_calls"].as_array().into_iter().flatten() {
                if call["function"]["name"] != "write_file" {
                    continue;
                }
                let args: Value =
                    serde_json::from_str(call["function"]["arguments"].as_str().unwrap())
                        .unwrap_or(Value::Null);
                if let Some(path) = args["file_path"].as_str() {
                    calls
                        .entry(path.to_owned())
                        .or_default()
                        .insert(call["id"].as_str().unwrap_or_default().to_owned());
                }
            }
        }
    }
    assert_eq!(calls.len(), ITEMS, "{calls:?}");
    assert!(calls.values().all(|ids| ids.len() == 1), "{calls:?}");
    let writes = soak
        .events
        .iter()
        .filter(|event| event["kind"] == "tool_finished" && event["tool"] == "write_file")
        .count();
    assert_eq!(writes, ITEMS, "each item file was written by one tool run");
    for item in 1..=ITEMS {
        let text = std::fs::read_to_string(soak.project.path().join(item_path(item))).unwrap();
        assert!(text.starts_with("owner: heron"), "{text}");
    }
    eprintln!(
        "goal soak: {ITEMS} turns, {checkpoints} checkpoints, {} requests, {:?}",
        bodies.len(),
        soak.started.elapsed()
    );
}

/// Every checkpoint request carries the chat's first message verbatim, so a rule given
/// there never depends on an earlier summary having kept it.
#[tokio::test(flavor = "multi_thread")]
async fn every_summarizer_request_carries_the_first_messages_rule() {
    let soak = soak().await;
    let missing: Vec<usize> = soak
        .summaries()
        .into_iter()
        .enumerate()
        .filter(|(_, at)| {
            !soak.bodies[*at]["messages"]
                .to_string()
                .contains("RULE-MARKER-7")
        })
        .map(|(checkpoint, _)| checkpoint + 1)
        .collect();
    assert!(
        missing.is_empty(),
        "checkpoints whose summarizer request lacks the rule: {missing:?}"
    );
}
