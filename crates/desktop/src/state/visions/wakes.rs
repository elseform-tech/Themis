//! Waking a Vision: a run in its Vision chat for a channel (see `runs`).
//!
//! Wakes are bounded like hook wakes (`state/hook_wakes.rs`): one Vision run at a time per
//! Vision chat, reasons that arrive meanwhile wait in a per-Vision queue and go together into
//! its next wake, at most `visions.max_wakes` runs in a row until the person writes in the
//! channel, none while the Vision is paused or Visions are off, and none for a chat the person
//! stopped (except the one `ChatStopped` that tells the Vision so) or took over. Each decision
//! is logged as `visions/wake {vision_id, channel_id, reason, outcome}`, metadata only.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::json;

use super::{ChannelEntry, Hold, NoticeKind, OwnedKind, Owner};
use crate::sink::EventSink;
use crate::state::AppState;
use crate::types::{GoalStatus, ThreadEvent};

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
        }
    }

    /// The owned chat it is about, if any.
    fn thread(&self) -> Option<&str> {
        match self {
            WakeReason::Mentioned => None,
            WakeReason::ChatFinished(thread)
            | WakeReason::ChatFailed(thread)
            | WakeReason::ChatStopped(thread)
            | WakeReason::QuestionAsked(thread, _)
            | WakeReason::HandedBack(thread) => Some(thread),
        }
    }

    /// One line of the wake message: the label, then the chat's and the question's ids.
    pub(super) fn line(&self) -> String {
        match self {
            WakeReason::Mentioned => self.label().to_owned(),
            WakeReason::QuestionAsked(thread, question) => {
                format!("{} {thread} {question}", self.label())
            }
            other => format!("{} {}", other.label(), other.thread().unwrap_or_default()),
        }
    }
}

/// One Vision's wakes in one channel.
#[derive(Debug, Default)]
struct WakeSlot {
    /// A run of its Vision chat is starting or running.
    running: bool,
    /// Runs in a row since the person last wrote in the channel.
    count: u32,
    /// Reasons waiting for its next run, oldest first.
    queued: Vec<WakeReason>,
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

/// Every Vision's wakes, by (Vision id, channel id). In memory: a restart starts afresh.
#[derive(Debug, Default)]
pub(crate) struct WakeSlots(HashMap<(String, String), WakeSlot>);

impl WakeSlots {
    /// Queues `reason` (once) and decides, in the same step, whether a run starts now.
    pub(super) fn admit(
        &mut self,
        vision_id: &str,
        channel_id: &str,
        reason: Option<WakeReason>,
        limit: u32,
    ) -> Admit {
        let slot = self
            .0
            .entry((vision_id.to_owned(), channel_id.to_owned()))
            .or_default();
        if let Some(reason) = reason {
            if !slot.queued.contains(&reason) {
                slot.queued.push(reason);
            }
            let over = slot.queued.len().saturating_sub(MAX_QUEUED);
            slot.queued.drain(..over);
        }
        if slot.running {
            return Admit::Queued;
        }
        if slot.queued.is_empty() {
            return Admit::Idle;
        }
        if slot.count >= limit {
            return Admit::Limit;
        }
        slot.count += 1;
        slot.running = true;
        Admit::Start(std::mem::take(&mut slot.queued))
    }

    /// The run ended.
    pub(super) fn ended(&mut self, vision_id: &str, channel_id: &str) {
        if let Some(slot) = self.slot(vision_id, channel_id) {
            slot.running = false;
        }
    }

    /// The run was refused before it started: its reasons wait again, first, and it did not
    /// count.
    pub(super) fn refused(&mut self, vision_id: &str, channel_id: &str, reasons: Vec<WakeReason>) {
        if let Some(slot) = self.slot(vision_id, channel_id) {
            slot.running = false;
            slot.count = slot.count.saturating_sub(1);
            let later = std::mem::replace(&mut slot.queued, reasons);
            for reason in later {
                if !slot.queued.contains(&reason) {
                    slot.queued.push(reason);
                }
            }
        }
    }

    /// The person wrote in the channel: its Visions may wake again.
    pub(super) fn person_wrote(&mut self, channel_id: &str) {
        for ((_, channel), slot) in &mut self.0 {
            if channel == channel_id {
                slot.count = 0;
            }
        }
    }

