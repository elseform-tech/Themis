//! Local, authenticated RPC over loopback TCP. The server is the only owner of
//! live application state; Tauri and the CLI use the same command dispatcher.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast;

use crate::sink::EventSink;
use crate::state::AppState;
use crate::types::{ApprovalRequest, ReviewItem, ThreadEventEnvelope};

const MAX_FRAME: usize = 8 * 1024 * 1024;
const DISCOVERY_FILE: &str = "server.json";

#[derive(Serialize, Deserialize)]
struct Discovery {
    port: u16,
    token: String,
}

#[derive(Serialize, Deserialize)]
struct Request {
    token: String,
    method: String,
    #[serde(default)]
    args: Value,
}

#[derive(Serialize, Deserialize)]
struct Response {
    ok: bool,
    value: Value,
    error: Option<String>,
}

/// A client reads fresh discovery data for every connection, so restarting a
/// server does not require restarting the desktop app or CLI.
#[derive(Clone)]
pub struct Client {
    data_dir: PathBuf,
}

impl Client {
    pub fn new(data_dir: PathBuf) -> Self {
        Self { data_dir }
    }

    pub async fn call(&self, method: &str, args: Value) -> Result<Value, String> {
        let (mut stream, discovery) = self.connect().await?;
        write_frame(
            &mut stream,
            &Request {
                token: discovery.token,
                method: method.to_owned(),
                args,
            },
        )
        .await?;
        let reply: Response = read_frame(&mut stream).await?;
        if reply.ok {
            Ok(reply.value)
        } else {
            Err(reply
                .error
                .unwrap_or_else(|| "server request failed".to_owned()))
        }
    }

    pub async fn subscribe(&self) -> Result<TcpStream, String> {
        let (mut stream, discovery) = self.connect().await?;
        write_frame(
            &mut stream,
            &Request {
                token: discovery.token,
                method: "subscribe".to_owned(),
                args: Value::Null,
            },
        )
        .await?;
        let reply: Response = read_frame(&mut stream).await?;
        if reply.ok {
            Ok(stream)
        } else {
            Err(reply
                .error
                .unwrap_or_else(|| "subscription failed".to_owned()))
        }
    }

    pub async fn next_event(stream: &mut TcpStream) -> Result<Value, String> {
        read_frame(stream).await
    }

    fn discovery(&self) -> Result<Discovery, String> {
        let text = std::fs::read_to_string(self.data_dir.join(DISCOVERY_FILE))
            .map_err(|err| format!("local server is unavailable: {err}"))?;
        serde_json::from_str(&text).map_err(|err| format!("invalid server discovery file: {err}"))
    }

    async fn connect(&self) -> Result<(TcpStream, Discovery), String> {
        let discovery = self.discovery()?;
        let stream = TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, discovery.port))
            .await
            .map_err(|err| format!("cannot connect to local server: {err}"))?;
        Ok((stream, discovery))
    }
}

struct BroadcastSink {
    tx: broadcast::Sender<Value>,
}

impl EventSink for BroadcastSink {
    fn emit_thread_event(&self, envelope: &ThreadEventEnvelope) {
        let _ = self
            .tx
            .send(json!({"name": "thread-event", "payload": envelope}));
    }

    fn emit_approval_request(&self, request: &ApprovalRequest) {
        let _ = self
            .tx
            .send(json!({"name": "approval-request", "payload": request}));
    }

    fn emit_review_item(&self, item: &ReviewItem) {
        let _ = self
            .tx
            .send(json!({"name": "review-item-added", "payload": item}));
    }
}

pub struct Server {
    listener: TcpListener,
    state: AppState,
    token: String,
    tx: broadcast::Sender<Value>,
    sink: Arc<dyn EventSink>,
    shutdown: broadcast::Sender<()>,
    data_dir: PathBuf,
}

