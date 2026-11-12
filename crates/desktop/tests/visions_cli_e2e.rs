//! Visions through the real app server and `themis call`: profiles and channels with revisions,
//! channel logs, the chats Visions own (out of the plain list, held when the person stops or
//! takes one over, released when a channel goes), Settings › Visions, and an agent's credential
//! refused on every method. In-process app server, isolated app data, synthetic credentials,
//! scripted provider.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::Output;
use std::time::Duration;

use serde_json::{json, Value};
use themis_desktop::server::{Client, Server};
use themis_desktop::state::{AppState, Hold, OwnedKind, Owner};
use themis_desktop::types::ProviderKind;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

#[allow(dead_code)]
mod common;

/// One app server per process installs the run-credential issuer, so these tests take turns.
static ONE_SERVER: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Every Visions method of the shared server.
const METHODS: [&str; 13] = [
    "list_visions",
    "save_vision",
    "pause_vision",
    "delete_vision",
    "list_channels",
    "save_channel",
    "delete_channel",
    "get_channel_page",
    "post_channel_message",
    "list_channel_threads",
    "hand_back_thread",
    "get_visions_settings",
    "update_visions_settings",
];

struct Fixture {
    _dir: tempfile::TempDir,
    _repo: tempfile::TempDir,
    data: PathBuf,
    project: PathBuf,
    state: AppState,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    served: Option<tokio::task::JoinHandle<Result<(), String>>>,
}

async fn fixture() -> Fixture {
    let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let data = dir.path().canonicalize().unwrap().join("app-data");
    std::fs::create_dir_all(&data).unwrap();
    let (repo, project) = common::init_repo();
    let state = AppState::new_for_test(data.join("settings.json"));
    let server = Server::bind(state.clone(), &data)
        .await
        .unwrap()
        .with_agent_cli(PathBuf::from(env!("CARGO_BIN_EXE_themis")));
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let served = tokio::spawn(server.run_until(async {
        let _ = stopped.await;
    }));
    Fixture {
        _dir: dir,
        _repo: repo,
        data,
        project: project.canonicalize().unwrap(),
        state,
        stop: Some(stop),
        served: Some(served),
    }
}

impl Fixture {
    async fn shut_down(mut self) {
        let _ = self.stop.take().unwrap().send(());
        self.served.take().unwrap().await.unwrap().unwrap();
    }

    fn port(&self) -> u16 {
        let text = std::fs::read_to_string(self.data.join("server.json")).unwrap();
        serde_json::from_str::<Value>(&text).unwrap()["port"]
            .as_u64()
            .unwrap() as u16
    }

    /// `themis` as the person (discovery file) or with an agent's credential.
    fn cli(&self, agent: Option<&str>, words: &[&str]) -> Output {
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_themis"));
        command
            .arg("--data-dir")
            .arg(&self.data)
            .args(words)
            .env_remove("OPENCODE_KEY")
            .env_remove(themis_core::agent_cli::TOKEN_ENV)
            .env_remove(themis_core::agent_cli::PORT_ENV);
        if let Some(token) = agent {
            command
                .env(themis_core::agent_cli::TOKEN_ENV, token)
                .env(themis_core::agent_cli::PORT_ENV, self.port().to_string());
        }
        command.output().expect("run CLI")
    }

    /// `themis call METHOD ARGS` as the person.
    fn call(&self, method: &str, args: Value) -> Output {
        self.cli(None, &["call", method, &args.to_string()])
    }

    fn call_ok(&self, method: &str, args: Value) -> Value {
        json_of(&self.call(method, args))
    }

    async fn thread(&self) -> String {
        self.state
            .create_thread(
                self.project.to_string_lossy().into_owned(),
                ProviderKind::Go,
                Some("test-model".to_owned()),
            )
            .await
            .unwrap()
            .id
    }

    /// A Vision named `name` and a channel in the project with it as the only member.
    fn vision_and_channel(&self, name: &str, channel: &str) -> (String, String) {
        let vision = self.call_ok(
            "save_vision",
            json!({"vision": {"name": name, "shape": "cloud", "color": "teal"}, "revision": null}),
        );
        let vision_id = vision["id"].as_str().unwrap().to_owned();
        let saved = self.call_ok(
            "save_channel",
            json!({"channel": {"name": channel, "project_root": self.project,
                "approval_mode": "custom", "members": [vision_id]}, "revision": null}),
        );
        (vision_id, saved["id"].as_str().unwrap().to_owned())
    }

