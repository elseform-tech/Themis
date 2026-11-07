//! The client side: discovery, framed calls and event subscriptions.

use super::*;

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

    pub(super) async fn call_raw(&self, method: &str, args: Value) -> Result<Value, String> {
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

    /// One folder's terminal output; the first value has what it kept and whether it runs.
    pub async fn subscribe_terminal(&self, root: &str) -> Result<(TcpStream, Value), String> {
        let (mut stream, discovery) = self.connect().await?;
        write_frame(
            &mut stream,
            &Request {
                token: discovery.token,
                method: "subscribe_terminal".to_owned(),
                args: json!({"root": root}),
            },
        )
        .await?;
        let reply: Response = read_frame(&mut stream).await?;
        if reply.ok {
            Ok((stream, reply.value))
        } else {
            Err(reply
                .error
                .unwrap_or_else(|| "terminal subscription failed".to_owned()))
        }
    }

    pub async fn next_event(stream: &mut TcpStream) -> Result<Value, String> {
        read_frame(stream).await
    }

    /// Whether a server owns this data directory. One whose port still accepts connections
    /// gets up to `patience` to answer a ping: a busy server is not a missing one, and
    /// starting a second would take its place. A server that refuses the token in
    /// `server.json` is a definite no: it does not own this folder (a server stops once
    /// `server.json` stops naming it), so it is not asked again. Tries pause between them.
    pub async fn answers(&self, patience: std::time::Duration) -> bool {
        const PAUSE: std::time::Duration = std::time::Duration::from_millis(100);
        let deadline = tokio::time::Instant::now() + patience;
        loop {
            match tokio::time::timeout(
                std::time::Duration::from_millis(500),
                self.call("ping", Value::Null),
            )
            .await
            {
                Ok(Ok(_)) => return true,
                Ok(Err(error)) if error == UNAUTHORIZED => return false,
                _ => {}
            }
            let listening = match self.discovery() {
                Ok(discovery) => {
                    TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, discovery.port))
                        .await
                        .is_ok()
                }
                Err(_) => false,
            };
            let now = tokio::time::Instant::now();
            if !listening || now >= deadline {
                return false;
            }
            tokio::time::sleep(PAUSE.min(deadline - now)).await;
        }
    }

    fn discovery(&self) -> Result<Discovery, String> {
        if let Some(agent) = agent_discovery() {
            return agent;
        }
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

/// In an agent's shell: its run's credential and the server's port, instead of the discovery file
/// (which the sandbox hides). `None` outside one.
fn agent_discovery() -> Option<Result<Discovery, String>> {
    let token = std::env::var(themis_core::agent_cli::TOKEN_ENV).ok()?;
    let port = std::env::var(themis_core::agent_cli::PORT_ENV).unwrap_or_default();
    Some(match port.parse::<u16>() {
        Ok(port) if port > 0 && !token.is_empty() => Ok(Discovery { port, token }),
        _ => Err(format!(
            "{} needs {} to name the app server's port",
            themis_core::agent_cli::TOKEN_ENV,
            themis_core::agent_cli::PORT_ENV
        )),
    })
}

/// Whether this process runs in an agent's shell (its calls carry the run's credential).
pub(crate) fn in_agent_shell() -> bool {
    std::env::var_os(themis_core::agent_cli::TOKEN_ENV).is_some()
}
