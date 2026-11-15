//! Waking a Vision: a run in its Vision chat for a channel (see `runs`).
//!
//! Wakes are bounded like hook wakes (`state/hook_wakes.rs`): one Vision run at a time per
//! Vision chat, reasons that arrive meanwhile wait in a per-Vision queue and go together into
//! its next wake, at most `visions.max_wakes` runs in a row until the person writes in the
//! channel, none while the Vision is paused or Visions are off, none while it is over its daily
//! budget (the reason waits for a wake after the budget is raised; see `budget`), and none for a
//! chat the person stopped (except the one `ChatStopped` that tells the Vision so) or took over.
//! Each decision is logged as `visions/wake {vision_id, channel_id, reason, outcome}`, metadata
//! only.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::json;

use super::{ChannelEntry, Hold, NoticeKind, OwnedKind, Owner};
use crate::sink::EventSink;
use crate::state::AppState;
use crate::types::{GoalStatus, ThreadEvent};

/// Wake outcomes after which no run will take a question: it goes to the person instead.
/// Not `disabled`: with Visions off nothing in their channels moves, and the question stays in
/// its work chat, as with no Visions at all.
const STRANDS: &[&str] = &[
    "paused",
    "limit",
    "over_budget",
    "budget_unknown",
    "gone",
    "configuration",
];

/// Most reasons one Vision keeps waiting; the oldest go first beyond it.
const MAX_QUEUED: usize = 64;

/// Why a Vision wakes. Chat-related reasons carry the owned chat's id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WakeReason {
    /// The person mentioned it in its channel (or wrote where it is the only member).
    Mentioned,
    ChatFinished(String),
    ChatFailed(String),
    ChatStopped(String),
    /// The chat and the question's id.
    QuestionAsked(String, String),
    HandedBack(String),
    /// The chat waits on the person's approval or permission question (its id): the Vision tells
    /// the person, never answers it.
    ApprovalWaiting(String, String),
    /// The budget check (not the person) stopped the chat: `reached` (today's budget ran out)
    /// or `unknown` (spend could not be checked). It waits for a wake the budget allows.
    BudgetStopped(String, &'static str),
    /// Its chief wrote to it (a task, a redirect): the seq in its team log.
    Tasked(u64),
    /// One of its workers wrote to it (a report, a pushback): the worker and the seq in its log.
    WorkerNote(String, u64),
    /// Another channel's chief asked it for something (the request id).
    Request(String),
    /// The channel it asked answered (the request id).
    RequestAnswered(String),
    /// One of its routines is due (its id and instruction).
    Routine(String, String),
    /// A worker it tasked ended that turn without acting (the worker).
    WorkerSilent(String),
}

impl WakeReason {
    /// The reason's name in logs and in the wake message.
    pub(super) fn label(&self) -> &'static str {
        match self {
            WakeReason::Mentioned => "mentioned",
            WakeReason::ChatFinished(_) => "chat_finished",
            WakeReason::ChatFailed(_) => "chat_failed",
            WakeReason::ChatStopped(_) => "chat_stopped",
            WakeReason::QuestionAsked(..) => "question_asked",
            WakeReason::HandedBack(_) => "handed_back",
            WakeReason::ApprovalWaiting(..) => "approval_waiting",
            WakeReason::BudgetStopped(..) => "budget_stopped",
            WakeReason::Tasked(_) => "tasked",
            WakeReason::WorkerNote(..) => "worker_note",
            WakeReason::Request(_) => "request",
            WakeReason::RequestAnswered(_) => "request_answered",
            WakeReason::Routine(..) => "routine",
            WakeReason::WorkerSilent(_) => "worker_silent",
        }
    }

    /// The owned chat it is about, if any.
    fn thread(&self) -> Option<&str> {
        match self {
            WakeReason::Mentioned
            | WakeReason::Tasked(_)
            | WakeReason::WorkerNote(..)
            | WakeReason::Request(_)
            | WakeReason::RequestAnswered(_)
            | WakeReason::Routine(..)
            | WakeReason::WorkerSilent(_) => None,
            WakeReason::ChatFinished(thread)
            | WakeReason::ChatFailed(thread)
            | WakeReason::ChatStopped(thread)
            | WakeReason::QuestionAsked(thread, _)
            | WakeReason::ApprovalWaiting(thread, _)
            | WakeReason::BudgetStopped(thread, _)
            | WakeReason::HandedBack(thread) => Some(thread),
        }
    }

    /// One line of the wake message: the label, then the chat's and the question's ids.
    pub(super) fn line(&self) -> String {
        match self {
            WakeReason::Mentioned => self.label().to_owned(),
            WakeReason::QuestionAsked(thread, id) | WakeReason::ApprovalWaiting(thread, id) => {
                format!("{} {thread} {id}", self.label())
            }
            WakeReason::BudgetStopped(thread, cause) => {
                format!("{} {thread} {cause}", self.label())
            }
            WakeReason::Tasked(seq) => format!("{} {seq}", self.label()),
            WakeReason::WorkerNote(worker, seq) => format!("{} {worker} {seq}", self.label()),
            WakeReason::Request(id)
            | WakeReason::RequestAnswered(id)
            | WakeReason::WorkerSilent(id) => {
                format!("{} {id}", self.label())
            }
            WakeReason::Routine(id, instruction) => format!("{} {id}: {instruction}", self.label()),
            other => format!("{} {}", other.label(), other.thread().unwrap_or_default()),
        }
    }
}

/// One Vision's wakes in one channel.
#[derive(Debug, Default)]
struct WakeSlot {
    /// A run of its Vision chat is starting or running.
    running: bool,
    /// When the newest run started (Unix ms).
    started_ms: Option<i64>,
    /// Reasons waiting for its next run, oldest first.
    queued: Vec<WakeReason>,
    /// A run was refused as busy before it started: try again when a run ends.
    retry: bool,
}