    fn own(&self, thread: &str, vision: &str, channel: &str, kind: OwnedKind) {
        self.state
            .owner_set(Owner {
                thread_id: thread.to_owned(),
                vision_id: vision.to_owned(),
                channel_id: channel.to_owned(),
                kind,
                hold: Hold::None,
            })
            .unwrap();
    }

    fn listed_ids(&self) -> Vec<String> {
        let listed = self.call_ok("list_threads", json!({"projectRoot": self.project}));
        listed
            .as_array()
            .unwrap()
            .iter()
            .map(|thread| thread["id"].as_str().unwrap().to_owned())
            .collect()
    }

    fn entries(&self, channel: &str) -> Vec<Value> {
        let page = self.call_ok("get_channel_page", json!({"channelId": channel}));
        page["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|line| line["entry"].clone())
            .collect()
    }

    /// Points the chats at `provider` with a synthetic key.
    async fn use_provider(&self, provider: &MockServer) {
        self.state
            .set_secret("go".into(), "test-key".into())
            .await
            .unwrap();
        self.state.set_go_base_url_override(Some(provider.uri()));
    }

    /// Starts a run in `thread` that waits on the stalled provider.
    async fn start_stalled_run(&self, thread: &str, provider: &MockServer) {
        self.call_ok(
            "send_message",
            json!({"threadId": thread, "text": "work on it", "reasoningEffort": null}),
        );
        wait_until(|| async { !provider.received_requests().await.unwrap().is_empty() }).await;
    }

    async fn running(&self, thread: &str) -> bool {
        self.state
            .get_thread(thread)
            .await
            .is_ok_and(|info| info.running)
    }
}

async fn wait_until<F, Fut>(mut check: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    tokio::time::timeout(Duration::from_secs(10), async {
        while !check().await {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("condition not reached in time");
}

/// A provider that answers only after a long wait, so a run stays running.
async fn stalled_provider() -> MockServer {
    let provider = MockServer::builder().start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(Duration::from_secs(30))
                .set_body_json(json!({"choices":[{"message":{"role":"assistant","content":"late"},"finish_reason":"stop"}]})),
        )
        .mount(&provider)
        .await;
    provider
}

/// A provider that answers at once.
async fn quick_provider() -> MockServer {
    let provider = MockServer::builder().start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"choices":[{"message":{"role":"assistant","content":"on it"},"finish_reason":"stop"}]}),
        ))
        .mount(&provider)
        .await;
    provider
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn json_of(output: &Output) -> Value {
    assert!(output.status.success(), "{}", text(output));
    serde_json::from_slice(&output.stdout).unwrap_or_else(|_| panic!("{}", text(output)))
}

fn failed_with(output: &Output, words: &str) -> bool {
    !output.status.success() && text(output).contains(words)
}

fn owner_hold(state: &AppState, thread: &str) -> Option<Hold> {
    state.owner_of(thread).map(|owner| owner.hold)
}

/// Sets the hold the way a person's Stop leaves it, without a run to stop.
fn hold(state: &AppState, thread: &str, hold: Hold) {
    let owner = state.owner_of(thread).expect("owned");
    state.owner_set(Owner { hold, ..owner }).unwrap();
}