impl Server {
    pub async fn bind(state: AppState, data_dir: &Path) -> Result<Self, String> {
        std::fs::create_dir_all(data_dir).map_err(|err| err.to_string())?;
        let client = Client::new(data_dir.to_path_buf());
        if tokio::time::timeout(
            std::time::Duration::from_millis(500),
            client.call("ping", Value::Null),
        )
        .await
        .is_ok_and(|result| result.is_ok())
        {
            return Err("a local Themis server already owns this data directory".to_owned());
        }
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .map_err(|err| format!("cannot bind local server: {err}"))?;
        let token = format!("{}{}", uuid::Uuid::new_v4(), uuid::Uuid::new_v4());
        let discovery = Discovery {
            port: listener.local_addr().map_err(|err| err.to_string())?.port(),
            token: token.clone(),
        };
        write_discovery(data_dir, &discovery)?;
        let (tx, _) = broadcast::channel(1024);
        let (shutdown, _) = broadcast::channel(1);
        let sink: Arc<dyn EventSink> = Arc::new(BroadcastSink { tx: tx.clone() });
        state.set_scheduler_sink(Arc::clone(&sink));
        Ok(Self {
            listener,
            state,
            token,
            tx,
            sink,
            shutdown,
            data_dir: data_dir.to_path_buf(),
        })
    }

    pub async fn run(self) -> Result<(), String> {
        let mut stopped = self.shutdown.subscribe();
        loop {
            let (stream, _) = tokio::select! {
                result = self.listener.accept() => result.map_err(|err| err.to_string())?,
                _ = stopped.recv() => break,
            };
            let state = self.state.clone();
            let token = self.token.clone();
            let tx = self.tx.clone();
            let sink = Arc::clone(&self.sink);
            let shutdown = self.shutdown.clone();
            tokio::spawn(async move {
                let _ = serve_connection(stream, state, token, tx, sink, shutdown).await;
            });
        }
        let path = self.data_dir.join(DISCOVERY_FILE);
        if std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<Discovery>(&text).ok())
            .is_some_and(|discovery| discovery.token == self.token)
        {
            std::fs::remove_file(path).map_err(|err| err.to_string())?;
        }
        Ok(())
    }
}

async fn serve_connection(
    mut stream: TcpStream,
    state: AppState,
    token: String,
    tx: broadcast::Sender<Value>,
    sink: Arc<dyn EventSink>,
    shutdown: broadcast::Sender<()>,
) -> Result<(), String> {
    let request: Request = read_frame(&mut stream).await?;
    if request.token != token {
        return write_frame(
            &mut stream,
            &Response {
                ok: false,
                value: Value::Null,
                error: Some("unauthorized".to_owned()),
            },
        )
        .await;
    }
    if request.method == "shutdown" {
        write_frame(
            &mut stream,
            &Response {
                ok: true,
                value: Value::Null,
                error: None,
            },
        )
        .await?;
        let _ = shutdown.send(());
        return Ok(());
    }
    if request.method == "subscribe" {
        let mut rx = tx.subscribe();
        write_frame(
            &mut stream,
            &Response {
                ok: true,
                value: Value::Null,
                error: None,
            },
        )
        .await?;
        loop {
            match rx.recv().await {
                Ok(event) => write_frame(&mut stream, &event).await?,
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    write_frame(&mut stream, &json!({"name": "lagged", "payload": null})).await?;
                }
                Err(broadcast::error::RecvError::Closed) => return Ok(()),
            }
        }
    }
    let reply = match dispatch(&state, sink, &request.method, request.args).await {
        Ok(value) => Response {
            ok: true,
            value,
            error: None,
        },
        Err(error) => {
            state.record_error(&request.method, error.clone());
            Response {
                ok: false,
                value: Value::Null,
                error: Some(error),
            }
        }
    };
    write_frame(&mut stream, &reply).await
}

async fn write_frame<T: Serialize>(stream: &mut TcpStream, value: &T) -> Result<(), String> {
    let bytes = serde_json::to_vec(value).map_err(|err| err.to_string())?;
    let len = u32::try_from(bytes.len()).map_err(|err| err.to_string())?;
    if bytes.len() > MAX_FRAME {
        return Err("server frame exceeds size limit".to_owned());
    }
    stream.write_u32(len).await.map_err(|err| err.to_string())?;
    stream
        .write_all(&bytes)
        .await
        .map_err(|err| err.to_string())
}

