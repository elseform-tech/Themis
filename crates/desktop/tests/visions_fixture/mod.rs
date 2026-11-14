//! The shared fixture of the Visions CLI end-to-end tests (`visions_cli_e2e.rs`): an
//! in-process app server on isolated app data, the person's and a Vision run's `themis`, and
//! scripted providers.

use std::path::{Path, PathBuf};
use std::process::Output;
use std::time::Duration;

use serde_json::{json, Value};
use themis_desktop::server::Server;
use themis_desktop::state::{AppState, Hold, OwnedKind, Owner};
use themis_desktop::types::ProviderKind;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

pub struct Fixture {
    _dir: tempfile::TempDir,
    _repo: tempfile::TempDir,
    pub data: PathBuf,
    pub project: PathBuf,
    pub state: AppState,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    served: Option<tokio::task::JoinHandle<Result<(), String>>>,
}

pub async fn fixture() -> Fixture {
    let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let data = dir.path().canonicalize().unwrap().join("app-data");
    std::fs::create_dir_all(&data).unwrap();
    let (repo, project) = crate::common::init_repo();
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
    pub async fn shut_down(mut self) {
        let _ = self.stop.take().unwrap().send(());
        self.served.take().unwrap().await.unwrap().unwrap();
    }

    pub fn port(&self) -> u16 {
        let text = std::fs::read_to_string(self.data.join("server.json")).unwrap();
        serde_json::from_str::<Value>(&text).unwrap()["port"]
            .as_u64()
            .unwrap() as u16
    }

    /// `themis` as the person (discovery file) or with an agent's credential.
    pub fn cli(&self, agent: Option<&str>, words: &[&str]) -> Output {
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
    pub fn call(&self, method: &str, args: Value) -> Output {
        self.cli(None, &["call", method, &args.to_string()])
    }

    pub fn call_ok(&self, method: &str, args: Value) -> Value {
        json_of(&self.call(method, args))
    }

    pub async fn thread(&self) -> String {
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
    pub fn vision_and_channel(&self, name: &str, channel: &str) -> (String, String) {
        self.vision_and_channel_in(name, channel, "custom")
    }

    /// The same, with the channel's approval mode `mode`.
    /// The Vision runs its chats on the scripted provider's `test-model`.
    pub fn vision_and_channel_in(&self, name: &str, channel: &str, mode: &str) -> (String, String) {
        let vision = self.call_ok(
            "save_vision",
            json!({"vision": {"name": name, "shape": "cloud", "color": "teal",
                "model": "test-model"}, "revision": null}),
        );
        let vision_id = vision["id"].as_str().unwrap().to_owned();
        let saved = self.call_ok(
            "save_channel",
            json!({"channel": {"name": channel, "project_root": self.project,
                "approval_mode": mode, "members": [vision_id]}, "revision": null}),
        );
        (vision_id, saved["id"].as_str().unwrap().to_owned())
    }

    /// Visions switched off in the person's runtime.jsonc (kept with what it holds): no real
    /// Vision run wakes, for a test that plays the Vision's run itself or checks only the store.
    pub fn no_wakes(&self) {
        let path = self.data.join("runtime.jsonc");
        let mut config: Value = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_else(|| json!({}));
        config["visions"]["enabled"] = json!(false);
        std::fs::write(path, config.to_string()).unwrap();
    }

    /// The person's `visions` block in runtime.jsonc, for the runs that start from now on.
    pub fn set_visions(&self, visions: Value) {
        let config = json!({ "visions": visions });
        std::fs::write(self.data.join("runtime.jsonc"), config.to_string()).unwrap();
    }

    /// The Vision's own chat in the channel and a credential for its run there, as a Vision's
    /// run gets one. The test plays that run: no real one wakes in the chat (it would end the
    /// credential with it).
    pub async fn vision_run(&self, vision: &str, channel: &str) -> (String, String) {
        self.no_wakes();
        let chat = self.thread().await;
        self.own(&chat, vision, channel, OwnedKind::VisionChat);
        let mut scope = themis_core::agent_cli::RunScope::new(&self.project, None);
        scope.vision = Some(self.state.visions_scope(vision, channel).unwrap());
        let access = themis_core::agent_cli::issue(&chat, "vision-run", &scope).expect("issued");
        (chat, access.env()[0].1.clone())
    }

    pub fn agent_call_ok(&self, token: &str, method: &str, args: Value) -> Value {
        json_of(&self.agent_call(token, method, args))
    }

    /// `themis call METHOD ARGS` with a run's credential.
    pub fn agent_call(&self, token: &str, method: &str, args: Value) -> Output {
        self.cli(Some(token), &["call", method, &args.to_string()])
    }

    /// A work chat the Vision starts through its credential.
    pub fn vision_starts_chat(&self, token: &str) -> String {
        let created = json_of(&self.agent_call(
            token,
            "create_thread",
            json!({"projectRoot": self.project, "provider": "go", "model": "chosen-by-call"}),
        ));
        created["id"].as_str().unwrap().to_owned()
    }

    /// A work chat the Vision starts and gives work that asks it one question ("Which
    /// license?"), and that question's id, once the chat is idle again.
    pub async fn asked_question(&self, token: &str) -> (String, String) {
        let work = self.vision_starts_chat(token);
        let ask = json!({"threadId": work, "text": "please ask me first"});
        json_of(&self.agent_call(token, "send_message", ask));
        let asked = || self.call_ok("list_questions", json!({"threadId": work}));
        wait_until(|| async { asked()[0]["id"].is_string() && !self.running(&work).await }).await;
        let id = asked()[0]["id"].as_str().unwrap().to_owned();
        (work, id)
    }

    pub fn own(&self, thread: &str, vision: &str, channel: &str, kind: OwnedKind) {
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

    pub fn listed_ids(&self) -> Vec<String> {
        let listed = self.call_ok("list_threads", json!({"projectRoot": self.project}));
        listed
            .as_array()
            .unwrap()
            .iter()
            .map(|thread| thread["id"].as_str().unwrap().to_owned())
            .collect()
    }

    /// The texts the chat's history shows steered in, each with the Vision that wrote it.
    pub fn steered(&self, thread: &str) -> Vec<(String, Value)> {
        let history = self.call_ok("get_thread_history", json!({"threadId": thread}));
        history
            .as_array()
            .unwrap()
            .iter()
            .map(|item| &item["envelope"]["event"])
            .filter(|event| event["kind"] == "steered")
            .map(|event| {
                let text = event["text"].as_str().unwrap().to_owned();
                (text, event.get("vision_id").cloned().unwrap_or(Value::Null))
            })
            .collect()
    }

    pub fn entries(&self, channel: &str) -> Vec<Value> {
        let page = self.call_ok("get_channel_page", json!({"channelId": channel}));
        page["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|line| line["entry"].clone())
            .collect()
    }

    /// Points the chats at `provider` with a synthetic key, its `test-model` listed as free
    /// (Visions' budgets need a loaded price catalog).
    pub async fn use_provider(&self, provider: &MockServer) {
        self.state
            .set_secret("go".into(), "test-key".into())
            .await
            .unwrap();
        self.state.set_go_base_url_override(Some(provider.uri()));
        self.state
            .set_model_catalog_for_test(vec![themis_core::providers::GoModel {
                id: "test-model".into(),
                price: Some(themis_core::providers::ModelPrice::default()),
                ..Default::default()
            }])
            .await;
    }

    /// Starts a run in `thread` that waits on the stalled provider.
    pub async fn start_stalled_run(&self, thread: &str, provider: &MockServer) {
        let before = provider.received_requests().await.unwrap().len();
        self.call_ok(
            "send_message",
            json!({"threadId": thread, "text": "work on it", "reasoningEffort": null}),
        );
        wait_until(|| async { provider.received_requests().await.unwrap().len() > before }).await;
    }

    pub async fn running(&self, thread: &str) -> bool {
        self.state
            .get_thread(thread)
            .await
            .is_ok_and(|info| info.running)
    }
}

pub async fn wait_until<F, Fut>(mut check: F)
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
pub async fn stalled_provider() -> MockServer {
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
pub async fn quick_provider() -> MockServer {
    let provider = MockServer::builder().start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"choices":[{"message":{"role":"assistant","content":"on it"},"finish_reason":"stop"}]}),
        ))
        .mount(&provider)
        .await;
    provider
}

pub fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

pub fn json_of(output: &Output) -> Value {
    assert!(output.status.success(), "{}", text(output));
    serde_json::from_slice(&output.stdout).unwrap_or_else(|_| panic!("{}", text(output)))
}

pub fn failed_with(output: &Output, words: &str) -> bool {
    !output.status.success() && text(output).contains(words)
}

pub fn owner_hold(state: &AppState, thread: &str) -> Option<Hold> {
    state.owner_of(thread).map(|owner| owner.hold)
}

/// Sets the hold the way a person's Stop leaves it, without a run to stop.
pub fn hold(state: &AppState, thread: &str, hold: Hold) {
    let owner = state.owner_of(thread).expect("owned");
    state.owner_set(Owner { hold, ..owner }).unwrap();
}

pub fn runtime_file(data: &Path) -> String {
    std::fs::read_to_string(data.join("runtime.jsonc")).unwrap_or_default()
}