fn runtime_file(data: &Path) -> String {
    std::fs::read_to_string(data.join("runtime.jsonc")).unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_channel_and_a_vision_round_trip_with_revisions() {
    let _turn = ONE_SERVER.lock().await;
    let fx = fixture().await;
    let mut events = Client::new(fx.data.clone()).subscribe().await.unwrap();

    let vision = fx.call_ok(
        "save_vision",
        json!({"vision": {"name": "Wren", "role": "Release captain", "shape": "cloud",
            "color": "teal"}, "revision": null}),
    );
    let vision_id = vision["id"].as_str().unwrap().to_owned();
    let first = vision["revision"].as_str().unwrap().to_owned();
    let changed = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let event = Client::next_event(&mut events).await.unwrap();
            if event["name"] == "visions-changed" {
                break event;
            }
        }
    })
    .await
    .expect("a save announces visions-changed");
    assert_eq!(changed["payload"]["vision_id"], vision_id.as_str());

    let renamed = json!({"id": vision_id, "name": "Wren B", "shape": "cloud", "color": "teal"});
    let saved = fx.call_ok("save_vision", json!({"vision": renamed, "revision": first}));
    assert_ne!(saved["revision"], first.as_str());
    let stale = fx.call("save_vision", json!({"vision": renamed, "revision": first}));
    assert!(
        failed_with(&stale, "changed since it was opened"),
        "{}",
        text(&stale)
    );
    let listed = fx.call_ok("list_visions", json!({}));
    assert_eq!(listed[0]["name"], "Wren B");

    let channel = fx.call_ok(
        "save_channel",
        json!({"channel": {"name": "hq", "project_root": fx.project, "approval_mode": "custom",
            "members": [vision_id]}, "revision": null}),
    );
    let channel_id = channel["id"].as_str().unwrap().to_owned();
    let topic = json!({"id": channel_id, "name": "hq", "topic": "Ship 2.0",
        "project_root": fx.project, "approval_mode": "custom", "members": [vision_id]});
    fx.call_ok(
        "save_channel",
        json!({"channel": topic, "revision": channel["revision"]}),
    );
    let stale = fx.call(
        "save_channel",
        json!({"channel": topic, "revision": channel["revision"]}),
    );
    assert!(
        failed_with(&stale, "changed since it was opened"),
        "{}",
        text(&stale)
    );
    let channels = fx.call_ok("list_channels", json!({"projectRoot": fx.project}));
    assert_eq!(channels.as_array().unwrap().len(), 1);
    assert_eq!(channels[0]["topic"], "Ship 2.0");

    let posted = fx.call_ok(
        "post_channel_message",
        json!({"channelId": channel_id, "text": "@wren b what is left?"}),
    );
    assert_eq!(posted["seq"], 1);
    let empty = fx.call(
        "post_channel_message",
        json!({"channelId": channel_id, "text": "  "}),
    );
    assert!(!empty.status.success());
    let paused = fx.call_ok(
        "pause_vision",
        json!({"visionId": vision_id, "paused": true}),
    );
    assert_eq!(paused["paused"], true);
    let entries = fx.entries(&channel_id);
    assert_eq!(
        entries[0],
        json!({"type": "person", "text": "@wren b what is left?"})
    );
    assert_eq!(entries[1]["type"], "notice");
    assert_eq!(entries[1]["kind"], "paused");
    let page = fx.call_ok(
        "get_channel_page",
        json!({"channelId": channel_id, "beforeSeq": 2, "maxBytes": 10}),
    );
    assert_eq!(page["entries"].as_array().unwrap().len(), 1);
    assert_eq!(page["entries"][0]["seq"], 1);

    // Deleting the Vision takes it out of the channel; the channel and its log stay.
    fx.call_ok("delete_vision", json!({"visionId": vision_id}));
    assert_eq!(fx.call_ok("list_visions", json!({})), json!([]));
    let channels = fx.call_ok("list_channels", json!({"projectRoot": fx.project}));
    assert_eq!(channels[0]["members"], json!([]));
    assert_eq!(fx.entries(&channel_id).len(), 2);
    fx.shut_down().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn owned_chats_leave_the_plain_list_and_show_in_the_channel() {
    let _turn = ONE_SERVER.lock().await;
    let fx = fixture().await;
    let (vision, channel) = fx.vision_and_channel("Wren", "hq");
    let plain = fx.thread().await;
    let work = fx.thread().await;
    let own_chat = fx.thread().await;
    fx.own(&work, &vision, &channel, OwnedKind::Work);
    fx.own(&own_chat, &vision, &channel, OwnedKind::VisionChat);

    assert_eq!(fx.listed_ids(), vec![plain.clone()]);
    let in_channel = fx.call_ok("list_channel_threads", json!({"channelId": channel}));
    let in_channel = in_channel.as_array().unwrap();
    assert_eq!(in_channel.len(), 1, "{in_channel:?}");
    assert_eq!(in_channel[0]["id"], work.as_str());
    assert_eq!(in_channel[0]["owner"]["vision_id"], vision.as_str());
    assert_eq!(in_channel[0]["owner"]["hold"], "none");

    // Discarding an owned chat forgets its owner.
    fx.call_ok("discard_thread", json!({"threadId": work}));
    assert!(fx.state.owner_of(&work).is_none());
    let in_channel = fx.call_ok("list_channel_threads", json!({"channelId": channel}));
    assert_eq!(in_channel, json!([]));
    fx.shut_down().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn deleting_a_channel_stops_and_releases_its_chats() {
    let _turn = ONE_SERVER.lock().await;
    let fx = fixture().await;
    let provider = stalled_provider().await;
    fx.use_provider(&provider).await;
    let (vision, channel) = fx.vision_and_channel("Wren", "hq");
    fx.call_ok(
        "post_channel_message",
        json!({"channelId": channel, "text": "start the release"}),
    );
    let running = fx.thread().await;
    let idle = fx.thread().await;
    fx.start_stalled_run(&running, &provider).await;
    fx.own(&running, &vision, &channel, OwnedKind::Work);
    fx.own(&idle, &vision, &channel, OwnedKind::Work);
    assert!(fx.listed_ids().is_empty());

    fx.call_ok("delete_channel", json!({"channelId": channel}));
    wait_until(|| async { !fx.running(&running).await }).await;
    assert!(fx.state.owner_of(&running).is_none());
    assert!(fx.state.owner_of(&idle).is_none());
    let mut listed = fx.listed_ids();
    listed.sort();
    let mut expected = vec![running.clone(), idle.clone()];
    expected.sort();
    assert_eq!(listed, expected, "both chats are ordinary chats again");
    assert_eq!(
        fx.call_ok("list_channels", json!({"projectRoot": fx.project})),
        json!([])
    );
    // Nothing deleted: the chats, the Vision and the channel's log stay.
    assert!(fx.state.get_thread(&running).await.is_ok());
    assert_eq!(
        fx.call_ok("list_visions", json!({}))
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let log = fx
        .data
        .join("visions/channels")
        .join(format!("{channel}.jsonl"));
    assert!(std::fs::read_to_string(log)
        .unwrap()
        .contains("start the release"));
    let gone = fx.call("delete_channel", json!({"channelId": channel}));
    assert!(failed_with(&gone, "Unknown channel"), "{}", text(&gone));
    fx.shut_down().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_person_stop_marks_the_chat_held() {
    let _turn = ONE_SERVER.lock().await;
    let fx = fixture().await;
    let provider = stalled_provider().await;
    fx.use_provider(&provider).await;
    let (vision, channel) = fx.vision_and_channel("Wren", "hq");
    let work = fx.thread().await;
    fx.start_stalled_run(&work, &provider).await;
    fx.own(&work, &vision, &channel, OwnedKind::Work);

    let stopped = fx.cli(None, &["thread", "stop", &work]);
    assert!(stopped.status.success(), "{}", text(&stopped));
    assert_eq!(owner_hold(&fx.state, &work), Some(Hold::StoppedByPerson));
    let entries = fx.entries(&channel);
    assert_eq!(
        entries.last().unwrap(),
        &json!({"type": "notice", "kind": "stopped", "vision_id": vision, "thread_id": work})
    );
    wait_until(|| async { !fx.running(&work).await }).await;

    // The person writing in the channel lets the Vision pick it up again.
    fx.call_ok(
        "post_channel_message",
        json!({"channelId": channel, "text": "carry on"}),
    );
    assert_eq!(owner_hold(&fx.state, &work), Some(Hold::None));

    // Stop again, then hand back: the hold clears and the channel says so.
    hold(&fx.state, &work, Hold::StoppedByPerson);
    let handed = fx.call_ok("hand_back_thread", json!({"threadId": work}));
    assert_eq!(handed["hold"], "none");
    assert_eq!(owner_hold(&fx.state, &work), Some(Hold::None));
    assert_eq!(fx.entries(&channel).last().unwrap()["kind"], "handed_back");
    let plain = fx.thread().await;
    let refused = fx.call("hand_back_thread", json!({"threadId": plain}));
    assert!(
        failed_with(&refused, "does not belong to a Vision"),
        "{}",
        text(&refused)
    );
    fx.shut_down().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_person_message_in_an_owned_chat_is_a_take_over() {
    let _turn = ONE_SERVER.lock().await;
    let fx = fixture().await;
    let provider = quick_provider().await;
    fx.use_provider(&provider).await;
    let (vision, channel) = fx.vision_and_channel("Wren", "hq");
    let work = fx.thread().await;
    fx.own(&work, &vision, &channel, OwnedKind::Work);
    hold(&fx.state, &work, Hold::StoppedByPerson);

    fx.call_ok(
        "send_message",
        json!({"threadId": work, "text": "I'll take it from here", "reasoningEffort": null}),
    );
    assert_eq!(owner_hold(&fx.state, &work), Some(Hold::TakenOver));
    wait_until(|| async { !fx.running(&work).await }).await;

    // Writing in the channel does not undo a take over; only Hand back does.
    fx.call_ok(
        "post_channel_message",
        json!({"channelId": channel, "text": "status?"}),
    );
    assert_eq!(owner_hold(&fx.state, &work), Some(Hold::TakenOver));
    fx.call_ok("hand_back_thread", json!({"threadId": work}));
    assert_eq!(owner_hold(&fx.state, &work), Some(Hold::None));
    fx.shut_down().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn visions_settings_write_one_field_and_refuse_a_stale_revision() {
    let _turn = ONE_SERVER.lock().await;
    let fx = fixture().await;
    let first = fx.call_ok("get_visions_settings", json!({}));
    assert_eq!(first["settings"]["autonomy"], "small");
    assert_eq!(first["settings"]["max_wakes"], 6);
    let revision = first["revision"].clone();

    let updated = fx.call_ok(
        "update_visions_settings",
        json!({"changes": {"max_wakes": 3}, "revision": revision}),
    );
    assert_eq!(updated["settings"]["max_wakes"], 3);
    assert_eq!(updated["settings"]["autonomy"], "small");
    assert_ne!(updated["revision"], revision);
    let file = runtime_file(&fx.data);
    assert!(file.contains("\"visions\""), "{file}");
    // Written in place: the file's comments and commented-out examples stay.
    assert!(
        file.contains("// \"browser\": { \"enabled\": false }"),
        "{file}"
    );
    assert!(file.contains("\"max_wakes\": 3"), "{file}");

    let stale = fx.call(
        "update_visions_settings",
        json!({"changes": {"autonomy": "most"}, "revision": revision}),
    );
    assert!(
        failed_with(&stale, "since this page loaded"),
        "{}",
        text(&stale)
    );
    let other = fx.call(
        "update_visions_settings",
        json!({"changes": {"model": "x"}, "revision": updated["revision"]}),
    );
    assert!(
        failed_with(&other, "cannot change `model`"),
        "{}",
        text(&other)
    );
    let invalid = fx.call(
        "update_visions_settings",
        json!({"changes": {"max_wakes": 99}, "revision": updated["revision"]}),
    );
    assert!(failed_with(&invalid, "max_wakes"), "{}", text(&invalid));
    let latest = fx.call_ok("get_visions_settings", json!({}));
    assert_eq!(latest["settings"]["max_wakes"], 3);
    assert_eq!(latest["revision"], updated["revision"]);
    fx.shut_down().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_credential_is_refused_every_visions_method() {
    let _turn = ONE_SERVER.lock().await;
    let fx = fixture().await;
    let (_, channel) = fx.vision_and_channel("Wren", "hq");
    let thread = fx.thread().await;
    let scope = themis_core::agent_cli::RunScope::new(&fx.project, None);
    let access = themis_core::agent_cli::issue(&thread, "run-1", &scope).expect("issued");
    let agent = access.env()[0].1.clone();
    for method in METHODS {
        let args = json!({"threadId": thread, "channelId": channel, "projectRoot": fx.project,
            "text": "hello", "changes": {"max_wakes": 2}});
        let output = fx.cli(Some(&agent), &["call", method, &args.to_string()]);
        assert!(
            failed_with(&output, &format!("Themis refused `{method}`")),
            "{method}: {}",
            text(&output)
        );
    }
    // Nothing the refused calls asked for happened.
    assert_eq!(fx.entries(&channel), Vec::<Value>::new());
    assert!(!runtime_file(&fx.data).contains("\"visions\""));
    fx.shut_down().await;
}