async fn read_frame<T: DeserializeOwned>(stream: &mut TcpStream) -> Result<T, String> {
    let len = stream.read_u32().await.map_err(|err| err.to_string())? as usize;
    if len > MAX_FRAME {
        return Err("server frame exceeds size limit".to_owned());
    }
    let mut bytes = vec![0; len];
    stream
        .read_exact(&mut bytes)
        .await
        .map_err(|err| err.to_string())?;
    serde_json::from_slice(&bytes).map_err(|err| err.to_string())
}

fn write_discovery(data_dir: &Path, discovery: &Discovery) -> Result<(), String> {
    let path = data_dir.join(DISCOVERY_FILE);
    let temporary = data_dir.join(format!(".server-{}.tmp", uuid::Uuid::new_v4()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary).map_err(|err| err.to_string())?;
    use std::io::Write;
    file.write_all(&serde_json::to_vec(discovery).map_err(|err| err.to_string())?)
        .map_err(|err| err.to_string())?;
    #[cfg(target_os = "windows")]
    if path.exists() {
        std::fs::remove_file(&path).map_err(|err| err.to_string())?;
    }
    std::fs::rename(temporary, path).map_err(|err| err.to_string())
}

fn arg<T: DeserializeOwned>(args: &Value, name: &str) -> Result<T, String> {
    serde_json::from_value(args.get(name).cloned().unwrap_or(Value::Null))
        .map_err(|err| format!("invalid {name}: {err}"))
}

/// The command names and camelCase arguments match the existing Tauri bridge.
pub async fn dispatch(
    state: &AppState,
    sink: Arc<dyn EventSink>,
    method: &str,
    args: Value,
) -> Result<Value, String> {
    use crate::types::{
        ApprovalDecision, AutomationInput, ProviderKind, ReviewStatus, SettingsPatch, SkillInput,
    };
    macro_rules! output {
        ($expr:expr) => {
            serde_json::to_value($expr).map_err(|err| err.to_string())
        };
    }
    macro_rules! done {
        ($expr:expr) => {{
            $expr?;
            Ok(Value::Null)
        }};
    }
    match method {
        "plugin_action" => output!(state.plugin_action(args).await?),
        "list_prompt_skills" => output!(state.prompt_skills(arg(&args, "projectRoot")?).await?),
        "ping" => output!(state.ping().await),
        "open_project" => output!(state.open_project(arg(&args, "path")?).await?),
        "get_default_project" => output!(state.get_default_project().await?),
        "create_project" => output!(
            state
                .create_project(arg(&args, "name")?, arg(&args, "directory")?)
                .await?
        ),
        "rename_thread" => output!(
            state
                .rename_thread(arg(&args, "threadId")?, arg(&args, "title")?)
                .await?
        ),
        "stop_thread" => done!(state.stop_thread(arg(&args, "threadId")?).await),
        "list_go_models" => output!(state.list_go_models().await?),
        "create_thread" => output!(
            state
                .create_thread(
                    arg(&args, "projectRoot")?,
                    arg::<ProviderKind>(&args, "provider")?,
                    arg(&args, "model")?
                )
                .await?
        ),
        "get_thread" => output!(state.get_thread(&arg::<String>(&args, "threadId")?).await?),
        "get_thread_history" => output!(
            state
                .get_thread_history(&arg::<String>(&args, "threadId")?)
                .await?
        ),
        "import_legacy_history" => done!(
            state
                .import_legacy_history(
                    &arg::<String>(&args, "threadId")?,
                    &arg::<Vec<Value>>(&args, "messages")?
                )
                .await
        ),
        "list_threads" => output!(state.list_threads(arg(&args, "projectRoot")?).await?),
        "merge_thread" => output!(state.merge_thread(arg(&args, "threadId")?).await?),
        "discard_thread" => done!(state.discard_thread(arg(&args, "threadId")?).await),
        "send_message" => output!(
            state
                .send_message_with_effort(
                    sink,
                    arg(&args, "threadId")?,
                    arg(&args, "text")?,
                    arg(&args, "reasoningEffort")?
                )
                .await?
        ),
        "list_diff" => output!(state.list_diff(arg(&args, "threadId")?).await?),
        "accept_file" => done!(
            state
                .accept_file(arg(&args, "threadId")?, arg(&args, "path")?)
                .await
        ),
        "discard_file" => done!(
            state
                .discard_file(arg(&args, "threadId")?, arg(&args, "path")?)
                .await
        ),
        "add_comment" => output!(
            state
                .add_comment(
                    arg(&args, "threadId")?,
                    arg(&args, "path")?,
                    arg(&args, "comment")?
                )
                .await?
        ),
        "approve_action" => done!(
            state
                .approve_action(
                    arg(&args, "threadId")?,
                    arg(&args, "approvalId")?,
                    arg::<ApprovalDecision>(&args, "decision")?
                )
                .await
        ),
        "set_provider" => output!(
            state
                .set_provider(
                    arg(&args, "threadId")?,
                    arg::<ProviderKind>(&args, "provider")?,
                    arg(&args, "model")?
                )
                .await?
        ),
        "set_thread_effort" => output!(
            state
                .set_thread_effort(arg(&args, "threadId")?, arg(&args, "effort")?)
                .await?
        ),
        "open_in_editor" => done!(
            state
                .open_in_editor(arg(&args, "path")?, arg(&args, "line")?)
                .await
        ),
        "get_settings" => output!(state.get_settings().await),
        "update_settings" => output!(
            state
                .update_settings(arg::<SettingsPatch>(&args, "patch")?)
                .await?
        ),
        "get_secret_status" => output!(state.get_secret_status().await),
        "set_secret" => done!(
            state
                .set_secret(arg(&args, "provider")?, arg(&args, "value")?)
                .await
        ),
        "clear_secret" => done!(state.clear_secret(arg(&args, "provider")?).await),
        "list_skills" => output!(state.list_skills().await),
        "create_skill" => output!(
            state
                .create_skill(arg::<SkillInput>(&args, "input")?)
                .await?
        ),
        "update_skill" => output!(
            state
                .update_skill(arg(&args, "skillId")?, arg::<SkillInput>(&args, "input")?)
                .await?
        ),
        "delete_skill" => done!(state.delete_skill(arg(&args, "skillId")?).await),
        "set_thread_skills" => output!(
            state
                .set_thread_skills(arg(&args, "threadId")?, arg(&args, "skillIds")?)
                .await?
        ),
        "list_automations" => output!(state.list_automations().await),
        "create_automation" => output!(
            state
                .create_automation(arg::<AutomationInput>(&args, "input")?)
                .await?
        ),
        "update_automation" => output!(
            state
                .update_automation(
                    arg(&args, "automationId")?,
                    arg::<AutomationInput>(&args, "input")?
                )
                .await?
        ),
        "delete_automation" => done!(state.delete_automation(arg(&args, "automationId")?).await),
        "set_automation_enabled" => output!(
            state
                .set_automation_enabled(arg(&args, "automationId")?, arg(&args, "enabled")?)
                .await?
        ),
        "run_automation_now" => output!(
            state
                .run_automation_now(sink, arg(&args, "automationId")?)
                .await?
        ),
        "list_review_items" => output!(
            state
                .list_review_items(arg::<Option<ReviewStatus>>(&args, "status")?)
                .await
        ),
        "dismiss_review_item" => output!(state.dismiss_review_item(arg(&args, "reviewId")?).await?),
        "continue_review_item" => {
            output!(state.continue_review_item(arg(&args, "reviewId")?).await?)
        }
        "get_diagnostics" => output!(
            state
                .get_diagnostics_with_depth(
                    args.get("deep").and_then(Value::as_bool).unwrap_or(false)
                )
                .await
        ),
        _ => Err(format!("unknown server method '{method}'")),
    }
}
