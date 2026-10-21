//! A small Chrome DevTools Protocol client: one WebSocket to the browser,
//! flattened target sessions, request/response by id and a broadcast of events.
//!
//! Everything the browser sends is untrusted (pages influence console text,
//! titles, URLs): [`parse_incoming`] accepts only well-formed messages and
//! never panics.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;

/// How long one command may take before it fails.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(30);
/// Largest message accepted (full-page screenshots are big).
const MAX_MESSAGE_BYTES: usize = 64 * 1024 * 1024;
/// Events buffered per subscriber; a slow subscriber skips ahead.
const EVENT_BUFFER: usize = 2048;

/// One protocol event, with the target session it came from.
#[derive(Debug, Clone, PartialEq)]
pub struct CdpEvent {
    pub method: String,
    pub params: Value,
    pub session: Option<String>,
}

/// A parsed message from the browser.
#[derive(Debug, Clone, PartialEq)]
pub enum Incoming {
    Response {
        id: u64,
        result: Result<Value, String>,
    },
    Event(CdpEvent),
}

/// Parses one browser message; `None` for anything that is not a response
/// with a numeric id or an event with a method name.
pub fn parse_incoming(text: &str) -> Option<Incoming> {
    let value: Value = serde_json::from_str(text).ok()?;
    let object = value.as_object()?;
    let session = object
        .get("sessionId")
        .and_then(Value::as_str)
        .map(str::to_owned);
    if let Some(id) = object.get("id") {
        let id = id.as_u64()?;
        let result = match object.get("error") {
            Some(error) => Err(error.get("message").and_then(Value::as_str).map_or_else(
                || "browser error".to_owned(),
                |m| m.chars().take(500).collect(),
            )),
            None => Ok(object.get("result").cloned().unwrap_or(Value::Null)),
        };
        return Some(Incoming::Response { id, result });
    }
    let method = object.get("method")?.as_str()?;
    if method.is_empty() || method.len() > 128 {
        return None;
    }
    Some(Incoming::Event(CdpEvent {
        method: method.to_owned(),
        params: object.get("params").cloned().unwrap_or(Value::Null),
        session,
    }))
}

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>>;

/// A connection to one browser.
pub struct Cdp {
    outgoing: mpsc::UnboundedSender<String>,
    pending: Pending,
    next_id: AtomicU64,
    events: broadcast::Sender<CdpEvent>,
    closed: Arc<AtomicBool>,
}

impl std::fmt::Debug for Cdp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cdp")
            .field("closed", &self.closed.load(Ordering::SeqCst))
            .finish_non_exhaustive()
    }
}

impl Cdp {
    /// Connects to a browser's `ws://127.0.0.1:<port>/devtools/browser/<id>`.
    pub async fn connect(url: &str) -> anyhow::Result<Arc<Self>> {
        let config = tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default()
            .max_message_size(Some(MAX_MESSAGE_BYTES))
            .max_frame_size(Some(MAX_MESSAGE_BYTES));
        let (socket, _) =
            tokio_tungstenite::connect_async_with_config(url, Some(config), false).await?;
        let (mut sink, mut stream) = socket.split();
        let (outgoing, mut queue) = mpsc::unbounded_channel::<String>();
        let pending: Pending = Arc::default();
        let (events, _) = broadcast::channel(EVENT_BUFFER);
        let closed = Arc::new(AtomicBool::new(false));
        tokio::spawn(async move {
            while let Some(text) = queue.recv().await {
                if sink.send(Message::text(text)).await.is_err() {
                    break;
                }
            }
            let _ = sink.close().await;
        });
        let reader_pending = Arc::clone(&pending);
        let reader_events = events.clone();
        let reader_closed = Arc::clone(&closed);
        tokio::spawn(async move {
            while let Some(Ok(message)) = stream.next().await {
                let text = match &message {
                    Message::Text(text) => text.as_str(),
                    Message::Close(_) => break,
                    _ => continue,
                };
                match parse_incoming(text) {
                    Some(Incoming::Response { id, result }) => {
                        let waiter = reader_pending
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .remove(&id);
                        if let Some(waiter) = waiter {
                            let _ = waiter.send(result);
                        }
                    }
                    Some(Incoming::Event(event)) => {
                        let _ = reader_events.send(event);
                    }
                    None => {}
                }
            }
            reader_closed.store(true, Ordering::SeqCst);
            // Fail every waiting call: the browser is gone.
            let waiting: Vec<_> = reader_pending
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .drain()
                .collect();
            for (_, waiter) in waiting {
                let _ = waiter.send(Err("the browser closed".to_owned()));
            }
        });
        Ok(Arc::new(Self {
            outgoing,
            pending,
            next_id: AtomicU64::new(1),
            events,
            closed,
        }))
    }