/// What a wake may do now.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Admit {
    /// Start a run with these reasons (counted).
    Start(Vec<WakeReason>),
    /// A run is going: the reason waits for the next one.
    Queued,
    /// It woke `visions.max_wakes` times in a row: the reasons wait until the person writes.
    Limit,
    /// Nothing waits.
    Idle,
}

/// Most approval ids remembered as already told to a Vision.
const MAX_APPROVALS_SEEN: usize = 256;

/// Every Vision's wakes, by (Vision id, channel id). In memory: a restart starts afresh.
#[derive(Debug, Default)]
pub(crate) struct WakeSlots {
    slots: HashMap<(String, String), WakeSlot>,
    /// Runs started in each channel since the person last wrote there, every member's
    /// together, and whether the channel was told it reached `visions.max_wakes`.
    channels: HashMap<String, (u32, bool)>,
    /// Approvals and permission questions a Vision was already woken for, newest last.
    approvals_seen: std::collections::VecDeque<String>,
}

impl WakeSlots {
    /// Queues `reason` (once) and decides, in the same step, whether a run starts now.
    pub(super) fn admit(
        &mut self,
        vision_id: &str,
        channel_id: &str,
        reason: Option<WakeReason>,
        limit: u32,
    ) -> Admit {
        let slot = self.queue_reason(vision_id, channel_id, reason);
        if slot.running {
            return Admit::Queued;
        }
        if slot.queued.is_empty() {
            return Admit::Idle;
        }
        let count = self.channels.entry(channel_id.to_owned()).or_default();
        if count.0 >= limit {
            return Admit::Limit;
        }
        count.0 += 1;
        let slot = self.queue_reason(vision_id, channel_id, None);
        slot.running = true;
        slot.started_ms = Some(chrono::Utc::now().timestamp_millis());
        slot.retry = false;
        Admit::Start(std::mem::take(&mut slot.queued))
    }

    /// Queues `reason` for the Vision's next run without starting anything.
    pub(super) fn wait(&mut self, vision_id: &str, channel_id: &str, reason: Option<WakeReason>) {
        self.queue_reason(vision_id, channel_id, reason);
    }

    /// Queues `reason` (once, the newest [`MAX_QUEUED`] kept) and returns the slot.
    fn queue_reason(
        &mut self,
        vision_id: &str,
        channel_id: &str,
        reason: Option<WakeReason>,
    ) -> &mut WakeSlot {
        let slot = self
            .slots
            .entry((vision_id.to_owned(), channel_id.to_owned()))
            .or_default();
        if let Some(reason) = reason {
            if !slot.queued.contains(&reason) {
                slot.queued.push(reason);
            }
            let over = slot.queued.len().saturating_sub(MAX_QUEUED);
            slot.queued.drain(..over);
        }
        slot
    }

    /// The run ended; returns when it started.
    pub(super) fn ended(&mut self, vision_id: &str, channel_id: &str) -> Option<i64> {
        let slot = self.slot(vision_id, channel_id)?;
        slot.running = false;
        slot.started_ms.take()
    }

    /// The Visions with reasons waiting in `channel_id` and no run going.
    pub(super) fn waiting_in(&self, channel_id: &str) -> Vec<String> {
        self.slots
            .iter()
            .filter(|((_, channel), slot)| {
                channel == channel_id && !slot.running && !slot.queued.is_empty()
            })
            .map(|((vision, _), _)| vision.clone())
            .collect()
    }

    /// The run was refused before it started: its reasons wait again, first. It still counts
    /// against `visions.max_wakes`, so retries stay bounded. Only a busy refusal (`retry`) is
    /// tried again when a run ends; any other waits for the next wake.
    pub(super) fn refused(
        &mut self,
        vision_id: &str,
        channel_id: &str,
        reasons: Vec<WakeReason>,
        retry: bool,
    ) {
        if let Some(slot) = self.slot(vision_id, channel_id) {
            slot.running = false;
            slot.started_ms = None;
            slot.retry = retry;
            let later = std::mem::replace(&mut slot.queued, reasons);
            for reason in later {
                if !slot.queued.contains(&reason) {
                    slot.queued.push(reason);
                }
            }
        }
    }

    /// The Visions whose refused run may be tried again now (a run ended), once each.
    pub(super) fn due_retries(&mut self) -> Vec<(String, String)> {
        let mut due = Vec::new();
        for (key, slot) in &mut self.slots {
            if slot.retry && !slot.running {
                slot.retry = false;
                due.push(key.clone());
            }
        }
        due
    }

    /// The Visions with reasons waiting and no run going.
    pub(super) fn waiting(&self) -> Vec<(String, String)> {
        self.slots
            .iter()
            .filter(|(_, slot)| !slot.running && !slot.queued.is_empty())
            .map(|(key, _)| key.clone())
            .collect()
    }

    /// Whether approval `id` is new to the Visions (it is then remembered).
    pub(super) fn first_approval_notice(&mut self, id: &str) -> bool {
        if self.approvals_seen.iter().any(|seen| seen == id) {
            return false;
        }
        self.approvals_seen.push_back(id.to_owned());
        if self.approvals_seen.len() > MAX_APPROVALS_SEEN {
            self.approvals_seen.pop_front();
        }
        true
    }

    /// The person wrote in the channel: its team may wake again.
    pub(super) fn person_wrote(&mut self, channel_id: &str) {
        self.channels.remove(channel_id);
    }

    /// Whether the channel is yet to be told it reached its limit (it is then remembered as
    /// told, until the person writes).
    pub(super) fn first_limit_notice(&mut self, channel_id: &str) -> bool {
        let count = self.channels.entry(channel_id.to_owned()).or_default();
        !std::mem::replace(&mut count.1, true)
    }

    fn slot(&mut self, vision_id: &str, channel_id: &str) -> Option<&mut WakeSlot> {
        self.slots
            .get_mut(&(vision_id.to_owned(), channel_id.to_owned()))
    }
}

