//! Per-chat prompt queue and steering.
//!
//! A message sent while a chat's run is going waits in the chat's queue
//! (`queues/<chat id>.json` beside `threads.json`) and goes out by itself,
//! oldest first, when the run ends. Steering moves a waiting message into the
//! running run instead: the run reads it before its next model request. A
//! steer the run never picked up returns to the front of the queue.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::json;
use themis_core::runtime::SteerInbox;

use super::chat_grants::safe_chat_id;
use super::AppState;
use crate::sink::EventSink;
use crate::types::{QueuedMessage, ThreadEvent, ThreadEventEnvelope};

/// Directory name under the app data directory.
pub(super) const QUEUES_DIR: &str = "queues";
/// Most messages one chat can have waiting.
const MAX_QUEUED: usize = 50;
/// Longest queued message, in characters.
const MAX_QUEUED_CHARS: usize = 100_000;
/// Wait before retrying a queued message the concurrency limit turned away.
const BUSY_RETRY_SECS: u64 = 5;

impl AppState {
    fn queue_path(&self, thread_id: &str) -> Option<PathBuf> {
        safe_chat_id(thread_id).then(|| self.inner.queues_dir.join(format!("{thread_id}.json")))
    }

    /// The chat's queue. A damaged file is ignored (an empty queue) and noted
    /// in diagnostics with a fixed reason; it never blocks the chat.
    pub(super) fn queue_for(&self, thread_id: &str) -> Vec<QueuedMessage> {
        let mut queues = self.inner.queues.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(queue) = queues.get(thread_id) {
            return queue.clone();
        }
        let queue = match self.queue_path(thread_id).map(std::fs::read_to_string) {
            Some(Ok(text)) => serde_json::from_str(&text).unwrap_or_else(|_| {
                let _ = self.inner.diagnostics.append(
                    "queue",
                    "queue_ignored",
                    "warn",
                    json!({"thread_id": thread_id, "reason": "unreadable queue file"}),
                );
                Vec::new()
            }),
            _ => Vec::new(),
        };
        queues.insert(thread_id.to_owned(), queue.clone());
        queue
    }