    /// Whether the connection is gone (the browser exited or crashed).
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    /// Every event from now on.
    pub fn subscribe(&self) -> broadcast::Receiver<CdpEvent> {
        self.events.subscribe()
    }

    /// Sends one command and waits for its result.
    pub async fn call(
        &self,
        session: Option<&str>,
        method: &str,
        params: Value,
    ) -> Result<Value, String> {
        self.call_with_timeout(session, method, params, CALL_TIMEOUT)
            .await
    }

    pub async fn call_with_timeout(
        &self,
        session: Option<&str>,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, String> {
        if self.is_closed() {
            return Err("the browser closed".to_owned());
        }
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let mut message = json!({"id": id, "method": method, "params": params});
        if let Some(session) = session {
            message["sessionId"] = json!(session);
        }
        let (waiter, result) = oneshot::channel();
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id, waiter);
        if self.outgoing.send(message.to_string()).is_err() {
            self.pending
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&id);
            return Err("the browser closed".to_owned());
        }
        match tokio::time::timeout(timeout, result).await {
            Ok(Ok(result)) => result.map_err(|error| format!("{method}: {error}")),
            Ok(Err(_)) => Err("the browser closed".to_owned()),
            Err(_) => {
                self.pending
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&id);
                Err(format!("{method} timed out after {} s", timeout.as_secs()))
            }
        }
    }

    /// Sends a command without waiting (screencast acknowledgements, input
    /// the caller does not need confirmed).
    pub fn send(&self, session: Option<&str>, method: &str, params: Value) {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let mut message = json!({"id": id, "method": method, "params": params});
        if let Some(session) = session {
            message["sessionId"] = json!(session);
        }
        let _ = self.outgoing.send(message.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn responses_errors_and_events_parse() {
        assert_eq!(
            parse_incoming(r#"{"id":3,"result":{"a":1}}"#),
            Some(Incoming::Response {
                id: 3,
                result: Ok(json!({"a":1}))
            })
        );
        assert_eq!(
            parse_incoming(r#"{"id":4,"error":{"code":-32000,"message":"No node"}}"#),
            Some(Incoming::Response {
                id: 4,
                result: Err("No node".into())
            })
        );
        assert_eq!(
            parse_incoming(
                r#"{"method":"Page.loadEventFired","params":{"timestamp":1},"sessionId":"S"}"#
            ),
            Some(Incoming::Event(CdpEvent {
                method: "Page.loadEventFired".into(),
                params: json!({"timestamp":1}),
                session: Some("S".into()),
            }))
        );
        for bad in [
            "",
            "[]",
            "{}",
            r#"{"id":-1}"#,
            r#"{"id":"x"}"#,
            r#"{"method":""}"#,
            r#"{"method":5}"#,
        ] {
            assert_eq!(parse_incoming(bad), None, "{bad}");
        }
    }

    proptest! {
        #[test]
        fn any_text_parses_without_panicking(text in ".{0,200}") {
            let _ = parse_incoming(&text);
        }

        #[test]
        fn shaped_messages_keep_their_id_or_method(id in any::<u64>(), method in "[A-Za-z.]{1,40}", session in proptest::option::of("[A-F0-9]{1,32}")) {
            let response = json!({"id": id, "result": {}, "sessionId": session}).to_string();
            let parsed_response = matches!(parse_incoming(&response), Some(Incoming::Response { id: got, .. }) if got == id);
            prop_assert!(parsed_response);
            let event = json!({"method": method, "params": {}, "sessionId": session}).to_string();
            match parse_incoming(&event) {
                Some(Incoming::Event(event)) => {
                    prop_assert_eq!(event.method, method);
                    prop_assert_eq!(event.session, session);
                }
                other => prop_assert!(false, "{other:?}"),
            }
        }
    }
}
