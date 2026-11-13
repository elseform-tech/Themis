//! What a run carries with it: notes, steering, end requests, the system head, memory and configuration for this task.

use super::*;

/// Notes delivered to a running agent between steps by something it started and that ends
/// later (subagent results). Unlike steered messages they are not the person's words.
#[derive(Default)]
pub struct RunNotes {
    queue: Mutex<std::collections::VecDeque<String>>,
    pending: std::sync::atomic::AtomicUsize,
    arrived: tokio::sync::Notify,
}

impl RunNotes {
    pub fn new() -> Arc<Self> {
        Arc::default()
    }
    /// `count` more notes will arrive; the run waits for them before it ends.
    pub fn expect(&self, count: usize) {
        self.pending.fetch_add(count, Ordering::SeqCst);
    }
    /// Delivers one expected note.
    pub fn deliver(&self, note: String) {
        self.queue
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_back(note);
        // Keep Rust 1.98 support; the try_update replacement requires Rust 1.99.
        #[allow(deprecated)]
        let _ = self
            .pending
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1));
        self.arrived.notify_waiters();
    }
    /// Takes back a delivered note the run has not read yet; false once the model saw it.
    pub fn withdraw(&self, note: &str) -> bool {
        let mut queue = self.queue.lock().unwrap_or_else(|e| e.into_inner());
        match queue.iter().position(|queued| queued == note) {
            Some(index) => {
                queue.remove(index);
                true
            }
            None => false,
        }
    }
    /// Notes still expected.
    pub fn pending(&self) -> usize {
        self.pending.load(Ordering::SeqCst)
    }
    pub(super) fn waiting(&self) -> bool {
        !self
            .queue
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_empty()
    }
    pub(super) fn take(&self) -> Vec<String> {
        self.queue
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .drain(..)
            .collect()
    }
}

/// Starts a run note that carries memory changed mid-run, not a subagent's report.
pub const MEMORY_NOTE_MARKER: &str = "[memory-note]";

/// Runs `future` with `notes` as the run's note channel.
pub async fn with_run_notes<F: std::future::Future>(notes: Arc<RunNotes>, future: F) -> F::Output {
    RUN_NOTES.scope(notes, future).await
}

pub(super) fn run_notes() -> Option<Arc<RunNotes>> {
    RUN_NOTES.try_with(Arc::clone).ok()
}

/// Waits until a note arrives or the run is stopped.
pub(super) async fn wait_for_note(notes: &RunNotes, stopped: &AtomicBool) {
    while !notes.waiting() && !stopped.load(Ordering::SeqCst) {
        let arrived = notes.arrived.notified();
        if notes.waiting() {
            break;
        }
        let _ = tokio::time::timeout(std::time::Duration::from_millis(200), arrived).await;
    }
}

/// Whether this run is a subagent's: only they replace the system instructions.
pub(super) fn is_subagent_run() -> bool {
    SYSTEM_HEAD.try_with(|_| ()).is_ok()
}

/// Runs `future` with `head` in place of the ordinary system instructions (subagent runs).
pub async fn with_system_head<F: std::future::Future>(head: String, future: F) -> F::Output {
    SYSTEM_HEAD.scope(head, future).await
}

/// Runs `future` knowing the memory block its system message carries (the index and the
/// relevant memories), so the context meter can show it as its own part.
pub async fn with_memory_block<F: std::future::Future>(block: String, future: F) -> F::Output {
    MEMORY_BLOCK.scope(block, future).await
}

/// Gives the note read back right after a context checkpoint, such as the chat's to-do list as
/// it stands then; `None` adds nothing.
pub type CheckpointNote = Arc<dyn Fn() -> Option<String> + Send + Sync>;

/// Runs `future` with `note` added after each context checkpoint it installs.
pub async fn with_checkpoint_note<F: std::future::Future>(
    note: CheckpointNote,
    future: F,
) -> F::Output {
    CHECKPOINT_NOTE.scope(note, future).await
}

/// Runs `future` with the chat's first user message, which every context checkpoint
/// request then carries verbatim (bounded), so its rules never depend on earlier summaries.
pub async fn with_first_request<F: std::future::Future>(first: String, future: F) -> F::Output {
    FIRST_REQUEST.scope(first, future).await
}

