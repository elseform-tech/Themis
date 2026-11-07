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

mod client;
mod commands;

pub use client::Client;
pub use commands::dispatch;

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
        // Chats left alone end their plugin hook sessions.
        let sessions = self.state.clone();
        let mut sessions_stop = self.shutdown.subscribe();
        tokio::spawn(async move {
            let mut every = tokio::time::interval(crate::state::HOOK_IDLE_CHECK_EVERY);
            loop {
                tokio::select! {
                    _ = every.tick() => {
                        sessions.end_idle_hook_sessions_at(std::time::Instant::now()).await;
                    }
                    _ = sessions_stop.recv() => break,
                }
            }
        });
        // A browser no chat has used for a while exits, so it doesn't sit in memory.
        let browsers = self.state.clone();
        let mut browsers_stop = self.shutdown.subscribe();
        tokio::spawn(async move {
            let mut every = tokio::time::interval(std::time::Duration::from_secs(60));
            loop {
                tokio::select! {
                    _ = every.tick() => browsers.exit_idle_browser().await,
                    _ = browsers_stop.recv() => break,
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
        // Open plugin hook sessions end here, within a short budget.
        self.state.end_all_hook_sessions().await;
        // Background programs belong to this server: stop them with it.
        self.state.shutdown_background_tasks().await;
        self.state.shutdown_terminals().await;
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
    if request.method == "subscribe_terminal" {
        // On its own connection, like the browser's: a noisy program never crowds the app's
        // events. A reader that falls behind what is kept picks up at the oldest text.
        let root = arg::<String>(&request.args, "root")?;
        let Some(session) = state.terminal_session(&root) else {
            return write_frame(
                &mut stream,
                &Response {
                    ok: false,
                    value: Value::Null,
                    error: Some("This terminal isn't open".to_owned()),
                },
            )
            .await;
        };
        let mut updates = session.updates();
        let (scrollback, mut offset) = session.scrollback();
        let mut value = state.terminal_state(&root);
        value["scrollback"] = json!(scrollback);
        write_frame(
            &mut stream,
            &Response {
                ok: true,
                value,
                error: None,
            },
        )
        .await?;
        let mut gone = [0u8; 1];
        loop {
            // Whether it ended is read first: its output is all in by then.
            let ended = session.ended();
            loop {
                let (data, next) = session.since(offset);
                if data.is_empty() {
                    break;
                }
                offset = next;
                write_frame(&mut stream, &json!({"kind": "output", "data": data})).await?;
            }
            if let Some(code) = ended {
                return write_frame(&mut stream, &json!({"kind": "exit", "code": code})).await;
            }
            tokio::select! {
                changed = updates.changed() => {
                    if changed.is_err() {
                        return Ok(());
                    }
                }
                // The reader sends nothing, so this only returns when it has left.
                _ = stream.read(&mut gone) => return Ok(()),
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
