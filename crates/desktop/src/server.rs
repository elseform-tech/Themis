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
use crate::transcript::history_pages::{HistoryCursor, HistoryPage, MAX_PAGE_BYTES};
use crate::types::{
    ApprovalRequest, ReviewItem, ThreadChange, ThreadChangeKind, ThreadEventEnvelope,
};

pub(crate) const MAX_FRAME: usize = 8 * 1024 * 1024;
const DISCOVERY_FILE: &str = "server.json";

#[cfg(test)]
mod history_transport_tests;

/// How long a server that still accepts connections gets to answer before it counts as gone.
pub const SERVER_PATIENCE: std::time::Duration = std::time::Duration::from_secs(5);

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
        if method == "get_thread_history" {
            return self.history(args).await;
        }
        self.call_raw(method, args).await
    }

    async fn history(&self, args: Value) -> Result<Value, String> {
        let thread_id = arg::<String>(&args, "threadId")?;
        let mut cursor = HistoryCursor::default();
        let mut through_seq = None;
        let mut pending = String::new();
        let mut items = Vec::new();
        loop {
            let page: HistoryPage = serde_json::from_value(self.call_raw(
                "get_thread_history_page",
                json!({"threadId":thread_id,"cursor":cursor,"throughSeq":through_seq,"maxBytes":MAX_PAGE_BYTES}),
            ).await?).map_err(|e| format!("Invalid history page: {e}"))?;
            if through_seq.is_some_and(|seq| seq != page.through_seq)
                || page.next_cursor.is_some_and(|next| next <= cursor)
            {
                return Err("History page watermark changed or cursor did not advance".into());
            }
            through_seq = Some(page.through_seq);
            pending.push_str(&page.jsonl);
            if let Some(end) = pending.rfind('\n') {
                for line in pending[..end].split('\n') {
                    items.push(
                        serde_json::from_str::<Value>(line)
                            .map_err(|e| format!("Invalid history entry: {e}"))?,
                    );
                }
                pending.drain(..=end);
            }
            match page.next_cursor {
                Some(next) => cursor = next,
                None if pending.is_empty() => return Ok(Value::Array(items)),
                None => return Err("History page ended with an incomplete entry".into()),
            }
        }
    }

    async fn call_raw(&self, method: &str, args: Value) -> Result<Value, String> {
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

    /// One chat's browser updates (state and screencast frames); the first
    /// value is the current state.
    pub async fn subscribe_browser(&self, thread_id: &str) -> Result<(TcpStream, Value), String> {
        let (mut stream, discovery) = self.connect().await?;
        write_frame(
            &mut stream,
            &Request {
                token: discovery.token,
                method: "subscribe_browser".to_owned(),
                args: json!({"threadId": thread_id}),
            },
        )
        .await?;
        let reply: Response = read_frame(&mut stream).await?;
        if reply.ok {
            Ok((stream, reply.value))
        } else {
            Err(reply
                .error
                .unwrap_or_else(|| "browser subscription failed".to_owned()))
        }
    }

    pub async fn next_event(stream: &mut TcpStream) -> Result<Value, String> {
        read_frame(stream).await
    }

    /// Whether a server owns this data directory. One whose port still accepts connections
    /// gets up to `patience` to answer a ping: a busy server is not a missing one, and
    /// starting a second would take its place.
    pub async fn answers(&self, patience: std::time::Duration) -> bool {
        let ping = || {
            tokio::time::timeout(
                std::time::Duration::from_millis(500),
                self.call("ping", Value::Null),
            )
        };
        let deadline = tokio::time::Instant::now() + patience;
        loop {
            if ping().await.is_ok_and(|result| result.is_ok()) {
                return true;
            }
            let listening = match self.discovery() {
                Ok(discovery) => {
                    TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, discovery.port))
                        .await
                        .is_ok()
                }
                Err(_) => false,
            };
            if !listening || tokio::time::Instant::now() >= deadline {
                return false;
            }
        }
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

    fn emit_approval_expired(&self, expired: &crate::types::ApprovalExpired) {
        let _ = self
            .tx
            .send(json!({"name": "approval-expired", "payload": expired}));
    }

    fn emit_review_item(&self, item: &ReviewItem) {
        let _ = self
            .tx
            .send(json!({"name": "review-item-added", "payload": item}));
    }

    fn emit_thread_changed(&self, change: &ThreadChange) {
        let _ = self
            .tx
            .send(json!({"name": "thread-changed", "payload": change}));
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
        if client.answers(SERVER_PATIENCE).await {
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
        self.run_until(std::future::pending()).await
    }

    /// Serves until a `shutdown` request, SIGTERM/SIGINT, or `stop` completes.
    pub async fn run_until(
        self,
        stop: impl std::future::Future<Output = ()>,
    ) -> Result<(), String> {
        self.state.log_server_event("started");
        // Samples Themis's own CPU and memory use for the title bar and the log.
        let sampler = self.state.clone();
        let mut sampler_stop = self.shutdown.subscribe();
        tokio::spawn(async move {
            let mut every = tokio::time::interval(crate::resources::SAMPLE_EVERY);
            loop {
                tokio::select! {
                    _ = every.tick() => {
                        let state = sampler.clone();
                        let _ = tokio::task::spawn_blocking(move || state.sample_resources()).await;
                    }
                    _ = sampler_stop.recv() => break,
                }
            }
        });
        let mut stopped = self.shutdown.subscribe();
        let mut termination = Termination::install();
        tokio::pin!(stop);
        loop {
            let (stream, _) = tokio::select! {
                result = self.listener.accept() => result.map_err(|err| err.to_string())?,
                _ = stopped.recv() => break,
                _ = termination.requested() => break,
                _ = &mut stop => break,
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
        // Background programs belong to this server: stop them with it.
        self.state.shutdown_background_tasks().await;
        let path = self.data_dir.join(DISCOVERY_FILE);
        if std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<Discovery>(&text).ok())
            .is_some_and(|discovery| discovery.token == self.token)
        {
            std::fs::remove_file(path).map_err(|err| err.to_string())?;
        }
        self.state.log_server_event("stopped");
        Ok(())
    }
}

/// SIGTERM/SIGINT listeners, created once before the accept loop so a signal
/// arriving between accepts or during cleanup is never dropped (the first
/// registration replaces the default action). Never fires where signals cannot
/// be observed.
struct Termination {
    #[cfg(unix)]
    streams: Option<(tokio::signal::unix::Signal, tokio::signal::unix::Signal)>,
}

impl Termination {
    fn install() -> Self {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{signal, SignalKind};
            let streams = match (
                signal(SignalKind::terminate()),
                signal(SignalKind::interrupt()),
            ) {
                (Ok(terminate), Ok(interrupt)) => Some((terminate, interrupt)),
                _ => None,
            };
            Self { streams }
        }
        #[cfg(not(unix))]
        Self {}
    }

    async fn requested(&mut self) {
        #[cfg(unix)]
        if let Some((terminate, interrupt)) = self.streams.as_mut() {
            tokio::select! {
                _ = terminate.recv() => {}
                _ = interrupt.recv() => {}
            }
            return;
        }
        std::future::pending::<()>().await;
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
                Ok(event) => {
                    let bytes = serde_json::to_vec(&event).map_err(|err| err.to_string())?;
                    if bytes.len() > MAX_FRAME {
                        state.record_error("subscribe", "server event exceeds size limit".into());
                        write_frame(&mut stream, &json!({"name": "lagged", "payload": null}))
                            .await?;
                    } else {
                        write_bytes(&mut stream, &bytes).await?;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    write_frame(&mut stream, &json!({"name": "lagged", "payload": null})).await?;
                }
                Err(broadcast::error::RecvError::Closed) => return Ok(()),
            }
        }
    }
    if request.method == "subscribe_browser" {
        // One chat's browser updates on their own connection, so screencast
        // frames never crowd the app's event stream. Frames a slow reader
        // misses are skipped; the next one replaces them.
        let thread_id = arg::<String>(&request.args, "threadId")?;
        let mut rx = match state.browser_updates(&thread_id).await {
            Ok(rx) => rx,
            Err(error) => {
                return write_frame(
                    &mut stream,
                    &Response {
                        ok: false,
                        value: Value::Null,
                        error: Some(error),
                    },
                )
                .await;
            }
        };
        write_frame(
            &mut stream,
            &Response {
                ok: true,
                value: state.browser_state(&thread_id).await.unwrap_or(Value::Null),
                error: None,
            },
        )
        .await?;
        loop {
            let update = match rx.recv().await {
                Ok(update) => update,
                // Missed frames do not matter, but a missed state change
                // would stick ("Agent is clicking" after the run): resend it.
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    match state.browser_state_update(&thread_id).await {
                        Some(update) => update,
                        None => continue,
                    }
                }
                Err(broadcast::error::RecvError::Closed) => return Ok(()),
            };
            let payload = crate::state::browser_update_payload(&update);
            let bytes = serde_json::to_vec(&payload).map_err(|err| err.to_string())?;
            if bytes.len() <= MAX_FRAME {
                write_bytes(&mut stream, &bytes).await?;
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
    let bytes = serde_json::to_vec(&reply).map_err(|err| err.to_string())?;
    if bytes.len() > MAX_FRAME {
        state.record_error(&request.method, "server response exceeds size limit".into());
        return write_frame(&mut stream, &Response {
            ok: false, value: Value::Null,
            error: Some("Server response exceeds 8 MiB; use a bounded or paged API (get_thread_history_page for history)".into()),
        }).await;
    }
    write_bytes(&mut stream, &bytes).await
}

async fn write_frame<T: Serialize>(stream: &mut TcpStream, value: &T) -> Result<(), String> {
    let bytes = serde_json::to_vec(value).map_err(|err| err.to_string())?;
    write_bytes(stream, &bytes).await
}

async fn write_bytes(stream: &mut TcpStream, bytes: &[u8]) -> Result<(), String> {
    let len = u32::try_from(bytes.len()).map_err(|err| err.to_string())?;
    if bytes.len() > MAX_FRAME {
        return Err("server frame exceeds size limit".to_owned());
    }
    stream.write_u32(len).await.map_err(|err| err.to_string())?;
    stream.write_all(bytes).await.map_err(|err| err.to_string())
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

/// Runs one command, then tells every subscribed window about thread
/// creation, metadata changes and removal, so an open app follows changes
/// made through the shared server (for example `themis call create_thread`).
pub async fn dispatch(
    state: &AppState,
    sink: Arc<dyn EventSink>,
    method: &str,
    args: Value,
) -> Result<Value, String> {
    let thread_id = args
        .get("threadId")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let removed_root = match (method, &thread_id) {
        ("discard_thread", Some(id)) => state.thread_project_root(id).await,
        _ => None,
    };
    let result = dispatch_command(state, Arc::clone(&sink), method, args).await;
    if let Ok(value) = &result {
        announce_thread_change(state, sink.as_ref(), method, value, thread_id, removed_root).await;
    }
    result
}

async fn announce_thread_change(
    state: &AppState,
    sink: &dyn EventSink,
    method: &str,
    value: &Value,
    thread_id: Option<String>,
    removed_root: Option<String>,
) {
    let (kind, id) = match method {
        "create_thread" => (
            ThreadChangeKind::Created,
            value.get("id").and_then(Value::as_str).map(str::to_owned),
        ),
        "rename_thread"
        | "set_thread_approval_mode"
        | "set_thread_effort"
        | "set_thread_skills" => (ThreadChangeKind::Updated, thread_id),
        "discard_thread" => {
            if let (Some(thread_id), Some(project_root)) = (thread_id, removed_root) {
                sink.emit_thread_changed(&ThreadChange {
                    kind: ThreadChangeKind::Removed,
                    project_root,
                    thread_id,
                    thread: None,
                });
            }
            return;
        }
        _ => return,
    };
    let Some(id) = id else { return };
    let (Some(project_root), Ok(thread)) = (
        state.thread_project_root(&id).await,
        state.get_thread(&id).await,
    ) else {
        return;
    };
    sink.emit_thread_changed(&ThreadChange {
        kind,
        project_root,
        thread_id: id,
        thread: Some(thread),
    });
}

/// The command names and camelCase arguments match the existing Tauri bridge.
async fn dispatch_command(
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
        "get_runtime_configuration" => state.get_runtime_configuration(arg(&args, "projectRoot")?),
        "get_effective_permissions" => {
            state
                .get_effective_permissions(arg(&args, "projectRoot")?, arg(&args, "threadId")?)
                .await
        }
        "save_runtime_configuration" => state.save_runtime_configuration(
            &arg::<String>(&args, "scope")?,
            arg(&args, "projectRoot")?,
            &arg::<String>(&args, "json")?,
        ),
        "query_diagnostic_logs" => output!(state
            .query_diagnostic_logs(serde_json::from_value(args).map_err(|e| e.to_string())?)?),
        "set_thread_approval_mode" => output!(
            state
                .set_thread_approval_mode(arg(&args, "threadId")?, arg(&args, "mode")?)
                .await?
        ),
        "plugin_action" => output!(state.plugin_action(args).await?),
        "list_prompt_skills" => output!(state.prompt_skills(arg(&args, "projectRoot")?).await?),
        "ping" => output!(state.ping().await),
        "get_browser_settings" => output!({
            let (settings, revision) = state.user_browser_view()?;
            json!({
            "settings": settings,
            "revision": revision,
            "browsers": themis_core::browser::launch::installed_browsers(),
            "built_in": state.browser_built_in(),
            "default_downloads_directory": themis_core::browser::session::default_downloads_directory(),
            "starter_sites": themis_core::configuration::STARTER_BLOCKED_SITES,
            })
        }),
        "update_browser_settings" => {
            let changes: serde_json::Map<String, Value> = arg(&args, "changes")?;
            let revision: Option<String> = arg(&args, "revision")?;
            let (settings, revision) =
                state.patch_user_browser_config(changes, revision.as_deref())?;
            output!(json!({"settings": settings, "revision": revision}))
        }
        "get_web_settings" => output!(json!({
            "search_url": state.user_web_config()?.search_url,
            "built_in": crate::state::searxng::status(),
        })),
        "update_web_settings" => {
            let search_url: Option<String> = arg(&args, "searchUrl")?;
            let search_url = search_url
                .map(|url| url.trim().to_owned())
                .filter(|url| !url.is_empty());
            let web = state.update_user_web_config(|web| {
                web.search_url = search_url;
                Ok(())
            })?;
            output!(json!({"search_url": web.search_url}))
        }
        "check_search_server" => {
            let url = match arg::<Option<String>>(&args, "url")? {
                Some(url) => url,
                None => match state.user_web_config()?.search_url {
                    Some(url) => url,
                    // Yours unset: the one built into Themis (started if needed).
                    None => crate::state::searxng::url(&state.app_data_dir()).await?,
                },
            };
            output!(themis_core::tools::web::check_search_server(&url).await)
        }
        "browser_history" => output!(
            state
                .browser_history(
                    arg::<Option<String>>(&args, "projectRoot")?,
                    arg::<Option<String>>(&args, "query")?.unwrap_or_default()
                )
                .await?
        ),
        "remove_browser_history" => done!(
            state
                .remove_browser_history(arg(&args, "projectRoot")?, arg(&args, "url")?)
                .await
        ),
        "clear_browsing_data" => output!(
            state
                .clear_browsing_data(
                    arg::<Option<String>>(&args, "projectRoot")?,
                    arg(&args, "range")?,
                    arg(&args, "types")?
                )
                .await?
        ),
        "open_project" => output!(state.open_project(arg(&args, "path")?).await?),
        "rename_project" => output!(
            state
                .rename_project(arg(&args, "path")?, arg(&args, "name")?)
                .await?
        ),
        "delete_project" => done!(state.delete_project(arg(&args, "path")?).await),
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
        "get_context_usage" => output!(
            state
                .get_context_usage(
                    &arg::<String>(&args, "threadId")?,
                    arg::<Option<String>>(&args, "runId")?.as_deref()
                )
                .await?
        ),
        "get_thread_history" => output!(
            state
                .get_thread_history(&arg::<String>(&args, "threadId")?)
                .await?
        ),
        "get_thread_history_page" => output!(
            state
                .get_thread_history_page(
                    &arg::<String>(&args, "threadId")?,
                    arg::<Option<HistoryCursor>>(&args, "cursor")?.unwrap_or_default(),
                    arg::<Option<i64>>(&args, "throughSeq")?,
                    arg::<Option<usize>>(&args, "maxBytes")?.unwrap_or(MAX_PAGE_BYTES),
                )
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
        "get_pending_approvals" => output!(state.pending_approvals()),
        "list_threads" => output!(state.list_threads(arg(&args, "projectRoot")?).await?),
        "merge_thread" => output!(state.merge_thread(arg(&args, "threadId")?).await?),
        "discard_thread" => done!(state.discard_thread(arg(&args, "threadId")?).await),
        "attachment_path" => output!(
            state
                .attachment_path(arg(&args, "threadId")?, arg(&args, "path")?)
                .await?
        ),
        "attach_files" => output!(
            state
                .attach_files(arg(&args, "threadId")?, arg(&args, "paths")?)
                .await?
        ),
        "send_message" => output!(
            crate::state::configuration::RUN_OVERRIDE
                .scope(
                    arg(&args, "runtimeConfiguration")?,
                    state.send_message_with_attachments(
                        sink,
                        arg(&args, "threadId")?,
                        arg(&args, "text")?,
                        arg(&args, "reasoningEffort")?,
                        serde_json::from_value(
                            args.get("attachments")
                                .cloned()
                                .unwrap_or_else(|| serde_json::json!([]))
                        )
                        .map_err(|e| e.to_string())?
                    )
                )
                .await?
        ),
        "list_diff" => output!(state.list_diff(arg(&args, "threadId")?).await?),
        "get_workspace" => output!(
            state
                .get_workspace(
                    arg(&args, "projectRoot")?,
                    args.get("pullRequests")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                )
                .await?
        ),
        "list_project_dir" => output!(
            state
                .list_project_dir(arg(&args, "projectRoot")?, arg(&args, "path")?)
                .await?
        ),
        "read_project_file" => output!(
            state
                .read_project_file(arg(&args, "projectRoot")?, arg(&args, "path")?)
                .await?
        ),
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
        "set_provider" => {
            let thread_id: String = arg(&args, "threadId")?;
            let info = state
                .set_provider(
                    thread_id.clone(),
                    arg::<ProviderKind>(&args, "provider")?,
                    arg(&args, "model")?,
                )
                .await?;
            state.goal_model_changed(sink, &thread_id).await;
            output!(info)
        }
        "list_queue" => output!(state.list_queue(arg(&args, "threadId")?).await?),
        "enqueue_message" => output!(
            state
                .enqueue_message(
                    sink,
                    arg(&args, "threadId")?,
                    arg(&args, "text")?,
                    arg(&args, "reasoningEffort")?,
                    arg::<Option<Vec<String>>>(&args, "attachments")?.unwrap_or_default(),
                )
                .await?
        ),
        "update_queued_message" => output!(
            state
                .update_queued_message(
                    sink,
                    arg(&args, "threadId")?,
                    arg(&args, "messageId")?,
                    arg(&args, "text")?,
                    arg(&args, "held")?,
                )
                .await?
        ),
        "remove_queued_message" => done!(
            state
                .remove_queued_message(sink, arg(&args, "threadId")?, arg(&args, "messageId")?)
                .await
        ),
        "steer_queued_message" => done!(
            state
                .steer_queued_message(sink, arg(&args, "threadId")?, arg(&args, "messageId")?)
                .await
        ),
        "get_goal" => output!(state.get_goal(arg(&args, "threadId")?).await?),
        "set_goal" => output!(
            state
                .set_goal(sink, arg(&args, "threadId")?, arg(&args, "text")?)
                .await?
        ),
        "edit_goal" => output!(
            state
                .edit_goal(sink, arg(&args, "threadId")?, arg(&args, "text")?)
                .await?
        ),
        "pause_goal" => output!(state.pause_goal(sink, arg(&args, "threadId")?).await?),
        "resume_goal" => output!(state.resume_goal(sink, arg(&args, "threadId")?).await?),
        "remove_goal" => output!(state.remove_goal(sink, arg(&args, "threadId")?).await?),
        "set_thread_effort" => output!(
            state
                .set_thread_effort(arg(&args, "threadId")?, arg(&args, "effort")?)
                .await?
        ),
        "ensure_runtime_config" => output!(state.ensure_runtime_config()?),
        "open_runtime_config" => done!(state.open_runtime_config(arg(&args, "editor")?).await),
        "open_mcp_config" => output!(
            state
                .open_mcp_config(
                    arg(&args, "editor")?,
                    arg::<Option<bool>>(&args, "open")?.unwrap_or(true)
                )
                .await?
        ),
        "apply_mcp_config" => output!(state.apply_mcp_config().await?),
        "mcp_config_status" => output!(state.mcp_config_status()),
        "list_keys" => output!(state.list_keys(arg(&args, "projectRoot")?).await?),
        "set_key" => done!(state.set_key(
            &arg::<String>(&args, "name")?,
            &arg::<String>(&args, "value")?
        )),
        "remove_key" => done!(state.remove_key(&arg::<String>(&args, "name")?)),
        "list_editors" => output!(json!({"editors": crate::state::editors::installed_editors()})),
        "open_in_editor" => done!(
            state
                .open_in_editor(arg(&args, "path")?, arg(&args, "line")?)
                .await
        ),
        "record_notification" => done!(state.record_notification(arg(&args, "record")?)),
        "get_settings" => output!(state.get_settings().await),
        "update_settings" => output!(
            state
                .update_settings(arg::<SettingsPatch>(&args, "patch")?)
                .await?
        ),
        "get_secret_status" => output!(state.get_secret_status().await?),
        "check_go_key" => output!(state.check_go_key().await?),
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
        "browser_state" => {
            state
                .browser_state(&arg::<String>(&args, "threadId")?)
                .await
        }
        "browser_screencast" => done!(
            state
                .browser_screencast(
                    &arg::<String>(&args, "threadId")?,
                    arg(&args, "width")?,
                    arg(&args, "height")?,
                    arg(&args, "scale")?,
                )
                .await
        ),
        "browser_screencast_stop" => {
            done!(
                state
                    .browser_screencast_stop(&arg::<String>(&args, "threadId")?)
                    .await
            )
        }
        "browser_input" => done!(
            state
                .browser_input(&arg::<String>(&args, "threadId")?, args["input"].clone())
                .await
        ),
        "browser_set_muted" => done!(
            state
                .browser_set_muted(&arg::<String>(&args, "threadId")?, arg(&args, "muted")?)
                .await
        ),
        "browser_open_url" => done!(
            state
                .browser_open_url(
                    &arg::<String>(&args, "threadId")?,
                    &arg::<String>(&args, "address")?,
                )
                .await
        ),
        "browser_navigate" => done!(
            state
                .browser_navigate(
                    &arg::<String>(&args, "threadId")?,
                    &arg::<String>(&args, "action")?,
                )
                .await
        ),
        "browser_tab" => done!(
            state
                .browser_tab(
                    &arg::<String>(&args, "threadId")?,
                    &arg::<String>(&args, "action")?,
                    arg(&args, "tab")?,
                )
                .await
        ),
        "browser_capture" => {
            state
                .browser_capture(&arg::<String>(&args, "threadId")?)
                .await
        }
        "browser_record" => {
            state
                .browser_record(
                    &arg::<String>(&args, "threadId")?,
                    &arg::<String>(&args, "action")?,
                )
                .await
        }
        "browser_devtools_url" => {
            output!(
                state
                    .browser_devtools_url(&arg::<String>(&args, "threadId")?)
                    .await?
            )
        }
        "get_thread_tasks" => output!(
            state
                .get_thread_tasks(&arg::<String>(&args, "threadId")?)
                .await?
        ),
        "stop_background_task" => output!(
            state
                .stop_background_task(arg(&args, "threadId")?, arg(&args, "taskId")?)
                .await?
        ),
        "get_background_task_log" => output!(
            state
                .get_background_task_log(
                    arg(&args, "threadId")?,
                    arg(&args, "taskId")?,
                    arg::<Option<u64>>(&args, "offset")?,
                    arg::<Option<usize>>(&args, "maxBytes")?,
                )
                .await?
        ),
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
        "resource_usage" => output!(state.resource_usage()),
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

#[cfg(test)]
mod patience_tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[tokio::test]
    async fn a_listening_server_gets_time_to_answer_and_a_missing_one_does_not() {
        let data = tempfile::tempdir().unwrap();
        let client = Client::new(data.path().to_path_buf());
        // No discovery file: no server, decided at once.
        let started = Instant::now();
        assert!(!client.answers(Duration::from_secs(5)).await);
        assert!(started.elapsed() < Duration::from_secs(2));
        // A busy server: it accepts connections but is slow to reply. Not replaced early.
        let busy = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let port = busy.local_addr().unwrap().port();
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((stream, _)) = busy.accept().await {
                held.push(stream);
            }
        });
        write_discovery(
            data.path(),
            &Discovery {
                port,
                token: "t".into(),
            },
        )
        .unwrap();
        let started = Instant::now();
        assert!(!client.answers(Duration::from_millis(1500)).await);
        assert!(started.elapsed() >= Duration::from_millis(1500));
    }
}