    fn save_queue(&self, thread_id: &str, queue: &[QueuedMessage]) {
        self.inner
            .queues
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(thread_id.to_owned(), queue.to_vec());
        let Some(path) = self.queue_path(thread_id) else {
            return;
        };
        let result = if queue.is_empty() {
            match std::fs::remove_file(&path) {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                    Err(error.to_string())
                }
                _ => Ok(()),
            }
        } else {
            super::write_json_file(&path, &queue)
        };
        if let Err(error) = result {
            self.record_error("save_queue", error);
        }
    }

    fn announce_queue(&self, sink: &dyn EventSink, thread_id: &str, queue: Vec<QueuedMessage>) {
        sink.emit_thread_event(&ThreadEventEnvelope {
            thread_id: thread_id.to_owned(),
            run_id: String::new(),
            event: ThreadEvent::QueueChanged { queue },
        });
    }

    /// Changes the chat's queue under one lock and announces the result.
    fn edit_queue<T>(
        &self,
        sink: &dyn EventSink,
        thread_id: &str,
        change: impl FnOnce(&mut Vec<QueuedMessage>) -> Result<T, String>,
    ) -> Result<T, String> {
        let _guard = self
            .inner
            .queue_lock
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut queue = self.queue_for(thread_id);
        let value = change(&mut queue)?;
        self.save_queue(thread_id, &queue);
        self.announce_queue(sink, thread_id, queue);
        Ok(value)
    }

    async fn require_thread(&self, thread_id: &str) -> Result<bool, String> {
        self.inner
            .threads
            .read()
            .await
            .get(thread_id)
            .map(|record| record.running)
            .ok_or_else(|| format!("unknown thread '{thread_id}'"))
    }

    /// The chat's waiting messages, oldest first.
    pub async fn list_queue(&self, thread_id: String) -> Result<Vec<QueuedMessage>, String> {
        self.require_thread(&thread_id).await?;
        Ok(self.queue_for(&thread_id))
    }

    /// Adds a message to the chat's queue. When no run is going it is sent
    /// right away, so a message is never stranded by a run that just ended.
    pub async fn enqueue_message(
        &self,
        sink: Arc<dyn EventSink>,
        thread_id: String,
        text: String,
        reasoning_effort: Option<String>,
        attachments: Vec<String>,
    ) -> Result<QueuedMessage, String> {
        let running = self.require_thread(&thread_id).await?;
        let text = text.trim().to_owned();
        if text.is_empty() && attachments.is_empty() {
            return Err("Write a message to queue".to_owned());
        }
        if text.chars().count() > MAX_QUEUED_CHARS {
            return Err("This message is too long to queue".to_owned());
        }
        let message = QueuedMessage {
            id: uuid::Uuid::new_v4().to_string(),
            text,
            attachments,
            reasoning_effort,
            held: false,
        };
        self.edit_queue(sink.as_ref(), &thread_id, |queue| {
            if queue.len() >= MAX_QUEUED {
                return Err(format!("At most {MAX_QUEUED} messages can wait in a chat"));
            }
            queue.push(message.clone());
            Ok(())
        })?;
        let _ = self.inner.diagnostics.append(
            "queue",
            "enqueued",
            "info",
            json!({"thread_id": thread_id, "queued": self.queue_for(&thread_id).len()}),
        );
        if !running {
            self.dispatch_queue(sink, &thread_id).await;
        }
        Ok(message)
    }

    /// Edits a waiting message's text, or holds it while it is being edited.
    pub async fn update_queued_message(
        &self,
        sink: Arc<dyn EventSink>,
        thread_id: String,
        message_id: String,
        text: Option<String>,
        held: Option<bool>,
    ) -> Result<QueuedMessage, String> {
        self.require_thread(&thread_id).await?;
        let updated = self.edit_queue(sink.as_ref(), &thread_id, |queue| {
            let message = queue
                .iter_mut()
                .find(|message| message.id == message_id)
                .ok_or_else(|| "That message was already sent or removed".to_owned())?;
            if let Some(text) = text {
                let text = text.trim().to_owned();
                if text.is_empty() && message.attachments.is_empty() {
                    return Err("A queued message cannot be empty; remove it instead".to_owned());
                }
                if text.chars().count() > MAX_QUEUED_CHARS {
                    return Err("This message is too long to queue".to_owned());
                }
                message.text = text;
            }
            if let Some(held) = held {
                message.held = held;
            }
            Ok(message.clone())
        })?;
        // A saved edit may be the only thing a finished chat was waiting for.
        if !updated.held && !self.require_thread(&thread_id).await? {
            self.dispatch_queue(sink, &thread_id).await;
        }
        Ok(updated)
    }

    /// Removes a waiting message before it is sent.
    pub async fn remove_queued_message(
        &self,
        sink: Arc<dyn EventSink>,
        thread_id: String,
        message_id: String,
    ) -> Result<(), String> {
        self.require_thread(&thread_id).await?;
        self.edit_queue(sink.as_ref(), &thread_id, |queue| {
            let before = queue.len();
            queue.retain(|message| message.id != message_id);
            if queue.len() == before {
                return Err("That message was already sent or removed".to_owned());
            }
            Ok(())
        })
    }

    /// Delivers a waiting message into the chat's running run.
    pub async fn steer_queued_message(
        &self,
        sink: Arc<dyn EventSink>,
        thread_id: String,
        message_id: String,
    ) -> Result<(), String> {
        self.require_thread(&thread_id).await?;
        let inbox = self
            .inner
            .steer_inboxes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&thread_id)
            .cloned()
            .ok_or_else(|| "No run is going; send the message instead".to_owned())?;
        let text = self.edit_queue(sink.as_ref(), &thread_id, |queue| {
            let index = queue
                .iter()
                .position(|message| message.id == message_id)
                .ok_or_else(|| "That message was already sent or removed".to_owned())?;
            if !queue[index].attachments.is_empty() {
                return Err("Messages with attachments wait for the next turn".to_owned());
            }
            Ok(queue.remove(index).text)
        })?;
        inbox
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_back(text);
        let _ = self.inner.diagnostics.append(
            "queue",
            "steered",
            "info",
            json!({"thread_id": thread_id}),
        );
        Ok(())
    }

    /// A fresh steer inbox for the run starting in `thread_id`.
    pub(super) fn open_steer_inbox(&self, thread_id: &str) -> SteerInbox {
        let inbox = SteerInbox::default();
        self.inner
            .steer_inboxes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(thread_id.to_owned(), Arc::clone(&inbox));
        inbox
    }

    /// Closes the run's steer inbox; steers it never read go back to the
    /// front of the queue, in order, to be sent as the next message.
    pub(super) fn close_steer_inbox(&self, thread_id: &str) {
        let Some(inbox) = self
            .inner
            .steer_inboxes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(thread_id)
        else {
            return;
        };
        let unread: Vec<String> = inbox
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .drain(..)
            .collect();
        if unread.is_empty() {
            return;
        }
        let _guard = self
            .inner
            .queue_lock
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut queue = self.queue_for(thread_id);
        for text in unread.into_iter().rev() {
            queue.insert(
                0,
                QueuedMessage {
                    id: uuid::Uuid::new_v4().to_string(),
                    text,
                    attachments: Vec::new(),
                    reasoning_effort: None,
                    held: false,
                },
            );
        }
        self.save_queue(thread_id, &queue);
    }

    /// Runs [`AppState::after_run`] on its own task. A plain function (not an
    /// `async` one) keeps the run → queue → run cycle out of the futures'
    /// types, so each stays `Send`.
    pub(super) fn spawn_after_run(
        &self,
        sink: Arc<dyn EventSink>,
        thread_id: String,
        run_id: String,
        terminal: ThreadEvent,
    ) {
        let state = self.clone();
        let task: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> =
            Box::pin(async move { state.after_run(sink, thread_id, run_id, terminal).await });
        tokio::spawn(task);
    }

    /// Runs [`AppState::dispatch_queue`] on its own task (see [`Self::spawn_after_run`]).
    pub(super) fn spawn_dispatch_queue(&self, sink: Arc<dyn EventSink>, thread_id: String) {
        let state = self.clone();
        let task: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> =
            Box::pin(async move {
                state.dispatch_queue(sink, &thread_id).await;
            });
        tokio::spawn(task);
    }

    /// What happens when a chat's run ends: the next queued message goes out;
    /// with nothing queued, a running goal continues.
    pub(super) async fn after_run(
        &self,
        sink: Arc<dyn EventSink>,
        thread_id: String,
        run_id: String,
        terminal: ThreadEvent,
    ) {
        if self.dispatch_queue(Arc::clone(&sink), &thread_id).await {
            // The goal learns about the turn it was waiting on later.
            self.goal_turn_interrupted(&run_id);
            return;
        }
        self.goal_after_run(sink, thread_id, run_id, terminal).await;
    }

    /// Sends the next waiting message if the chat is idle. Returns whether
    /// a message was sent (or will be, after the concurrency limit frees up).
    pub(super) async fn dispatch_queue(&self, sink: Arc<dyn EventSink>, thread_id: &str) -> bool {
        if self.require_thread(thread_id).await.unwrap_or(true) {
            return false;
        }
        let next = {
            let _guard = self
                .inner
                .queue_lock
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let mut queue = self.queue_for(thread_id);
            let Some(index) = queue.iter().position(|message| !message.held) else {
                return false;
            };
            let next = queue.remove(index);
            self.save_queue(thread_id, &queue);
            self.announce_queue(sink.as_ref(), thread_id, queue);
            next
        };
        sink.emit_thread_event(&ThreadEventEnvelope {
            thread_id: thread_id.to_owned(),
            run_id: String::new(),
            event: ThreadEvent::UserMessage {
                text: next.text.clone(),
                attachments: next.attachments.clone(),
            },
        });
        let sent = Box::pin(self.send_message_with_options(
            Arc::clone(&sink),
            thread_id.to_owned(),
            next.text.clone(),
            uuid::Uuid::new_v4().to_string(),
            next.reasoning_effort.clone(),
            next.attachments.clone(),
        ))
        .await;
        match sent {
            Ok(_) => true,
            Err(error) if error.contains("concurrency limit") || error.contains("is busy") => {
                // Not this message's fault: it goes back to the front and waits.
                self.requeue_front(sink.as_ref(), thread_id, next);
                if error.contains("concurrency limit") {
                    self.retry_queue_later(sink, thread_id.to_owned());
                }
                true
            }
            // A send that failed for its own reason was recorded as a failed
            // run; its end already moved the queue along.
            Err(_) => true,
        }
    }

    fn requeue_front(&self, sink: &dyn EventSink, thread_id: &str, message: QueuedMessage) {
        let _guard = self
            .inner
            .queue_lock
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut queue = self.queue_for(thread_id);
        queue.insert(0, message);
        self.save_queue(thread_id, &queue);
        self.announce_queue(sink, thread_id, queue);
    }

    /// Tries the queue again once the concurrency limit may have freed up.
    fn retry_queue_later(&self, sink: Arc<dyn EventSink>, thread_id: String) {
        let state = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(BUSY_RETRY_SECS)).await;
            state.spawn_dispatch_queue(sink, thread_id);
        });
    }

    /// Removes the queues of deleted chats.
    pub(super) fn forget_queues(&self, thread_ids: &[String]) {
        let mut queues = self.inner.queues.lock().unwrap_or_else(|e| e.into_inner());
        for id in thread_ids {
            queues.remove(id);
            if let Some(path) = self.queue_path(id) {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}