    fn slot(&mut self, vision_id: &str, channel_id: &str) -> Option<&mut WakeSlot> {
        self.0
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
    /// queues the reason for its next one (see the module notes for the bounds).
    pub fn visions_wake(
        &self,
        sink: &Arc<dyn EventSink>,
        channel_id: &str,
        vision_id: &str,
        reason: WakeReason,
    ) {
        let label = reason.label();
        let outcome = self.visions_wake_or_queue(sink, channel_id, vision_id, Some(reason));
        self.visions_note(
            "wake",
            json!({"vision_id": vision_id, "channel_id": channel_id, "reason": label, "outcome": outcome}),
        );
    }

    /// The same with the app's own sink, for callers that hold none (the person's Stop, a
    /// question asked). Without a server nothing can run, and it is logged so.
    pub(crate) fn visions_wake_later(&self, channel_id: &str, vision_id: &str, reason: WakeReason) {
        match self.scheduler_sink() {
            Some(sink) => self.visions_wake(&sink, channel_id, vision_id, reason),
            None => self.visions_note(
                "wake",
                json!({"vision_id": vision_id, "channel_id": channel_id, "reason": reason.label(), "outcome": "no_sink"}),
            ),
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
            Ok(Admit::Limit) => "limit",
            Ok(Admit::Idle) => "idle",
            Err(refused) => refused,
        }
    }

    /// Whether the Vision may wake now, checked and counted under one lock; `Err` names why
    /// not (the reason is then dropped: nothing waits on a paused or removed Vision).
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
        if vision.paused {
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
        if let Some(reason) = &reason {
            if let Some(thread) = reason.thread() {
                let owner = self
                    .owner_of(thread)
                    .filter(|owner| owner.vision_id == vision_id)
                    .ok_or("not_owned")?;
                if let Some(why) = held_back(&owner, reason) {
                    return Err(why);
                }
            }
        }
        Ok(self
            .visions_cache()
            .wakes
            .admit(vision_id, channel_id, reason, config.max_wakes))
    }

    /// The person wrote in `channel_id`: its Visions may wake `visions.max_wakes` times again.
    pub(super) fn visions_reset_wakes(&self, channel_id: &str) {
        self.visions_cache().wakes.person_wrote(channel_id);
    }

    /// What a chat's run end means for Visions: a Vision chat's reply goes to its channel and
    /// what waited starts its next run; a work chat's end wakes its Vision, unless the chat
    /// goes on (a queued message or a running goal is next) or the person holds it.
    pub(crate) fn visions_after_run(
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
            let _ = self.channel_append(
                &owner.channel_id,
                ChannelEntry::Notice {
                    kind: NoticeKind::ChatFailed,
                    vision_id: Some(owner.vision_id.clone()),
                    thread_id: Some(thread_id.to_owned()),
                },
            );
        }
        self.visions_wake(sink, &owner.channel_id, &owner.vision_id, reason);
    }

    /// A Vision chat's run ended: its reply is the Vision's message in the channel, and when
    /// it finished, what waited meanwhile starts the next run (still bounded).
    fn visions_chat_ended(&self, sink: &Arc<dyn EventSink>, owner: &Owner, terminal: &ThreadEvent) {
        let (vision_id, channel_id) = (owner.vision_id.as_str(), owner.channel_id.as_str());
        let finished = self.visions_post_reply(owner, terminal);
        self.visions_cache().wakes.ended(vision_id, channel_id);
        if !finished {
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

    /// The questions a work chat asked (not permissions, which are the person's) wake its
    /// Vision.
    pub(crate) fn visions_question_asked(&self, thread_id: &str, question_ids: &[String]) {
        let Some(owner) = self
            .owner_of(thread_id)
            .filter(|owner| owner.kind == OwnedKind::Work)
        else {
            return;
        };
        for id in question_ids {
            if self.visions_permission_question(thread_id, std::iter::once(id.as_str())) {
                continue;
            }
            self.visions_wake_later(
                &owner.channel_id,
                &owner.vision_id,
                WakeReason::QuestionAsked(thread_id.to_owned(), id.clone()),
            );
        }
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
    use themis_core::configuration::ApprovalMode;
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
        // A run refused before it started does not count, and its reasons wait first.
        slots.refused("v", "c", vec![finished("t")]);
        slots.ended("v", "c");
        assert_eq!(
            slots.admit("v", "c", None, 1),
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
                    autonomy: None,
                    custom: None,
                    daily_budget_usd: None,
                    soul: String::new(),
                    paused: false,
                    revision: String::new(),
                },
                None,
            )
            .unwrap()
            .id
    }

    /// Wren and Ada in `#hq` (or only Wren).
    fn setup(both: bool) -> Setup {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        let state = AppState::new_for_test(dir.path().join("settings.json"));
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
                    name: "hq".into(),
                    topic: String::new(),
                    project_root: project.canonicalize().unwrap(),
                    approval_mode: ApprovalMode::Custom,
                    members,
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
    }

    /// `@Name` wakes only that Vision; no mention wakes the only member, and nobody in a
    /// channel of several.
    #[test]
    fn a_mention_wakes_only_that_vision() {
        let s = setup(true);
        s.state
            .visions_post(&sink(), &s.channel, "@ada can you check the docs?")
            .unwrap();
        assert_eq!(started(&s.state), vec![s.ada.clone()]);
        s.state
            .visions_post(&sink(), &s.channel, "anyone there?")
            .unwrap();
        assert_eq!(
            started(&s.state),
            vec![s.ada.clone()],
            "nobody is mentioned"
        );

        let alone = setup(false);
        alone
            .state
            .visions_post(&sink(), &alone.channel, "anyone there?")
            .unwrap();
        assert_eq!(started(&alone.state), vec![alone.wren.clone()]);
    }
}
