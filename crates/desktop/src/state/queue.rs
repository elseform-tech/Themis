//! Per-chat prompt queue and steering.
//!
//! A message sent while a chat's run is going waits in the chat's queue
//! (`queues/<chat id>.json` beside `threads.json`) and goes out by itself,
//! oldest first, when the run ends. Steering moves a waiting message into the
//! running run instead: it shows in the chat at once, approvals the run waits on
//! are denied so nothing holds it up, and the run reads it before its next model
//! request. A steer the run never picked up leaves the chat and returns to the
//! front of the queue.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::json;
use themis_core::runtime::SteerInbox;
use themis_core::tools::Approval;

use super::chat_grants::safe_chat_id;
use super::{AppState, SteerEntry};
use crate::approvals::SteerWaiting;
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

/// How far a message the server sends itself got.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ServerSend {
    /// Not yet saved to the chat.
    Pending,
    /// Saved (and shown) as the user's message; a later failure must not
    /// resend it.
    Saved,
    /// Admitted and failed to start: its end step continues the queue/goal.
    SpawnFailed,
}

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
    pub(super) fn edit_queue<T>(
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

    /// Whether the chat runs now; an error for an unknown chat.
    pub(super) async fn require_thread(&self, thread_id: &str) -> Result<bool, String> {
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
            note: None,
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
                message.note = None;
            }
            if let Some(held) = held {
                message.held = held;
                if !held {
                    // Saving (even unchanged) means "try again".
                    message.note = None;
                }
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

    /// Delivers a waiting message into the chat's running run: it shows in the
    /// chat now, and any approval the run waits on is denied so the run can read
    /// it. Saved grants and rules stay as they are.
    pub async fn steer_queued_message(
        &self,
        sink: Arc<dyn EventSink>,
        thread_id: String,
        message_id: String,
    ) -> Result<(), String> {
        self.require_thread(&thread_id).await?;
        // Under the queue lock: closing the inbox takes the same lock, so the
        // message lands either in a live inbox or back in the queue, never in
        // an inbox that was already closed.
        let steered = self.edit_queue(sink.as_ref(), &thread_id, |queue| {
            let index = queue
                .iter()
                .position(|message| message.id == message_id)
                .ok_or_else(|| "That message was already sent or removed".to_owned())?;
            if !queue[index].attachments.is_empty() {
                return Err("Messages with attachments wait for the next turn".to_owned());
            }
            let inboxes = self
                .inner
                .steer_inboxes
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let entry = inboxes
                .get(&thread_id)
                .ok_or_else(|| "No run is going; send the message instead".to_owned())?;
            let text = queue.remove(index).text;
            // Before the inbox: the run may read it at once, and the chat must
            // hear of it in this order.
            self.record_steer_event(
                sink.as_ref(),
                &thread_id,
                &entry.run_id,
                ThreadEvent::SteerShown { text: text.clone() },
            );
            entry
                .inbox
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push_back(text.clone());
            Ok(text)
        })?;
        // After the inbox: an approval asked from here on sees the message there
        // and is denied without a box (see `DesktopApprovalHook`).
        let denied = self.deny_approvals_for_new_message(&thread_id);
        self.note_added_request(sink.as_ref(), &thread_id, &steered);
        let _ = self.inner.diagnostics.append(
            "queue",
            "steered",
            "info",
            json!({"thread_id": thread_id, "approvals_denied": denied}),
        );
        Ok(())
    }

    /// Asks whether the chat's current run has a steered message still unread.
    /// Answers to questions share the inbox but are not steers: they leave
    /// approvals alone.
    pub(super) fn steer_waiting_check(&self, thread_id: &str) -> SteerWaiting {
        let inbox = self
            .inner
            .steer_inboxes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(thread_id)
            .map(|entry| Arc::clone(&entry.inbox))
            .unwrap_or_default();
        Arc::new(move || {
            inbox
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .any(|text| !text.starts_with(super::questions::ANSWER_MARKER))
        })
    }

    /// Denies every approval the chat waits on, because the person sent a new
    /// message. Each waiting hook takes its box away. Returns how many.
    fn deny_approvals_for_new_message(&self, thread_id: &str) -> usize {
        let mut denied = 0;
        self.inner
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|_, approval| {
                if approval.thread_id != thread_id {
                    return true;
                }
                let _ = approval.sender.send(Approval::DenyForNewMessage);
                denied += 1;
                false
            });
        denied
    }

    /// Saves a steer's display event in the chat's history and sends it to clients.
    fn record_steer_event(
        &self,
        sink: &dyn EventSink,
        thread_id: &str,
        run_id: &str,
        event: ThreadEvent,
    ) {
        let envelope = ThreadEventEnvelope {
            thread_id: thread_id.to_owned(),
            run_id: run_id.to_owned(),
            event,
        };
        if let Err(error) = self.inner.transcript.append_event(&envelope) {
            self.record_error("save_thread_event", error);
        }
        sink.emit_thread_event(&envelope);
    }

    /// A fresh steer inbox for the run `run_id` starting in `thread_id`.
    pub(super) fn open_steer_inbox(
        &self,
        thread_id: &str,
        run_id: &str,
        sink: Arc<dyn EventSink>,
    ) -> SteerInbox {
        let inbox = SteerInbox::default();
        self.inner
            .steer_inboxes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                thread_id.to_owned(),
                SteerEntry {
                    inbox: Arc::clone(&inbox),
                    sink,
                    run_id: run_id.to_owned(),
                },
            );
        inbox
    }

    /// Closes the run's steer inbox; steers it never read go back to the
    /// front of the queue, in order, to be sent as the next message.
    pub(super) fn close_steer_inbox(&self, thread_id: &str) {
        // Same lock order as steering: queue, then inboxes.
        let _guard = self
            .inner
            .queue_lock
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let Some(SteerEntry {
            inbox,
            sink,
            run_id,
        }) = self
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
        let mut queue = self.queue_for(thread_id);
        let unread_count = unread.len();
        for text in unread.into_iter().rev() {
            // Answers were never shown as steers; a steer leaves the chat for now.
            if !text.starts_with(super::questions::ANSWER_MARKER) {
                self.record_steer_event(
                    sink.as_ref(),
                    thread_id,
                    &run_id,
                    ThreadEvent::SteerReturned { text: text.clone() },
                );
            }
            queue.insert(
                0,
                QueuedMessage {
                    id: uuid::Uuid::new_v4().to_string(),
                    text,
                    attachments: Vec::new(),
                    reasoning_effort: None,
                    held: false,
                    note: None,
                },
            );
        }
        self.save_queue(thread_id, &queue);
        let _ = self.inner.diagnostics.append(
            "queue",
            "steer_requeued",
            "info",
            json!({"thread_id": thread_id, "count": unread_count}),
        );
        self.announce_queue(sink.as_ref(), thread_id, queue);
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

    /// Sends a message on the server's own behalf (a queued message or a
    /// goal turn). Returns the send result and how far the message got: a
    /// saved message is already in the chat, and a failed start is handled by
    /// its run's end step.
    pub(super) async fn send_from_server(
        &self,
        sink: Arc<dyn EventSink>,
        thread_id: &str,
        text: String,
        run_id: &str,
        reasoning_effort: Option<String>,
        attachments: Vec<String>,
    ) -> (Result<(), String>, ServerSend) {
        self.inner
            .server_sends
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(run_id.to_owned(), ServerSend::Pending);
        let sent = Box::pin(self.send_message_with_options(
            sink,
            thread_id.to_owned(),
            text,
            run_id.to_owned(),
            reasoning_effort,
            attachments,
        ))
        .await;
        let state = self
            .inner
            .server_sends
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(run_id)
            .unwrap_or(ServerSend::Pending);
        (sent.map(|_| ()), state)
    }

    /// The effort a queued message was queued with, if the chat's current
    /// model still offers it; otherwise the chat's own effort applies.
    async fn current_effort(&self, thread_id: &str, effort: Option<String>) -> Option<String> {
        let effort = effort?;
        let model = self
            .inner
            .threads
            .read()
            .await
            .get(thread_id)
            .map(|record| record.model.clone())?;
        self.effort_offered(&model, &effort).await.then_some(effort)
    }

    /// Sends the next waiting message if the chat is idle. Returns whether
    /// a message was sent (or will be, after the concurrency limit frees up).
    /// A message that cannot be sent stays in the queue, held, with the reason
    /// on it, and the next one is tried; when the next fails the same way, the
    /// cause is the chat's, so the rest keep waiting (one note, not one each).
    pub(super) async fn dispatch_queue(&self, sink: Arc<dyn EventSink>, thread_id: &str) -> bool {
        let mut refused: Option<String> = None;
        loop {
            if self.require_thread(thread_id).await.unwrap_or(true) {
                return false;
            }
            let (index, next) = {
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
                (index, next)
            };
            let run_id = uuid::Uuid::new_v4().to_string();
            let effort = self
                .current_effort(thread_id, next.reasoning_effort.clone())
                .await;
            let (sent, progress) = self
                .send_from_server(
                    Arc::clone(&sink),
                    thread_id,
                    next.text.clone(),
                    &run_id,
                    effort,
                    next.attachments.clone(),
                )
                .await;
            let Err(error) = sent else {
                return true;
            };
            // A chat deleted meanwhile keeps nothing: no queue file, no note.
            if self.require_thread(thread_id).await.is_err() {
                return false;
            }
            match progress {
                // Admitted and failed to start: its end step moves the queue on.
                ServerSend::SpawnFailed => return true,
                // Saved as the user's message, then failed: it is in the chat
                // already, so it is not queued again; the chat says why.
                ServerSend::Saved => {
                    self.note_queue_failure(
                        sink.as_ref(),
                        thread_id,
                        &run_id,
                        &format!("This message could not start: {error}"),
                    );
                }
                ServerSend::Pending if super::threads::send_must_wait(&error) => {
                    // Not this message's fault: it goes back and waits.
                    self.requeue_at(sink.as_ref(), thread_id, index, next);
                    if error.contains(super::threads::CONCURRENCY_LIMIT) {
                        self.retry_queue_later(sink, thread_id.to_owned());
                    }
                    return true;
                }
                ServerSend::Pending if refused.as_deref() == Some(error.as_str()) => {
                    // Refused the same way twice: the chat, not the message.
                    self.requeue_at(sink.as_ref(), thread_id, index, next);
                    return false;
                }
                ServerSend::Pending => {
                    let mut held = next;
                    held.held = true;
                    held.note = Some(themis_core::goal::one_line(&error, 300));
                    self.requeue_at(sink.as_ref(), thread_id, index, held);
                    self.note_queue_failure(
                        sink.as_ref(),
                        thread_id,
                        &uuid::Uuid::new_v4().to_string(),
                        &format!(
                            "Queued message not sent: {error}. It stays in the queue; edit it to send it again."
                        ),
                    );
                    refused = Some(error);
                }
            }
        }
    }

    /// Records why a queued message did not go out, in the chat and diagnostics.
    fn note_queue_failure(&self, sink: &dyn EventSink, thread_id: &str, run_id: &str, error: &str) {
        let failed = ThreadEventEnvelope {
            thread_id: thread_id.to_owned(),
            run_id: run_id.to_owned(),
            event: ThreadEvent::Failed {
                error: error.to_owned(),
            },
        };
        if let Err(save) = self.inner.transcript.append_event(&failed) {
            self.record_error("save_thread_event", save);
        }
        sink.emit_thread_event(&failed);
        let _ = self.inner.diagnostics.append(
            "queue",
            "unsendable",
            "warn",
            json!({"thread_id": thread_id}),
        );
    }

    /// Puts a message back where it was taken from, keeping the queue order.
    fn requeue_at(
        &self,
        sink: &dyn EventSink,
        thread_id: &str,
        index: usize,
        message: QueuedMessage,
    ) {
        let _guard = self
            .inner
            .queue_lock
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut queue = self.queue_for(thread_id);
        queue.insert(index.min(queue.len()), message);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::approvals::DesktopApprovalHook;
    use crate::sink::ChannelSink;
    use themis_core::tools::{ApprovalHook, RiskLevel, ToolAction};

    /// Asks on its own thread, as a run does, and returns the decision.
    fn ask(state: &AppState, thread: &str) -> std::thread::JoinHandle<Approval> {
        let hook = DesktopApprovalHook::new(
            thread.to_owned(),
            true,
            Arc::new(ChannelSink::channel().0),
            Arc::clone(&state.inner.pending),
        )
        .with_timeout(std::time::Duration::from_millis(1_500));
        std::thread::spawn(move || {
            hook.approve(&ToolAction {
                display: None,
                command: Vec::new(),
                tool: "write_file".to_owned(),
                summary: "write".to_owned(),
                risk: RiskLevel::Write,
            })
        })
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_steer_denies_every_approval_waiting_in_its_chat_and_no_other() {
        let data = tempfile::tempdir().unwrap();
        let state = AppState::new_for_test(data.path().join("settings.json"));
        let mine = [ask(&state, "chat"), ask(&state, "chat")];
        let other = ask(&state, "other chat");
        while state.pending_approvals().len() < 3 {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        assert_eq!(state.deny_approvals_for_new_message("chat"), 2);
        for waiting in mine {
            assert_eq!(waiting.join().unwrap(), Approval::DenyForNewMessage);
        }
        let left = state.pending_approvals();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].thread_id, "other chat");
        // The other chat's box runs its own course.
        assert_eq!(other.join().unwrap(), Approval::Deny);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn only_a_steer_counts_as_waiting_not_an_answer() {
        let data = tempfile::tempdir().unwrap();
        let state = AppState::new_for_test(data.path().join("settings.json"));
        let inbox = state.open_steer_inbox("chat", "run", Arc::new(ChannelSink::channel().0));
        let waiting = state.steer_waiting_check("chat");
        assert!(!waiting());
        inbox.lock().unwrap().push_back(format!(
            "{}\nAnswer",
            super::super::questions::ANSWER_MARKER
        ));
        assert!(!waiting());
        inbox.lock().unwrap().push_back("Use tabs".to_owned());
        assert!(waiting());
        inbox.lock().unwrap().clear();
        assert!(!waiting());
    }
}