/// Why a chat's event may not wake its Vision: the person stopped it (only the `ChatStopped`
/// that says so goes through) or took it over (only the hand-back goes through).
fn held_back(owner: &Owner, reason: &WakeReason) -> Option<&'static str> {
    match owner.hold {
        Hold::None => None,
        Hold::StoppedByPerson if matches!(reason, WakeReason::ChatStopped(_)) => None,
        Hold::StoppedByPerson => Some("held"),
        Hold::TakenOver if matches!(reason, WakeReason::HandedBack(_)) => None,
        Hold::TakenOver => Some("taken_over"),
    }
}

impl AppState {
    /// Wakes `vision_id` in `channel_id` for `reason`: starts a run of its Vision chat, or
    /// queues the reason for its next one (see the module notes for the bounds). Returns the
    /// outcome's name (`started` and `queued` mean a run will answer).
    pub fn visions_wake(
        &self,
        sink: &Arc<dyn EventSink>,
        channel_id: &str,
        vision_id: &str,
        reason: WakeReason,
    ) -> &'static str {
        let label = reason.label();
        let outcome = self.visions_wake_or_queue(sink, channel_id, vision_id, Some(reason));
        self.visions_note(
            "wake",
            json!({"vision_id": vision_id, "channel_id": channel_id, "reason": label, "outcome": outcome}),
        );
        outcome
    }

    /// The same with the app's own sink, for callers that hold none (the person's Stop, a
    /// question asked). Without a server nothing can run, and it is logged so.
    pub(crate) fn visions_wake_later(
        &self,
        channel_id: &str,
        vision_id: &str,
        reason: WakeReason,
    ) -> &'static str {
        match self.scheduler_sink() {
            Some(sink) => self.visions_wake(&sink, channel_id, vision_id, reason),
            None => {
                self.visions_note(
                    "wake",
                    json!({"vision_id": vision_id, "channel_id": channel_id, "reason": reason.label(), "outcome": "no_sink"}),
                );
                "no_sink"
            }
        }
    }

    /// A worker's turn ended: the questions of its work chats in this channel that were open
    /// when the turn began and are still neither answered nor passed on reach the person, under
    /// the chief (a permission question is the person's alone and is shown as one already).
    fn visions_pass_on_left_open(&self, chat: &Owner, started_ms: i64) {
        if self.leads(&chat.channel_id, &chat.vision_id) {
            return;
        }
        let work = self.owners_where(|owner| {
            owner.vision_id == chat.vision_id
                && owner.channel_id == chat.channel_id
                && owner.kind == OwnedKind::Work
        });
        for owner in work {
            for question in self.open_questions(&owner.thread_id) {
                let left =
                    question.asked_at <= started_ms && !question.passed_on && !question.permission;
                if left {
                    self.visions_pass_on_stranded(&owner, &question.id);
                }
            }
        }
    }

    /// The chief of the channel a chat belongs to while it can act (a member, not paused there),
    /// else the chat's owner, so what the chief would hear still wakes someone.
    pub(super) fn chief_of(&self, owner: &Owner) -> String {
        self.channel_get(&owner.channel_id)
            .map(|channel| channel.chief)
            .filter(|chief| !chief.is_empty() && !self.paused_in(&owner.channel_id, chief))
            .filter(|chief| self.works_or_leads(&owner.channel_id, chief))
            .unwrap_or_else(|| owner.vision_id.clone())
    }

    /// A worker woken with a task: its turn is watched for any act (see [`Self::visions_acted`]).
    pub(crate) fn visions_watch_silent(
        &self,
        channel_id: &str,
        vision_id: &str,
        reasons: &[WakeReason],
    ) {
        if !reasons
            .iter()
            .any(|reason| matches!(reason, WakeReason::Tasked(_)))
        {
            return;
        }
        let key = (channel_id.to_owned(), vision_id.to_owned());
        let mut cache = self.visions_cache();
        cache.acted.remove(&key);
        cache.watched.insert(key);
    }

    /// A Vision made a call that acts (anything but a read) in its running turn.
    pub(crate) fn visions_acted(&self, channel_id: &str, vision_id: &str, method: &str) {
        if method.starts_with("list_") || method.starts_with("get_") {
            return;
        }
        let key = (channel_id.to_owned(), vision_id.to_owned());
        self.visions_cache().acted.insert(key);
    }

    /// A watched worker's turn finished without an act: its chief is woken, so the task does
    /// not stall in silence.
    fn visions_silent_check(
        &self,
        sink: &Arc<dyn EventSink>,
        owner: &Owner,
        terminal: &ThreadEvent,
    ) {
        let key = (owner.channel_id.clone(), owner.vision_id.clone());
        let silent = {
            let mut cache = self.visions_cache();
            let acted = cache.acted.remove(&key);
            cache.watched.remove(&key) && !acted
        };
        let finished = matches!(
            terminal,
            ThreadEvent::Finished { .. } | ThreadEvent::Incomplete { .. }
        );
        if !silent || !finished || !self.is_worker(owner) {
            return;
        }
        let chief = self.chief_of(owner);
        if chief == owner.vision_id {
            return;
        }
        let reason = WakeReason::WorkerSilent(owner.vision_id.clone());
        self.visions_wake(sink, &owner.channel_id, &chief, reason);
    }

    /// Whether `owner`'s chat belongs to a worker of its channel (not to the chief).
    fn is_worker(&self, owner: &Owner) -> bool {
        self.channel_get(&owner.channel_id).is_some_and(|channel| {
            channel.chief != owner.vision_id && channel.members.contains(&owner.vision_id)
        })
    }

    fn works_or_leads(&self, channel_id: &str, vision_id: &str) -> bool {
        self.channel_get(channel_id)
            .is_some_and(|channel| channel.members.iter().any(|member| member == vision_id))
    }

    /// The channel's team reached `visions.max_wakes`: posted once, under its chief, until the
    /// person writes.
    fn visions_team_paused(&self, channel_id: &str) {
        if !self.visions_cache().wakes.first_limit_notice(channel_id) {
            return;
        }
        let chief = self
            .channel_get(channel_id)
            .map(|channel| channel.chief)
            .unwrap_or_default();
        let entry = ChannelEntry::Notice {
            kind: NoticeKind::TeamPaused,
            vision_id: (!chief.is_empty()).then(|| chief.clone()),
            thread_id: None,
            unpriced_requests: None,
            reason: None,
        };
        if self.channel_post(channel_id, &chief, entry).is_err() {
            self.visions_note("team_paused_unrecorded", json!({"channel_id": channel_id}));
        }
    }

    /// Starts the runs of the channel's members (but `woken`, just woken) whose reasons wait,
    /// still bounded and logged.
    pub(super) fn visions_wake_waiting_in(
        &self,
        sink: &Arc<dyn EventSink>,
        channel_id: &str,
        woken: &str,
    ) {
        let waiting = self.visions_cache().wakes.waiting_in(channel_id);
        for vision_id in waiting.into_iter().filter(|vision| vision != woken) {
            let outcome = self.visions_wake_or_queue(sink, channel_id, &vision_id, None);
            self.visions_note(
                "wake",
                json!({"vision_id": vision_id, "channel_id": channel_id, "reason": "waiting", "outcome": outcome}),
            );
        }
    }

    /// Admits the wake and starts the run when one may start; returns the outcome's name.
    fn visions_wake_or_queue(
        &self,
        sink: &Arc<dyn EventSink>,
        channel_id: &str,
        vision_id: &str,
        reason: Option<WakeReason>,
    ) -> &'static str {
        match self.visions_wake_admit(channel_id, vision_id, reason) {
            Ok(Admit::Start(reasons)) => {
                self.visions_spawn_run(sink, channel_id, vision_id, reasons);
                "started"
            }
            Ok(Admit::Queued) => "queued",
            Ok(Admit::Limit) => {
                self.visions_team_paused(channel_id);
                "limit"
            }
            Ok(Admit::Idle) => "idle",
            Err(refused) => refused,
        }
    }

    /// Whether quiet hours hold this wake now: they hold what agents start, and a drain of what
    /// waited (no reason), never the person's own doing (a post, a hand back, a stop) nor the
    /// work that follows it in that channel until quiet hours end. Routines always wait.
    fn visions_held_by_quiet(&self, channel_id: &str, reason: Option<&WakeReason>) -> bool {
        if !self.visions_quiet_at(self.visions_now()) {
            return false;
        }
        let persons = matches!(
            reason,
            Some(WakeReason::Mentioned | WakeReason::HandedBack(_) | WakeReason::ChatStopped(_))
        );
        let mut cache = self.visions_cache();
        if persons {
            cache.awake_in_quiet.insert(channel_id.to_owned());
            return false;
        }
        let routine = matches!(reason, Some(WakeReason::Routine(..)));
        routine || !cache.awake_in_quiet.contains(channel_id)
    }

    /// Whether the Vision may wake now, checked and counted under one lock; `Err` names why
    /// not (the reason is then dropped: nothing waits on a paused or removed Vision), except
    /// `over_budget` and `budget_unknown`, where it waits for a wake the budget allows.
    pub(super) fn visions_wake_admit(
        &self,
        channel_id: &str,
        vision_id: &str,
        reason: Option<WakeReason>,
    ) -> Result<Admit, &'static str> {
        let channel = self.channel_get(channel_id).ok_or("gone")?;
        let vision = self
            .vision_get(vision_id)
            .filter(|_| channel.members.iter().any(|member| member == vision_id))
            .ok_or("gone")?;
        if vision.paused || channel.paused_workers.contains(&vision.id) {
            return Err("paused");
        }
        let config = self
            .runtime_configuration(Some(&channel.project_root), None)
            .map_err(|_| "configuration")?
            .config
            .visions;
        if !config.enabled {
            return Err("disabled");
        }
        if self.visions_held_by_quiet(channel_id, reason.as_ref()) {
            self.visions_cache()
                .wakes
                .wait(vision_id, channel_id, reason);
            return Err("quiet");
        }
        if let Some(reason) = &reason {
            if let Some(thread) = reason.thread() {
                // Its own chat, or (for the chief) one of its workers' there.
                let owner = self
                    .owner_of(thread)
                    .filter(|owner| {
                        owner.channel_id == channel_id
                            && (owner.vision_id == vision_id
                                || (channel.is_chief(vision_id)
                                    && channel.members.contains(&owner.vision_id)))
                    })
                    .ok_or("not_owned")?;
                if let Some(why) = held_back(&owner, reason) {
                    return Err(why);
                }
            }
        }
        if let Some(over) = self.visions_budget_gate(vision_id, channel_id) {
            self.visions_cache()
                .wakes
                .wait(vision_id, channel_id, reason);
            return Err(over.outcome());
        }
        Ok(self
            .visions_cache()
            .wakes
            .admit(vision_id, channel_id, reason, config.max_wakes))
    }

    /// Prices became known: every Vision whose reasons wait tries its wake again (each still
    /// checked like any other).
    pub(super) fn visions_wake_waiting(&self) {
        self.visions_budget_recheck();
        let Some(sink) = self.scheduler_sink() else {
            return;
        };
        let waiting = self.visions_cache().wakes.waiting();
        for (vision_id, channel_id) in waiting {
            let outcome = self.visions_wake_or_queue(&sink, &channel_id, &vision_id, None);
            self.visions_note(
                "wake",
                json!({"vision_id": vision_id, "channel_id": channel_id, "reason": "prices_known", "outcome": outcome}),
            );
        }
    }

    /// The person wrote in `channel_id`: its Visions may wake `visions.max_wakes` times again.
    pub(super) fn visions_reset_wakes(&self, channel_id: &str) {
        self.visions_cache().wakes.person_wrote(channel_id);
    }

    /// What a chat's run end means for Visions: a Vision chat's reply goes to its channel and
    /// what waited starts its next run; a work chat's end wakes its Vision, unless the chat
    /// goes on (a queued message or a running goal is next) or the person holds it. Any run's
    /// end also retries the Visions whose run was refused as busy.
    pub(crate) fn visions_after_run(
        &self,
        sink: &Arc<dyn EventSink>,
        thread_id: &str,
        terminal: &ThreadEvent,
    ) {
        // Those refused before this run ended; one refused just now waits for the next end.
        let due = self.visions_cache().wakes.due_retries();
        self.visions_mark_outcome(thread_id, terminal);
        self.visions_budget_after_run(thread_id);
        self.visions_run_ended(sink, thread_id, terminal);
        for (vision_id, channel_id) in due {
            let outcome = self.visions_wake_or_queue(sink, &channel_id, &vision_id, None);
            self.visions_note(
                "wake",
                json!({"vision_id": vision_id, "channel_id": channel_id, "reason": "retry", "outcome": outcome}),
            );
        }
    }

    fn visions_run_ended(
        &self,
        sink: &Arc<dyn EventSink>,
        thread_id: &str,
        terminal: &ThreadEvent,
    ) {
        let Some(owner) = self.owner_of(thread_id) else {
            return;
        };
        if owner.kind == OwnedKind::VisionChat {
            self.visions_chat_ended(sink, &owner, terminal);
            return;
        }
        let Some(reason) = work_end_reason(thread_id, terminal) else {
            return;
        };
        let goes_on = self.queue_for(thread_id).iter().any(|queued| !queued.held)
            || self
                .goal_for(thread_id)
                .is_some_and(|goal| goal.status == GoalStatus::Running);
        if owner.hold != Hold::None || goes_on {
            return;
        }
        if matches!(reason, WakeReason::ChatFailed(_)) {
            let _ = self.channel_post(
                &owner.channel_id,
                &owner.vision_id,
                ChannelEntry::Notice {
                    kind: NoticeKind::ChatFailed,
                    vision_id: Some(owner.vision_id.clone()),
                    thread_id: Some(thread_id.to_owned()),
                    unpriced_requests: None,
                    reason: None,
                },
            );
        }
        self.visions_wake(sink, &owner.channel_id, &owner.vision_id, reason);
    }

    /// A Vision chat's run ended: its reply is the Vision's message in the channel, and what
    /// waited meanwhile starts the next run (still bounded), whether this one finished or failed.
    /// A stopped one (the person's Stop, or a pause) starts nothing.
    fn visions_chat_ended(&self, sink: &Arc<dyn EventSink>, owner: &Owner, terminal: &ThreadEvent) {
        let (vision_id, channel_id) = (owner.vision_id.as_str(), owner.channel_id.as_str());
        self.visions_post_reply(owner, terminal);
        self.visions_silent_check(sink, owner, terminal);
        let started = self.visions_cache().wakes.ended(vision_id, channel_id);
        if let Some(started) = started {
            self.visions_pass_on_left_open(owner, started);
        }
        let stopped = matches!(terminal, ThreadEvent::Failed { error }
            if error.starts_with(themis_core::runtime::STOPPED_BY_YOU));
        if stopped {
            return;
        }
        let outcome = self.visions_wake_or_queue(sink, channel_id, vision_id, None);
        if outcome != "idle" {
            self.visions_note(
                "wake",
                json!({"vision_id": vision_id, "channel_id": channel_id, "reason": "queued", "outcome": outcome}),
            );
        }
    }

    /// The questions a work chat asked wake its Vision: a permission question (the person's
    /// alone) once, as `ApprovalWaiting`, so it can tell the person.
    pub(crate) fn visions_question_asked(&self, thread_id: &str, question_ids: &[String]) {
        let Some(owner) = self
            .owner_of(thread_id)
            .filter(|owner| owner.kind == OwnedKind::Work)
        else {
            return;
        };
        let workers = self.is_worker(&owner);
        for id in question_ids {
            if self.visions_permission_question(thread_id, std::iter::once(id.as_str())) {
                // A worker's permission question goes straight to the person, under the chief,
                // and wakes no one: only the person may answer it.
                if workers {
                    self.visions_pass_on_stranded(&owner, id);
                } else if self.visions_cache().wakes.first_approval_notice(id) {
                    self.visions_wake_later(
                        &owner.channel_id,
                        &self.chief_of(&owner),
                        WakeReason::ApprovalWaiting(thread_id.to_owned(), id.clone()),
                    );
                }
                continue;
            }
            let outcome = self.visions_wake_later(
                &owner.channel_id,
                &owner.vision_id,
                WakeReason::QuestionAsked(thread_id.to_owned(), id.clone()),
            );
            // No run may take it: the person gets it in the channel, from the chief.
            if STRANDS.contains(&outcome) {
                self.visions_pass_on_stranded(&owner, id);
            }
        }
    }

    /// An approval box opened in a chat (`approval_id`): its Vision, if it owns the chat, wakes
    /// once to tell the person.
    pub(crate) fn visions_approval_waiting(
        &self,
        sink: &Arc<dyn EventSink>,
        thread_id: &str,
        approval_id: &str,
    ) {
        let Some(owner) = self
            .owner_of(thread_id)
            .filter(|owner| owner.kind == OwnedKind::Work)
        else {
            return;
        };
        // A worker's approval box waits in the worker's thread, where the person answers it.
        if self.is_worker(&owner)
            || !self
                .visions_cache()
                .wakes
                .first_approval_notice(approval_id)
        {
            return;
        }
        let reason = WakeReason::ApprovalWaiting(thread_id.to_owned(), approval_id.to_owned());
        // The chief speaks for the team.
        let tells = self.chief_of(&owner);
        self.visions_wake(sink, &owner.channel_id, &tells, reason);
    }

    /// What a run's approval hook calls for each box it shows (see `visions_approval_waiting`).
    pub(crate) fn visions_approval_notifier(
        &self,
        sink: &Arc<dyn EventSink>,
        thread_id: &str,
    ) -> crate::approvals::Asked {
        let (state, sink, thread_id) = (self.clone(), Arc::clone(sink), thread_id.to_owned());
        Arc::new(move |approval_id: &str| {
            state.visions_approval_waiting(&sink, &thread_id, approval_id);
        })
    }
}

