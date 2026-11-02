//! The experiment switches for automatic memory and to-do lists, through the shared server and
//! the `themis` CLI with a scripted model (no real model is called):
//! - `memory.auto`: after a turn, the memory pass saves a preference the person mentioned in
//!   passing (no "remember") with the right scope and the chat's "Remembered" line; it saves
//!   nothing when nothing is new, refuses a secret, doesn't duplicate, skips a turn in which the
//!   agent saved, and is off by default.
//! - `goal.seed_todo`: a new goal's list is seeded before its first turn, which reads it, and
//!   the goal is reached once the items are done.
//! - `hooks.built_in`: each built-in hook adds its text (to the main agent and to subagents) only
//!   while its switch is on, switched through Integrations › Hooks' commands.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use themis_desktop::server::{Client, Server};
use themis_desktop::state::AppState;
use wiremock::{matchers::method, Mock, MockServer, Request, ResponseTemplate};

const PASS: &str = "You decide what an assistant should remember";
const SEED: &str = "You turn a goal into a short to-do list";
const CHECK: &str = "You decide whether an agent has reached a goal";
const SUBAGENT: &str = "You are a subagent";
const SUMMARY: &str = "You summarize agent context";
const TODO_SKILL: &str = "# To-do list";
const MEMORY_SKILL: &str = "# Memory\n";
const SUBAGENT_MEMORY_NOTE: &str = "As a subagent, your memory tool only reads.";
const STILL_OPEN: &str = "[Still open — runtime, not from the user]";
const REMINDER: &str = "[Memory reminder — runtime, not from the user]";
const SECRET: &str = "ghp_abcdefghijklmnopqrstuvwxyz0123456789AB";