/// The checkpoint request's first-request section, unless there is no first message or
/// the summarized records already hold it verbatim.
pub(super) fn first_request_section(older: &[ChatMessage]) -> Option<String> {
    let first = FIRST_REQUEST.try_with(Clone::clone).ok()?;
    if first.trim().is_empty()
        || older
            .iter()
            .any(|message| message.role == ChatRole::User && message.content == first)
    {
        return None;
    }
    Some(crate::prompts::fill(
        crate::prompts::SUMMARY_FIRST_REQUEST,
        &[("first_request", &bounded_context_hint(&first))],
    ))
}

/// A message steered into a running run, and who wrote it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Steer {
    /// The text, as written.
    pub text: String,
    /// `None` for the person (and what the app adds for them); otherwise the agent acting for
    /// them that wrote it (a Vision's id), whose words are not the person's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
}

impl From<String> for Steer {
    /// The person's message.
    fn from(text: String) -> Self {
        Self { text, author: None }
    }
}

impl From<&str> for Steer {
    fn from(text: &str) -> Self {
        Self::from(text.to_owned())
    }
}

/// Messages steered into a running run, oldest first. The caller pushes; the run takes them
/// before its next model request.
pub type SteerInbox = Arc<Mutex<std::collections::VecDeque<Steer>>>;

/// Runs `future` with a steer inbox; without one, steering is unavailable.
pub async fn with_steer_inbox<F: std::future::Future>(inbox: SteerInbox, future: F) -> F::Output {
    STEER_INBOX.scope(inbox, future).await
}

/// Set by a tool that needs the user before anything more can happen (a
/// blocking question): the run ends after that tool batch, with this text as
/// its answer, instead of asking the model again.
pub type EndRequest = Arc<Mutex<Option<String>>>;

/// Runs `future` with an end request a tool can set; without one, nothing ends a run early.
pub async fn with_end_request<F: std::future::Future>(end: EndRequest, future: F) -> F::Output {
    END_REQUEST.scope(end, future).await
}

pub(super) fn take_end_request() -> Option<String> {
    END_REQUEST
        .try_with(|end| end.lock().unwrap_or_else(|e| e.into_inner()).take())
        .ok()
        .flatten()
}

/// Takes every steered message waiting for this run.
pub(super) fn take_steers() -> Vec<Steer> {
    STEER_INBOX
        .try_with(|inbox| {
            inbox
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .drain(..)
                .collect()
        })
        .unwrap_or_default()
}

/// Whether a steered message is waiting for this run.
pub(super) fn steer_waiting() -> bool {
    STEER_INBOX
        .try_with(|inbox| !inbox.lock().unwrap_or_else(|e| e.into_inner()).is_empty())
        .unwrap_or(false)
}

/// The model-facing form of a steered message: the user's words, marked as
/// arriving mid-run so the model adjusts the work it is doing.
pub fn steered_message(text: &str) -> String {
    format!("{}\n{text}", crate::prompts::RUNTIME_STEERED)
}

/// The check before a run that read several steered messages ends: each
/// message numbered, on one line each.
pub(super) fn steers_check(steered: &[String]) -> String {
    let listed = steered
        .iter()
        .enumerate()
        .map(|(index, text)| format!("{}. {}", index + 1, text.replace('\n', " ")))
        .collect::<Vec<_>>()
        .join("\n");
    crate::prompts::fill(
        crate::prompts::RUNTIME_STEERS_CHECK,
        &[("messages", &listed)],
    )
}

/// Whether an eval switched this runtime nudge off (never, outside evals).
pub(super) fn nudge_off(name: &str) -> bool {
    #[cfg(feature = "eval")]
    {
        EVAL_NUDGES_OFF
            .try_with(|off| off.iter().any(|item| item == name))
            .unwrap_or(false)
    }
    #[cfg(not(feature = "eval"))]
    {
        let _ = name;
        false
    }
}

/// Marks the run's first request as a short reply so semantic compaction may wait while the
/// request still fits the hard input budget. Maintenance and overflow recovery are unaffected.
pub async fn with_short_reply_turn<F: std::future::Future>(short: bool, future: F) -> F::Output {
    SHORT_REPLY_TURN.scope(short, future).await
}

/// Capture validated compaction policy for a run, without changing global defaults.
pub async fn with_configuration<F: std::future::Future>(
    config: crate::configuration::CompactionConfig,
    future: F,
) -> F::Output {
    COMPACTION_CONFIG.scope(config, future).await
}

pub(crate) fn compaction_config() -> crate::configuration::CompactionConfig {
    COMPACTION_CONFIG.try_with(Clone::clone).unwrap_or_default()
}