/// The reason a work chat's run end wakes its Vision for; none for a stop (the person's wakes
/// it on its own, as `ChatStopped`; the Vision's own or its pause needs nothing).
fn work_end_reason(thread_id: &str, terminal: &ThreadEvent) -> Option<WakeReason> {
    match terminal {
        ThreadEvent::Failed { error }
            if error.starts_with(themis_core::runtime::STOPPED_BY_YOU) =>
        {
            None
        }
        ThreadEvent::Failed { .. } => Some(WakeReason::ChatFailed(thread_id.to_owned())),
        ThreadEvent::Finished { .. } | ThreadEvent::Incomplete { .. } => {
            Some(WakeReason::ChatFinished(thread_id.to_owned()))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    // Team wakes, in a file of their own; they share this module's helpers.
    mod team {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/state/visions/team_wakes_tests.rs"
        ));
    }

    use themis_core::diagnostics::DiagnosticFilter;

    use super::super::{Channel, Vision};
    use super::*;
    use crate::sink::ChannelSink;

    fn finished(thread: &str) -> WakeReason {
        WakeReason::ChatFinished(thread.to_owned())
    }

    /// `limit` runs in a row, then nothing starts (the reason waits, not lost) until the
    /// person writes in the channel; another channel's Visions keep their count.
    #[test]
    fn wakes_stop_at_max_until_the_person_writes() {
        let mut slots = WakeSlots::default();
        for _ in 0..3 {
            let admitted = slots.admit("v", "c", Some(WakeReason::Mentioned), 3);
            assert_eq!(admitted, Admit::Start(vec![WakeReason::Mentioned]));
            slots.ended("v", "c");
        }
        assert_eq!(slots.admit("v", "c", Some(finished("t")), 3), Admit::Limit);
        assert_eq!(slots.admit("v", "c", None, 3), Admit::Limit);
        slots.person_wrote("other");
        assert_eq!(slots.admit("v", "c", None, 3), Admit::Limit);
        slots.person_wrote("c");
        assert_eq!(
            slots.admit("v", "c", Some(WakeReason::Mentioned), 3),
            Admit::Start(vec![finished("t"), WakeReason::Mentioned]),
            "what waited goes with the person's message"
        );
        // A run refused before it started still counts; its reasons wait first.
        slots.refused("v", "c", vec![finished("t")], false);
        assert_eq!(slots.admit("v", "c", None, 1), Admit::Limit);
        assert_eq!(
            slots.admit("v", "c", None, 2),
            Admit::Start(vec![finished("t")])
        );
    }

    /// Reasons that arrive while the Vision runs wait (once each, in order) and all go into
    /// its next run; none starts a second run alongside.
    #[test]
    fn events_while_a_vision_runs_are_queued_not_dropped() {
        let mut slots = WakeSlots::default();
        let asked = WakeReason::QuestionAsked("a".into(), "q1".into());
        assert_eq!(
            slots.admit("v", "c", Some(WakeReason::Mentioned), 6),
            Admit::Start(vec![WakeReason::Mentioned])
        );
        assert_eq!(slots.admit("v", "c", Some(finished("a")), 6), Admit::Queued);
        assert_eq!(slots.admit("v", "c", Some(asked.clone()), 6), Admit::Queued);
        assert_eq!(slots.admit("v", "c", Some(finished("a")), 6), Admit::Queued);
        // Another Vision, or the same one in another channel, runs on its own.
        assert!(matches!(
            slots.admit("w", "c", Some(finished("b")), 6),
            Admit::Start(_)
        ));
        assert!(matches!(
            slots.admit("v", "d", Some(finished("b")), 6),
            Admit::Start(_)
        ));
        slots.ended("v", "c");
        assert_eq!(
            slots.admit("v", "c", None, 6),
            Admit::Start(vec![finished("a"), asked])
        );
        slots.ended("v", "c");
        assert_eq!(slots.admit("v", "c", None, 6), Admit::Idle);
        // A flood keeps the newest reasons, bounded.
        slots.admit("v", "c", Some(WakeReason::Mentioned), 100);
        for n in 0..(MAX_QUEUED + 10) {
            slots.admit("v", "c", Some(finished(&n.to_string())), 100);
        }
        slots.ended("v", "c");
        let Admit::Start(next) = slots.admit("v", "c", None, 100) else {
            panic!("the queued reasons start the next run");
        };
        assert_eq!(next.len(), MAX_QUEUED);
        assert_eq!(next.last(), Some(&finished(&(MAX_QUEUED + 9).to_string())));
    }

    struct Setup {
        _dir: tempfile::TempDir,
        state: AppState,
        channel: String,
        wren: String,
        ada: String,
    }

    fn vision(state: &AppState, name: &str) -> String {
        state
            .vision_save(
                Vision {
                    id: String::new(),
                    name: name.into(),
                    role: String::new(),
                    shape: "cloud".into(),
                    color: "teal".into(),
                    model: None,
                    accessory: "none".into(),
                    soul: String::new(),
                    paused: false,
                    revision: String::new(),
                },
                None,
            )
            .unwrap()
            .id
    }

    /// Wren and Ada in `#work` (or only Wren).
    fn setup(both: bool) -> Setup {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        let state = AppState::new_for_test(dir.path().join("settings.json"));
        // Budgets need a price catalog; a free model is enough here.
        *state.inner.go_catalog.try_write().unwrap() = vec![themis_core::providers::GoModel {
            id: "free".into(),
            price: Some(themis_core::providers::ModelPrice::default()),
            ..Default::default()
        }];
        let wren = vision(&state, "Wren");
        let ada = vision(&state, "Ada");
        let members = if both {
            vec![wren.clone(), ada.clone()]
        } else {
            vec![wren.clone()]
        };
        let channel = state
            .channel_save(
                Channel {
                    id: String::new(),
                    name: "work".into(),
                    topic: String::new(),
                    project_root: project.canonicalize().unwrap(),
                    full_access: false,
                    members,
                    chief: String::new(),
                    autonomy: None,
                    custom: None,
                    daily_budget_usd: None,
                    paused_workers: Vec::new(),
                    revision: String::new(),
                },
                None,
            )
            .unwrap()
            .id;
        Setup {
            _dir: dir,
            state,
            channel,
            wren,
            ada,
        }
    }

    fn sink() -> Arc<dyn EventSink> {
        Arc::new(ChannelSink::channel().0)
    }

    /// The Visions the log says a wake started a run for, in order.
    fn started(state: &AppState) -> Vec<String> {
        let filter = DiagnosticFilter {
            service: Some("visions".into()),
            ..DiagnosticFilter::default()
        };
        let mut records = state.inner.diagnostics.query(&filter).unwrap();
        records.sort_by_key(|record| record.timestamp);
        records
            .into_iter()
            .filter(|record| record.event == "wake" && record.details["outcome"] == "started")
            .map(|record| record.details["vision_id"].as_str().unwrap().to_owned())
            .collect()
    }

    fn own(state: &AppState, thread: &str, vision: &str, channel: &str, hold: Hold) {
        state
            .owner_set(Owner {
                thread_id: thread.into(),
                vision_id: vision.into(),
                channel_id: channel.into(),
                kind: OwnedKind::Work,
                hold,
            })
            .unwrap();
    }

    /// A paused Vision, or Visions switched off, wake for nothing (and keep nothing waiting);
    /// a chat the person stopped wakes its Vision only to say so, a taken-over one not at all.
    #[test]
    fn no_wake_while_paused() {
        let s = setup(false);
        let admit = |reason| {
            s.state
                .visions_wake_admit(&s.channel, &s.wren, Some(reason))
        };
        s.state.visions_pause(&s.wren, true).unwrap();
        assert_eq!(admit(WakeReason::Mentioned), Err("paused"));
        s.state.visions_pause(&s.wren, false).unwrap();
        assert_eq!(
            s.state.visions_wake_admit(&s.channel, &s.wren, None),
            Ok(Admit::Idle),
            "nothing was kept while paused"
        );

        let runtime = s
            .state
            .inner
            .settings
            .path()
            .with_file_name("runtime.jsonc");
        std::fs::write(&runtime, r#"{"visions": {"enabled": false}}"#).unwrap();
        assert_eq!(admit(WakeReason::Mentioned), Err("disabled"));
        std::fs::remove_file(&runtime).unwrap();

        own(
            &s.state,
            "stopped",
            &s.wren,
            &s.channel,
            Hold::StoppedByPerson,
        );
        assert_eq!(admit(finished("stopped")), Err("held"));
        assert_eq!(
            admit(WakeReason::QuestionAsked("stopped".into(), "q".into())),
            Err("held")
        );
        assert!(matches!(
            admit(WakeReason::ChatStopped("stopped".into())),
            Ok(Admit::Start(_))
        ));
        own(&s.state, "mine", &s.wren, &s.channel, Hold::TakenOver);
        assert_eq!(admit(finished("mine")), Err("taken_over"));
        own(&s.state, "theirs", &s.ada, &s.channel, Hold::None);
        assert_eq!(admit(finished("theirs")), Err("not_owned"));
        // Its own chat, but in another channel: not this channel's to wake for.
        own(&s.state, "elsewhere", &s.wren, "other-channel", Hold::None);
        assert_eq!(admit(finished("elsewhere")), Err("not_owned"));
    }

    /// The person's post wakes the channel's chief only, whoever it mentions: workers take
    /// their work from the chief.
    #[test]
    fn a_person_post_wakes_only_the_chief() {
        let s = setup(true);
        s.state
            .visions_post(&sink(), &s.channel, "@ada can you check the docs?")
            .unwrap();
        s.state
            .visions_post(&sink(), &s.channel, "anyone there?")
            .unwrap();
        let woken = started(&s.state);
        assert!(!woken.is_empty());
        assert!(woken.iter().all(|vision| *vision == s.wren), "{woken:?}");
    }

    /// Wren leads, Ada works: a chief and its worker share one wake count, stop together at
    /// `visions.max_wakes`, and the channel is told once; the person's post starts it again.
    #[test]
    fn a_team_bouncing_work_stops_at_the_channel_cap_with_one_notice() {
        let s = setup(true);
        let runtime = s
            .state
            .inner
            .settings
            .path()
            .with_file_name("runtime.jsonc");
        std::fs::write(&runtime, r#"{"visions": {"max_wakes": 2}}"#).unwrap();
        let wake = |vision: &str, reason| s.state.visions_wake(&sink(), &s.channel, vision, reason);
        assert_eq!(wake(&s.wren, WakeReason::Mentioned), "started");
        assert_eq!(wake(&s.ada, WakeReason::Tasked(1)), "started");
        assert_eq!(
            wake(&s.wren, WakeReason::WorkerNote(s.ada.clone(), 2)),
            "limit"
        );
        assert_eq!(wake(&s.ada, WakeReason::Tasked(3)), "limit");
        let paused_notices = |state: &AppState| {
            state
                .channel_page(&s.channel, None, 1 << 20)
                .unwrap()
                .entries
                .iter()
                .filter(|line| {
                    matches!(
                        line.entry,
                        ChannelEntry::Notice {
                            kind: NoticeKind::TeamPaused,
                            ..
                        }
                    )
                })
                .count()
        };
        assert_eq!(paused_notices(&s.state), 1);
        // The person's post resets the count: the chief wakes again.
        s.state
            .visions_post(&sink(), &s.channel, "carry on")
            .unwrap();
        // The chief wakes, and Ada's task that waited at the limit starts too.
        let after: Vec<String> = started(&s.state).into_iter().skip(2).collect();
        assert!(
            after.contains(&s.wren) && after.contains(&s.ada),
            "{after:?}"
        );
        assert_eq!(paused_notices(&s.state), 1);
    }

    /// A work chat's event wakes the worker that owns it, not the chief.
    #[test]
    fn a_work_chat_event_wakes_its_worker() {
        let s = setup(true);
        own(&s.state, "ada-work", &s.ada, &s.channel, Hold::None);
        let done = ThreadEvent::Finished {
            model: None,
            result: "ok".into(),
        };
        s.state.visions_after_run(&sink(), "ada-work", &done);
        assert_eq!(started(&s.state), vec![s.ada.clone()]);
    }

    /// The person hands a worker's chat back: the worker picks it up and the chief is told.
    #[tokio::test]
    async fn hand_back_wakes_the_owning_worker_and_tells_the_chief() {
        let s = setup(true);
        own(&s.state, "ada-work", &s.ada, &s.channel, Hold::TakenOver);
        s.state.visions_hand_back(&sink(), "ada-work").unwrap();
        let woken = started(&s.state);
        assert!(woken.contains(&s.ada), "{woken:?}");
        assert!(woken.contains(&s.wren), "{woken:?}");
    }

    /// A wake refused as busy is tried again when a run ends, each try counted, so the tries
    /// stop at `visions.max_wakes` until the person writes.
    #[test]
    fn busy_retries_stay_bounded_by_max_wakes() {
        let mut slots = WakeSlots::default();
        assert!(matches!(
            slots.admit("v", "c", Some(WakeReason::Mentioned), 2),
            Admit::Start(_)
        ));
        slots.refused("v", "c", vec![WakeReason::Mentioned], true);
        assert_eq!(slots.due_retries(), vec![("v".to_owned(), "c".to_owned())]);
        assert!(slots.due_retries().is_empty(), "once per refusal");
        assert!(matches!(slots.admit("v", "c", None, 2), Admit::Start(_)));
        slots.refused("v", "c", vec![WakeReason::Mentioned], true);
        assert_eq!(slots.due_retries().len(), 1);
        assert_eq!(slots.admit("v", "c", None, 2), Admit::Limit);
        slots.person_wrote("c");
        assert!(matches!(slots.admit("v", "c", None, 2), Admit::Start(_)));
    }

    /// A run that could not even be spawned (no runtime) is not tried again on every run end.
    #[test]
    fn a_refused_spawn_is_not_retried_on_every_run_end() {
        let s = setup(false);
        s.state
            .visions_post(&sink(), &s.channel, "anyone there?")
            .unwrap();
        assert_eq!(started(&s.state), vec![s.wren.clone()]);
        let done = ThreadEvent::Finished {
            model: None,
            result: "ok".into(),
        };
        s.state.visions_after_run(&sink(), "unrelated-chat", &done);
        assert_eq!(started(&s.state), vec![s.wren.clone()]);
    }

    /// Reasons that arrived while a Vision run failed are not stranded: the next run starts
    /// with them (bounded like every other).
    #[test]
    fn reasons_queued_during_a_failed_run_start_the_next() {
        let s = setup(false);
        s.state
            .owner_set(Owner {
                thread_id: "vision-chat".into(),
                vision_id: s.wren.clone(),
                channel_id: s.channel.clone(),
                kind: OwnedKind::VisionChat,
                hold: Hold::None,
            })
            .unwrap();
        own(&s.state, "work", &s.wren, &s.channel, Hold::None);
        let first = s
            .state
            .visions_wake_admit(&s.channel, &s.wren, Some(WakeReason::Mentioned));
        assert!(matches!(first, Ok(Admit::Start(_))));
        let queued = s
            .state
            .visions_wake_admit(&s.channel, &s.wren, Some(finished("work")));
        assert_eq!(queued, Ok(Admit::Queued));
        let failed = ThreadEvent::Failed {
            error: "provider error".into(),
        };
        s.state.visions_after_run(&sink(), "vision-chat", &failed);
        assert_eq!(started(&s.state), vec![s.wren.clone()]);
    }

    /// An approval waiting in a work chat wakes its Vision once per approval (to tell the
    /// person); not for a chat the person took over, nor for another Vision's chat.
    #[test]
    fn an_approval_waiting_wakes_the_vision_once() {
        let s = setup(true);
        own(&s.state, "work", &s.wren, &s.channel, Hold::None);
        own(&s.state, "mine", &s.ada, &s.channel, Hold::TakenOver);
        s.state.visions_approval_waiting(&sink(), "work", "a1");
        s.state.visions_approval_waiting(&sink(), "work", "a1");
        s.state.visions_approval_waiting(&sink(), "mine", "a2");
        s.state
            .visions_approval_waiting(&sink(), "plain-chat", "a3");
        assert_eq!(started(&s.state), vec![s.wren.clone()]);
        let line = WakeReason::ApprovalWaiting("work".into(), "a1".into()).line();
        assert_eq!(line, "approval_waiting work a1");
    }

    /// A refusal that is not "busy" (no chat, a bad configuration, no runtime) is not tried
    /// again on every run end; its reasons wait for the next wake.
    #[test]
    fn a_permanent_refusal_is_not_retried() {
        let mut slots = WakeSlots::default();
        assert!(matches!(
            slots.admit("v", "c", Some(WakeReason::Mentioned), 6),
            Admit::Start(_)
        ));
        slots.refused("v", "c", vec![WakeReason::Mentioned], false);
        assert!(slots.due_retries().is_empty());
        assert_eq!(
            slots.admit("v", "c", None, 6),
            Admit::Start(vec![WakeReason::Mentioned]),
            "its reasons still wait"
        );
    }
}