fn cli(dir: &Path, command: &str, args: Value) -> Value {
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
    assert!(
        output.status.success(),
        "{command}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap_or(Value::Null)
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

fn system(body: &Value) -> &str {
    body["messages"][0]["content"].as_str().unwrap_or_default()
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

/// What the memory pass answers, by what the person said that turn.
fn pass_answer(body: &Value) -> String {
    let user = body["messages"][1]["content"].as_str().unwrap_or_default();
    let person = user.split("<<<person").nth(1).unwrap_or_default();
    if person.contains("pnpm") {
        json!({"memories":[{"action":"save","scope":"global","name":"uses-pnpm","type":"user",
            "description":"Uses pnpm, not npm","body":"The person uses pnpm for JavaScript projects."}]})
        .to_string()
    } else if person.contains("deploy token") {
        json!({"memories":[{"action":"save","scope":"project","name":"deploy-token","type":"project",
            "description":"How to deploy","body":format!("Deploy with token {SECRET}")}]})
        .to_string()
    } else {
        r#"{"memories":[]}"#.to_owned()
    }
}

/// What the main agent does with each message.
fn main_answer(body: &Value) -> ResponseTemplate {
    if last(body)["role"] == "tool" {
        return text_reply("Done.");
    }
    let user = last_user(body);
    let wire = body["messages"].to_string();
    match () {
        _ if user.starts_with("remember-it") => call(
            "memory",
            json!({"action":"save","scope":"global","name":"short-replies","type":"feedback",
                "description":"Keep replies short","body":"Keep replies short."}),
        ),
        _ if user.starts_with("spawn-one") => call(
            "spawn_subagents",
            json!({"say":"Starting a helper.","subagents":[{"name":"Helper","brief":"Look around."}]}),
        ),
        _ if user.starts_with("plan-it") => call(
            "todo_write",
            json!({"todos":[{"title":"Write the parser","status":"in_progress"},{"title":"Write its tests","status":"pending"}]}),
        ),
        _ if user.starts_with("write-file") => call(
            "write_file",
            json!({"file_path": format!("{}.txt", user.lines().next().unwrap_or_default().replace(' ', "-")), "content":"x", "append": false}),
        ),
        // A goal turn finishes the features on its list (read at the turn's start), and leaves
        // the last item for the check.
        _ if wire.contains("[pending; from the goal] Tasks can be added") => call(
            "todo_write",
            json!({"todos":[
                {"id":"t1","title":"Tasks can be added and ticked off","status":"completed","outcome":"added and ticked three"},
                {"id":"t2","title":"CSV export downloads every task","status":"completed","outcome":"downloaded 3 rows"},
                {"id":"t3","title":"Verify end to end","status":"pending"}]}),
        ),
        _ if user.starts_with("filler") => text_reply("Noted."),
        _ => text_reply("Fixed the build."),
    }
}

struct Harness {
    data: tempfile::TempDir,
    project: tempfile::TempDir,
    events: tokio::net::TcpStream,
    _serving: tokio::task::JoinHandle<Result<(), String>>,
    _provider: MockServer,
    requests: Arc<Mutex<Vec<Value>>>,
}

impl Harness {
    async fn start(user_config: Option<&str>, project_config: Option<&str>) -> Self {
        let data = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        // A repository, so the end of a turn can tell which files it changed.
        let init = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(project.path())
            .status()
            .unwrap();
        assert!(init.success());
        if let Some(config) = user_config {
            std::fs::write(data.path().join("runtime.jsonc"), config).unwrap();
        }
        if let Some(config) = project_config {
            std::fs::create_dir_all(project.path().join(".themis")).unwrap();
            std::fs::write(project.path().join(".themis/config.jsonc"), config).unwrap();
        }
        let provider = MockServer::builder().start().await;
        let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
        let seen = Arc::clone(&requests);
        Mock::given(method("POST"))
            .respond_with(move |request: &Request| {
                let body: Value = request.body_json().unwrap();
                seen.lock().unwrap().push(body.clone());
                let head = system(&body);
                if head.starts_with(PASS) {
                    return text_reply(&pass_answer(&body));
                }
                if head.starts_with(SEED) {
                    return text_reply(r#"{"items":[{"title":"Tasks can be added and ticked off"},{"title":"CSV export downloads every task","details":"every row"}]}"#);
                }
                if head.contains(CHECK) {
                    // The check completes the seeded verification item on the turn's evidence.
                    return text_reply(
                        r#"{"verdict":"reached","reason":"Everything works.","progressed":true,"complete":[{"id":"t3","outcome":"clicked through every feature"}]}"#,
                    );
                }
                if head.contains(SUMMARY) {
                    return text_reply("Goal/constraints: keep notes. Unresolved work: none. Parked work: none. Next action: carry on.");
                }
                if head.starts_with(SUBAGENT) {
                    return text_reply("Sub done.");
                }
                main_answer(&body)
            })
            .mount(&provider)
            .await;
        let state = AppState::new_for_test(data.path().join("settings.json"));
        state
            .set_secret("go".into(), "synthetic-test-key".into())
            .await
            .unwrap();
        state.set_go_base_url_override(Some(provider.uri()));
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
        }
    }

    fn cli(&self, command: &str, args: Value) -> Value {
        cli(self.data.path(), command, args)
    }

    fn new_chat(&self) -> String {
        let thread = self.cli(
            "create_thread",
            json!({"projectRoot":self.project.path(),"provider":"go","model":"test-model"}),
        );
        let id = thread["id"].as_str().unwrap().to_owned();
        self.cli(
            "set_thread_approval_mode",
            json!({"threadId": id, "mode": "yolo"}),
        );
        id
    }

    /// Sends one message and waits for its run to end, then `settle` longer for what follows
    /// it; the memory lines it saw.
    async fn send(&mut self, id: &str, text: &str, settle: Duration) -> Vec<Value> {
        self.cli(
            "send_message",
            json!({"threadId":id,"text":text,"reasoningEffort":null}),
        );
        let mut found = Vec::new();
        let mut finished = false;
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
                return found;
            };
            let item = item.unwrap();
            if item["name"] != "thread-event" || item["payload"]["thread_id"] != id {
                continue;
            }
            let event = &item["payload"]["event"];
            let kind = event["kind"].as_str().unwrap_or_default();
            if kind == "memory_changed" {
                found.push(event.clone());
            }
            if ["finished", "failed", "incomplete"].contains(&kind) && !finished {
                assert_eq!(kind, "finished", "{event}");
                finished = true;
            }
        }
    }

    /// Requests of one kind, by their system message's start.
    fn requests_of(&self, head: &str) -> Vec<Value> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|body| system(body).starts_with(head))
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
                !system(body).starts_with(SUBAGENT)
                    && last_user(body).starts_with(text)
                    && last(body)["role"] == "user"
            })
            .cloned()
            .unwrap_or_else(|| panic!("no request for {text}"))
    }

    /// Every diagnostics line of `area`/`event`, and all the log text.
    fn logs(&self, area: &str, event: &str) -> (Vec<Value>, String) {
        let mut text = String::new();
        if let Ok(entries) = std::fs::read_dir(self.data.path().join("logs")) {
            for entry in entries.flatten() {
                text.push_str(&std::fs::read_to_string(entry.path()).unwrap_or_default());
            }
        }
        let lines = text
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .filter(|line| line["service"] == area && line["event"] == event)
            .collect();
        (lines, text)
    }

    /// Waits until there are `count` diagnostics lines of `area`/`event`.
    async fn logged(&self, area: &str, event: &str, count: usize) -> Vec<Value> {
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        loop {
            let (lines, _) = self.logs(area, event);
            if lines.len() >= count || std::time::Instant::now() > deadline {
                assert!(lines.len() >= count, "{area}/{event}: {lines:?}");
                return lines;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

fn fields(line: &Value) -> &Value {
    &line["details"]
}

#[tokio::test(flavor = "multi_thread")]
async fn the_memory_pass_notices_a_preference_and_skips_refuses_and_dedupes() {
    let mut h = Harness::start(Some(r#"{"memory":{"auto":true}}"#), None).await;
    let id = h.new_chat();

    // A preference said in passing, with no "remember": saved for every project, with its line.
    let lines = h
        .send(
            &id,
            "By the way, I always use pnpm. Please fix the build.",
            Duration::from_secs(5),
        )
        .await;
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert_eq!(lines[0]["action"], "saved");
    assert_eq!(lines[0]["scope"], "global");
    assert_eq!(lines[0]["name"], "uses-pnpm");
    let saved = std::fs::read_to_string(h.data.path().join("memory/global/uses-pnpm.md")).unwrap();
    assert!(saved.contains("type: user"), "{saved}");
    // The pass read the turn and the (empty) indexes, as data.
    let passes = h.requests_of(PASS);
    assert_eq!(passes.len(), 1);
    let user = passes[0]["messages"][1]["content"].as_str().unwrap();
    assert!(
        user.contains("<<<person\nBy the way, I always use pnpm."),
        "{user}"
    );
    assert!(
        user.contains("<<<assistant-reply\nFixed the build.\n>>>"),
        "{user}"
    );
    assert!(passes[0]["tools"].is_null(), "the pass offers no tools");
    let logged = h.logged("memory", "auto_pass", 1).await;
    let first = fields(&logged[0]);
    assert_eq!(first["outcome"], "saved", "{first}");
    assert_eq!(first["counts"]["saved"], 1);
    assert_eq!(first["prompt_tokens"], 321);
    // The next chat knows it.
    let next = h.new_chat();
    h.send(&next, "hello", Duration::from_millis(500)).await;
    assert!(system(&h.first_request_of("hello")).contains("uses-pnpm"));

    // Nothing new: nothing saved.
    let lines = h
        .send(&id, "Thanks, that looks right.", Duration::from_secs(3))
        .await;
    assert!(lines.is_empty(), "{lines:?}");
    // A secret: refused, never written.
    let lines = h
        .send(
            &id,
            &format!("The deploy token is {SECRET}, use it."),
            Duration::from_secs(3),
        )
        .await;
    assert!(lines.is_empty(), "{lines:?}");
    // The same preference again: no duplicate.
    let lines = h
        .send(&id, "Again, pnpm please.", Duration::from_secs(3))
        .await;
    assert!(lines.is_empty(), "{lines:?}");
    let logged = h.logged("memory", "auto_pass", 5).await;
    let outcomes: Vec<&Value> = logged.iter().map(|line| &fields(line)["outcome"]).collect();
    assert_eq!(outcomes[2], "nothing", "{logged:?}");
    assert_eq!(fields(&logged[3])["counts"]["sensitive"], 1, "{logged:?}");
    assert_eq!(fields(&logged[4])["counts"]["duplicate"], 1, "{logged:?}");
    let global: Vec<_> = std::fs::read_dir(h.data.path().join("memory/global"))
        .unwrap()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(global.len(), 2, "one memory and the index: {global:?}");
    let project_dir = h.data.path().join("memory/projects");
    let all = format!(
        "{:?}",
        std::fs::read_dir(&project_dir)
            .map(|entries| entries.flatten().count())
            .unwrap_or(0)
    );
    assert_eq!(all, "0", "the secret was never saved anywhere");

    // A turn in which the agent saved: the pass skips it.
    let before = h.requests_of(PASS).len();
    let lines = h.send(&id, "remember-it", Duration::from_secs(2)).await;
    assert_eq!(lines.len(), 1, "only the agent's own save: {lines:?}");
    assert_eq!(h.requests_of(PASS).len(), before);
    let skipped = h.logged("memory", "auto_pass_skipped", 1).await;
    assert_eq!(fields(&skipped[0])["reason"], "saved_in_turn");
    // Metadata only: never what was said or saved.
    let (_, text) = h.logs("memory", "auto_pass");
    assert!(!text.contains("pnpm") && !text.contains(SECRET), "{text}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_seeded_goal_reads_its_list_and_is_reached_once_its_items_are_done() {
    let mut h = Harness::start(Some(r#"{"goal":{"seed_todo":true}}"#), None).await;
    let id = h.new_chat();
    h.send(
        &id,
        "/goal Build a todo app with CSV export",
        Duration::from_millis(500),
    )
    .await;
    // One seeding request: the empty project counts as new, so set-up may come first.
    let seeds = h.requests_of(SEED);
    assert_eq!(seeds.len(), 1);
    let user = seeds[0]["messages"][1]["content"].as_str().unwrap();
    assert!(user.contains("a new, empty project"), "{user}");
    assert!(user.contains("Build a todo app with CSV export"), "{user}");
    // The goal's first turn opened with the seeded list.
    let first = h.first_request_of("/goal Build a todo app");
    let opening = last_user(&first);
    assert!(
        opening.contains("[pending; from the goal] Tasks can be added and ticked off"),
        "{opening}"
    );
    assert!(opening.contains("Verify end to end"), "{opening}");
    // The agent finishes the items, the check accepts them, and the goal is reached.
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    loop {
        let goal = h.cli("get_goal", json!({"threadId": id}));
        if goal["status"] == "reached" {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "{goal}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let list = h.cli("get_todo", json!({"threadId": id}));
    let items = list["items"].as_array().unwrap();
    assert_eq!(items.len(), 3, "{list}");
    assert!(items.iter().all(|item| item["source"] == "goal"), "{list}");
    assert!(
        items.iter().all(|item| item["status"] == "completed"),
        "{list}"
    );
    // The agent ticked the features; the check completed "Verify end to end" itself.
    assert_eq!(items[2]["title"], "Verify end to end");
    assert!(!items[2]["closed"].is_null(), "{list}");
    assert!(items[0]["closed"].is_null(), "{list}");
    let seeded = h.logged("todo", "seed", 1).await;
    assert_eq!(fields(&seeded[0])["outcome"], "seeded");
    assert_eq!(fields(&seeded[0])["items"], 3);

    // Seeding is off by default: another chat's goal starts with an empty list.
    let mut off = Harness::start(None, None).await;
    let other = off.new_chat();
    off.send(&other, "/goal Build it", Duration::from_millis(300))
        .await;
    assert!(off.requests_of(SEED).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn built_in_hooks_add_their_text_to_agents_and_subagents_only_while_switched_on() {
    let mut h = Harness::start(None, None).await;
    let id = h.new_chat();
    let view = h.cli("get_built_in_hooks", json!({}));
    let hooks = view["hooks"].as_array().unwrap();
    assert_eq!(hooks.len(), 3, "{view}");
    assert!(hooks.iter().all(|hook| hook["enabled"] == false), "{view}");

    // All off (the default): no skills, no note, and no memory pass.
    h.send(&id, "hello", Duration::from_millis(300)).await;
    assert!(!system(&h.first_request_of("hello")).contains(TODO_SKILL));
    h.send(&id, "spawn-one", Duration::from_millis(500)).await;
    let sub = h.requests_of(SUBAGENT);
    assert!(!sub.is_empty());
    assert!(!system(&sub[0]).contains(TODO_SKILL));
    h.send(&id, "plan-it", Duration::from_millis(300)).await;
    h.send(&id, "write-file one", Duration::from_millis(300))
        .await;
    h.send(&id, "next one", Duration::from_millis(300)).await;
    assert!(!last_user(&h.first_request_of("next one")).contains(STILL_OPEN));
    assert!(
        h.requests_of(PASS).is_empty(),
        "the memory pass is off by default"
    );

    // Switched on through Integrations › Hooks: saved in runtime.jsonc, with a fresh revision.
    let mut revision = view["revision"].clone();
    for hook in ["skills_at_start", "still_open"] {
        let view = h.cli(
            "update_built_in_hook",
            json!({"id": hook, "enabled": true, "revision": revision}),
        );
        revision = view["revision"].clone();
    }
    let config = std::fs::read_to_string(h.data.path().join("runtime.jsonc")).unwrap();
    assert!(config.contains("\"skills_at_start\": true"), "{config}");
    let stale = std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
        .arg("--data-dir")
        .arg(h.data.path())
        .args([
            "call",
            "update_built_in_hook",
            r#"{"id":"save_reminder","enabled":true,"revision":"stale"}"#,
        ])
        .env("HOME", h.data.path())
        .output()
        .unwrap();
    assert!(!stale.status.success(), "a stale revision is refused");

    h.send(&id, "hello again", Duration::from_millis(300)).await;
    let main = system(&h.first_request_of("hello again")).to_owned();
    assert!(
        main.contains(TODO_SKILL) && main.contains(MEMORY_SKILL),
        "{main}"
    );
    let before = h.requests_of(SUBAGENT).len();
    h.send(&id, "spawn-one again", Duration::from_millis(500))
        .await;
    let sub = h.requests_of(SUBAGENT);
    assert!(sub.len() > before);
    let sub_system = system(&sub[before]);
    assert!(sub_system.contains(TODO_SKILL), "{sub_system}");
    assert!(sub_system.contains(MEMORY_SKILL), "{sub_system}");
    assert!(sub_system.contains(SUBAGENT_MEMORY_NOTE), "{sub_system}");
    // Files changed while items are open: the next turn starts by naming them.
    h.send(&id, "plan-it again", Duration::from_millis(300))
        .await;
    h.send(&id, "write-file two", Duration::from_millis(300))
        .await;
    h.send(&id, "next two", Duration::from_millis(300)).await;
    let opening = last_user(&h.first_request_of("next two"));
    assert!(opening.contains(STILL_OPEN), "{opening}");
    assert!(opening.contains("Write its tests"), "{opening}");
    // Only once, and not after a turn that changed nothing.
    h.send(&id, "next three", Duration::from_millis(300)).await;
    assert!(!last_user(&h.first_request_of("next three")).contains(STILL_OPEN));
    // Off again: nothing.
    h.cli(
        "update_built_in_hook",
        json!({"id": "still_open", "enabled": false, "revision": revision}),
    );
    h.send(&id, "write-file three", Duration::from_millis(300))
        .await;
    h.send(&id, "next four", Duration::from_millis(300)).await;
    assert!(!last_user(&h.first_request_of("next four")).contains(STILL_OPEN));
    let (lines, _) = h.logs("hook", "built_in");
    assert!(lines
        .iter()
        .any(|line| fields(line)["hook"] == "still_open"));
}

/// With `save_reminder` on, the agent is reminded once before its context is compacted; with
/// it off, never.
#[tokio::test(flavor = "multi_thread")]
async fn the_save_reminder_comes_before_a_checkpoint_only_while_switched_on() {
    for on in [true, false] {
        let config = format!(r#"{{"hooks":{{"built_in":{{"save_reminder":{on}}}}}}}"#);
        let mut h = Harness::start(
            Some(&config),
            Some(r#"{"context_token_budget":24000,"recent_messages":4,"total_turns":8}"#),
        )
        .await;
        let id = h.new_chat();
        for index in 0..40 {
            let filler = format!(
                "filler {index}: {}",
                format!("detail {index} about beds, soil and watering; ").repeat(60)
            );
            h.send(&id, &filler, Duration::from_millis(100)).await;
            if !h.requests_of(SUMMARY).is_empty() {
                break;
            }
        }
        let requests = h.requests.lock().unwrap().clone();
        let checkpoint = requests
            .iter()
            .position(|body| system(body).contains(SUMMARY))
            .expect("a checkpoint was taken");
        let reminded: Vec<usize> = requests
            .iter()
            .enumerate()
            .filter(|(_, body)| body["messages"].to_string().contains(REMINDER))
            .map(|(index, _)| index)
            .collect();
        if on {
            assert!(
                reminded.first().is_some_and(|first| *first < checkpoint),
                "reminded before the checkpoint at {checkpoint}: {reminded:?}"
            );
        } else {
            assert!(reminded.is_empty(), "{reminded:?}");
        }
    }
}
