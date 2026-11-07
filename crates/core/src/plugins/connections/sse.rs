//! The older server-sent events transport: reading the stream and finding its message address.

use super::*;

/// Most of one server-sent event kept, in bytes; past that the stream is given up on.
pub(super) const SSE_EVENT_BYTES: usize = 1024 * 1024;

/// Events waiting to be read.
const SSE_BACKLOG: usize = 64;

/// Ends the stream's reader with the connection.
pub(super) struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// One server-sent event: its name (`message` when none is given) and its data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseEvent {
    pub event: String,
    pub data: String,
}

/// Reads server-sent events from a stream in whatever chunks they arrive. The text is the server's
/// and untrusted: bounded, and it never fails on what it can't read.
#[derive(Debug, Default)]
pub struct SseParser {
    /// Bytes of the line still arriving.
    pub(super) line: Vec<u8>,
    event: String,
    pub(super) data: String,
    /// A CR ended the last line, so a LF starting this chunk belongs to it.
    after_cr: bool,
}

impl SseParser {
    /// The events `chunk` completes. `Err` once an event is past [`SSE_EVENT_BYTES`].
    pub fn feed(&mut self, chunk: &[u8]) -> Result<Vec<SseEvent>, &'static str> {
        let mut events = Vec::new();
        for &byte in chunk {
            if std::mem::take(&mut self.after_cr) && byte == b'\n' {
                continue;
            }
            if byte == b'\n' || byte == b'\r' {
                self.after_cr = byte == b'\r';
                let line = std::mem::take(&mut self.line);
                self.line_done(&line, &mut events);
            } else {
                self.line.push(byte);
            }
            if self.line.len() + self.data.len() > SSE_EVENT_BYTES {
                return Err("MCP event exceeds 1 MiB");
            }
        }
        Ok(events)
    }

    fn line_done(&mut self, line: &[u8], events: &mut Vec<SseEvent>) {
        if line.is_empty() {
            if !self.data.is_empty() {
                let event = std::mem::take(&mut self.event);
                events.push(SseEvent {
                    event: if event.is_empty() {
                        "message".to_owned()
                    } else {
                        event
                    },
                    data: std::mem::take(&mut self.data),
                });
            }
            self.event.clear();
            return;
        }
        let line = String::from_utf8_lossy(line);
        let (field, value) = line.split_once(':').unwrap_or((&line, ""));
        let value = value.strip_prefix(' ').unwrap_or(value);
        match field {
            "event" => value.clone_into(&mut self.event),
            "data" => {
                if !self.data.is_empty() {
                    self.data.push('\n');
                }
                self.data.push_str(value);
            }
            _ => {}
        }
    }
}

/// Where a server's stream says to send messages, as an absolute address. It must stay on the
/// server the person allowed: same scheme, host and port, and no user name to smuggle one in.
pub fn sse_endpoint(stream_url: &str, named: &str) -> anyhow::Result<String> {
    let base = reqwest::Url::parse(stream_url).context("Invalid MCP URL")?;
    let target = base
        .join(named.trim())
        .context("Invalid MCP message address")?;
    if target.scheme() != base.scheme()
        || target.host_str() != base.host_str()
        || target.port_or_known_default() != base.port_or_known_default()
        || !target.username().is_empty()
        || target.password().is_some()
    {
        bail!("The MCP server's message address leaves the server");
    }
    Ok(target.to_string())
}

/// What a failed answer from a remote server means.
pub(super) fn http_failure(response: &reqwest::Response) -> anyhow::Error {
    let status = response.status();
    crate::diagnostics::emit(
        "mcp",
        "http_failed",
        "warn",
        json!({"http_status":status.as_u16(),"error_category":if matches!(status.as_u16(),401|403){"authentication"}else{"remote_http"}}),
    );
    // A Bearer challenge on 401 is MCP's way to ask for an OAuth sign-in.
    let challenge = response
        .headers()
        .get("www-authenticate")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.to_ascii_lowercase().starts_with("bearer"));
    if status == reqwest::StatusCode::UNAUTHORIZED && challenge {
        return anyhow::anyhow!("MCP server needs sign-in");
    }
    anyhow::anyhow!("MCP server returned HTTP {status}")
}

/// Opens a server's stream and waits for the address it names. Whatever comes after arrives on the
/// receiver, which closes when the stream ends.
pub(super) async fn open_sse(
    url: &str,
    bearer: Option<&str>,
    headers: &[(String, String)],
) -> anyhow::Result<(
    reqwest::Client,
    String,
    tokio::sync::mpsc::Receiver<Value>,
    AbortOnDrop,
)> {
    // No whole-request time limit: the answer to this one is a stream that stays open.
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let mut request = client.get(url).header("Accept", "text/event-stream");
    if let Some(token) = bearer {
        request = request.bearer_auth(token);
    }
    for (name, value) in headers {
        request = request.header(name.as_str(), value.as_str());
    }
    let mut response = tokio::time::timeout(Duration::from_secs(30), request.send())
        .await
        .context("MCP connection timed out")?
        .context("MCP connection failed")?;
    if !response.status().is_success() {
        return Err(http_failure(&response));
    }
    let streaming = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.to_ascii_lowercase().starts_with("text/event-stream"));
    if !streaming {
        bail!("MCP server didn't answer with an event stream");
    }
    let mut parser = SseParser::default();
    let (send, incoming) = tokio::sync::mpsc::channel(SSE_BACKLOG);
    let (named, endpoint) = tokio::sync::oneshot::channel::<String>();
    let reader = tokio::spawn(async move {
        let mut named = Some(named);
        loop {
            let Ok(Some(chunk)) = response.chunk().await else {
                return;
            };
            let Ok(events) = parser.feed(&chunk) else {
                return;
            };
            for event in events {
                match event.event.as_str() {
                    "endpoint" => {
                        if let Some(named) = named.take() {
                            let _ = named.send(event.data);
                        }
                    }
                    "message" => {
                        // What can't be read as a message is skipped, as a stray line is.
                        if let Ok(value) = serde_json::from_str::<Value>(&event.data) {
                            if send.send(value).await.is_err() {
                                return;
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    });
    let reader = AbortOnDrop(reader);
    let named = tokio::time::timeout(Duration::from_secs(30), endpoint)
        .await
        .context("MCP server named no message address")?
        .context("MCP server disconnected")?;
    let endpoint = sse_endpoint(url, &named)?;
    Ok((client, endpoint, incoming, reader))
}
