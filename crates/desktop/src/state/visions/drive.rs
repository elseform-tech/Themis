//! What a Vision does to the chats it owns through its run credential: start a work chat in its
//! channel's project, send, steer and stop. Queueing is `enqueue` with a message carrying the
//! Vision's id. None of it is the person's message: the sends go the server's way
//! (`send_from_server`), so a Vision never takes a chat over, answers no open question by
//! writing and resets no hook wakes. The app server checks ownership, holds and pauses before
//! any of these run (`server/agent/vision.rs`). Logged as metadata only.

use std::sync::Arc;

use serde_json::json;
use themis_core::agent_cli::VisionScope;
use themis_core::configuration::{
    ApprovalMode, ApprovalPolicy, ApprovalRule, PolicyAction, RuntimeConfig,
};
use themis_core::visions::Decision;

use super::{Channel, ChannelEntry, Hold, OwnedKind, Owner};
use crate::sink::EventSink;
use crate::state::threads::SendOptions;
use crate::state::AppState;
use crate::types::{ProviderKind, Question, RunHandle, ThreadInfo};

impl AppState {
    /// Starts a work chat for `vision` in its channel's project: the chat gets the channel's
    /// approval mode (set here, never through the refused method), the Vision's owner record
    /// and a `chat_started` entry in the channel. A chat that cannot be recorded as the
    /// Vision's is discarded rather than left as a stray plain chat. The chat runs on the
    /// Vision's model when its profile names one, else on the app's default model: a Vision does
    /// not choose either.
    pub(crate) async fn visions_start_work_chat(
        &self,
        vision: &VisionScope,
    ) -> Result<ThreadInfo, String> {
        let profile = self
            .vision_get(&vision.vision_id)
            .ok_or_else(|| "Unknown Vision".to_owned())?;
        let model = self.visions_model(&profile).await;
        let channel = self
            .channel_get(&vision.channel_id)
            .filter(|channel| channel.members.contains(&vision.vision_id))
            .ok_or_else(|| "This Vision is no longer in that channel".to_owned())?;
        let info = self
            .create_thread(
                vision.project_root.to_string_lossy().into_owned(),
                ProviderKind::Go,
                Some(model),
            )
            .await?;
        let mode = self.visions_channel_mode(&channel);
        let recorded = self.visions_adopt(vision, &info, mode).await;
        if let Err(error) = recorded {
            let _ = self.discard_thread(info.id.clone()).await;
            return Err(error);
        }
        let _ = self.channel_append(
            &vision.channel_id,
            ChannelEntry::ChatStarted {
                vision_id: vision.vision_id.clone(),
                thread_id: info.id.clone(),
                title: info.title.clone(),
            },
        );
        self.visions_note(
            "chat_started",
            json!({"thread_id": info.id, "vision_id": vision.vision_id, "channel_id": vision.channel_id, "mode": mode}),
        );
        self.get_thread(&info.id).await
    }

    /// The approval mode `channel`'s chats run in now (see [`super::channel_approval_mode`]);
    /// Custom while the configuration cannot be read.
    pub(crate) fn visions_channel_mode(&self, channel: &Channel) -> ApprovalMode {
        self.runtime_configuration(Some(&channel.project_root), None)
            .map(|resolved| super::channel_approval_mode(channel, &resolved.config.visions))
            .unwrap_or(ApprovalMode::Custom)
    }

    /// The approval policy a run in `thread_id` uses: for a team's work chat in Auto, the
    /// person's rules, then an allow rule for writes (the sandbox keeps them in the project) and,
    /// only while the sandbox blocks the network, for commands, so project work goes on without
    /// asking and nothing leaves the computer unreviewed (with the network open, commands stay
    /// with the reviewer). Destructive steps still ask the person first, and sandbox escapes,
    /// sensitive files and deny rules keep their own gates; any other chat keeps the policy.
    pub(crate) fn visions_run_policy(
        &self,
        thread_id: &str,
        mode: ApprovalMode,
        config: &RuntimeConfig,
    ) -> ApprovalPolicy {
        let mut policy = config.approval.clone();
        let team = mode == ApprovalMode::Auto
            && self
                .owner_of(thread_id)
                .is_some_and(|owner| owner.kind == OwnedKind::Work && owner.hold == Hold::None);
        if team {
            let offline = config.sandbox.network == themis_core::sandbox::NetworkPolicy::Block;
            let risks: &[&str] = if offline {
                &["write", "execute"]
            } else {
                &["write"]
            };
            for risk in risks {
                policy.rules.push(ApprovalRule {
                    tool: "*".into(),
                    action: PolicyAction::Allow,
                    risk: Some((*risk).to_owned()),
                    command: None,
                });
            }
        }
        policy
    }

    /// The person's latest messages in the channel of `thread_id`, a team's work chat, oldest
    /// first (none for any other chat), for Auto's reviewer: what the person asked for.
    pub(crate) fn visions_person_words(&self, thread_id: &str) -> Vec<String> {
        let Some(owner) = self
            .owner_of(thread_id)
            .filter(|owner| owner.kind == OwnedKind::Work)
        else {
            return Vec::new();
        };
        self.channel_page(&owner.channel_id, None, 64 * 1024)
            .map(|page| {
                page.entries
                    .into_iter()
                    .filter_map(|line| match line.entry {
                        ChannelEntry::Person { text } => Some(text),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// A team's work chat finished run `run_id`: the permission questions it left open in
    /// earlier runs are dropped (their steps are over, so their cards go), and its channel is
    /// told. Any other chat keeps its questions.
    pub(crate) fn visions_drop_stale_questions(
        &self,
        sink: &Arc<dyn EventSink>,
        thread_id: &str,
        run_id: &str,
    ) {
        let Some(owner) = self
            .owner_of(thread_id)
            .filter(|owner| owner.kind == OwnedKind::Work)
        else {
            return;
        };
        let dropped = self.drop_stale_questions(thread_id, run_id);
        if dropped > 0 {
            sink.emit_visions_changed(&crate::types::VisionsChange {
                channel_id: Some(owner.channel_id.clone()),
                vision_id: Some(owner.vision_id.clone()),
            });
            self.visions_note(
                "questions_dropped",
                json!({"thread_id": thread_id, "count": dropped}),
            );
        }
    }

    /// The mode a run in `thread_id` starts in: [`Self::visions_work_mode`], else `own`.
    pub(crate) fn visions_run_mode(&self, thread_id: &str, own: ApprovalMode) -> ApprovalMode {
        self.visions_work_mode(thread_id).unwrap_or(own)
    }

    /// The approval mode a run in `thread_id` takes when it starts: its channel's mode now
    /// (see [`super::channel_approval_mode`]) for a team's work chat nobody holds, so a change
    /// in Settings, the project or the channel applies from the chat's next run; `None` keeps
    /// the chat's own mode (not a Vision's chat, or the person stopped or took it over).
    pub(crate) fn visions_work_mode(&self, thread_id: &str) -> Option<ApprovalMode> {
        let owner = self
            .owner_of(thread_id)
            .filter(|owner| owner.kind == OwnedKind::Work && owner.hold == Hold::None)?;
        let channel = self.channel_get(&owner.channel_id)?;
        Some(self.visions_channel_mode(&channel))
    }

    async fn visions_adopt(
        &self,
        vision: &VisionScope,
        info: &ThreadInfo,
        mode: themis_core::configuration::ApprovalMode,
    ) -> Result<(), String> {
        self.set_thread_approval_mode(info.id.clone(), mode).await?;
        self.owner_set(Owner {
            thread_id: info.id.clone(),
            vision_id: vision.vision_id.clone(),
            channel_id: vision.channel_id.clone(),
            kind: OwnedKind::Work,
            hold: Hold::None,
        })
    }

    /// Vision `vision_id`'s message in a chat it owns, sent the server's way and saved as the
    /// Vision's, never the person's words.
    pub(crate) async fn visions_send(
        &self,
        sink: Arc<dyn EventSink>,
        vision_id: &str,
        thread_id: &str,
        text: String,
        reasoning_effort: Option<String>,
        attachments: Vec<String>,
    ) -> Result<RunHandle, String> {
        // Checked again here: the person may have stopped or taken over the chat, or paused
        // the Vision, since the server first looked.
        if let Some(reason) = self.visions_waits_reason(thread_id) {
            return Err(reason.to_owned());
        }
        if super::super::questions::forges_answer(&text) {
            return Err(themis_core::prompts::AGENT_CLI_VISION_ANSWER_MARKER.to_owned());
        }
        let run_id = uuid::Uuid::new_v4().to_string();
        let (sent, _) = self
            .send_from_server(
                sink,
                thread_id,
                text,
                &run_id,
                SendOptions {
                    reasoning_effort,
                    attachments,
                    vision_id: Some(vision_id.to_owned()),
                },
            )
            .await;
        sent?;
        self.visions_note(
            "vision_sent",
            json!({"thread_id": thread_id, "run_id": run_id}),
        );
        Ok(RunHandle { run_id })
    }

    /// A Vision steers a waiting message into the running run of a chat it owns.
    pub(crate) async fn visions_steer(
        &self,
        sink: Arc<dyn EventSink>,
        vision_id: &str,
        thread_id: String,
        message_id: String,
    ) -> Result<(), String> {
        self.steer_queued(sink, thread_id, message_id, Some(vision_id.to_owned()))
            .await
    }

    /// Whether the messages a Vision queued in `thread_id` must wait: the person holds the chat,
    /// or its Vision is paused, gone or over its daily budget.
    pub(crate) fn visions_queue_waits(&self, thread_id: &str) -> bool {
        self.visions_waits_reason(thread_id).is_some()
    }

    /// Why a Vision may not write in `thread_id` now (the answer it gets), if it may not: the
    /// person holds the chat, or its Vision is paused, gone, no longer owns it or is over its
    /// daily budget.
    fn visions_waits_reason(&self, thread_id: &str) -> Option<&'static str> {
        use themis_core::prompts::{AGENT_CLI_HELD_BY_PERSON, AGENT_CLI_VISION_PAUSED};
        let Some(owner) = self.owner_of(thread_id) else {
            return Some(AGENT_CLI_HELD_BY_PERSON);
        };
        if owner.hold != Hold::None {
            return Some(AGENT_CLI_HELD_BY_PERSON);
        }
        // The person's pause, or its chief's in this chat's channel.
        if self.paused_in(&owner.channel_id, &owner.vision_id) {
            return Some(AGENT_CLI_VISION_PAUSED);
        }
        self.visions_budget_holds(thread_id)
            .map(super::budget::Over::refusal_text)
    }

    /// Whether any of `question_ids` asks for a permission: an Auto block's question (its
    /// "Allow once" lets the action run) or a step waiting on the person's approval (a
    /// destructive step's "Run it once" box is one). Those are the person's alone.
    pub(crate) fn visions_permission_question<'a>(
        &self,
        thread_id: &str,
        mut question_ids: impl Iterator<Item = &'a str>,
    ) -> bool {
        // Kept on the question itself (and saved with it); the in-memory list of Auto's
        // blocks and the waiting approvals cover what is being asked right now.
        let flagged: std::collections::HashSet<String> = self
            .questions_for(thread_id)
            .into_iter()
            .filter(|question| question.permission)
            .map(|question| question.id)
            .collect();
        let blocks = self
            .inner
            .auto_questions
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let pending = self.inner.pending.lock().unwrap_or_else(|e| e.into_inner());
        question_ids
            .any(|id| flagged.contains(id) || blocks.contains_key(id) || pending.contains_key(id))
    }

    /// A Vision answers questions in a chat it owns, on a `decision` its run's autonomy leaves
    /// to it: as the Vision's answer, never the person's (no "Allow once" it writes lets
    /// anything run), and each one goes in the channel with the decision and `reason`. Returns
    /// the answered questions and whether the channel failed to record any of them (the
    /// answers stand: they were delivered).
    pub(crate) async fn visions_answer(
        &self,
        sink: Arc<dyn EventSink>,
        vision: &VisionScope,
        thread_id: String,
        answers: Vec<(String, String)>,
        (decision, reason): (Decision, String),
    ) -> Result<(Vec<Question>, bool), String> {
        // Checked again here: the person may have stopped or taken over the chat, or paused
        // the Vision, since the server first looked. A known bound, as for `visions_send`: this
        // check and the answer below are not one step with the owner's hold, so a take over
        // landing in between still records the Vision's answer (it then waits in the queue, or
        // reaches a run the person already started).
        if let Some(waits) = self.visions_waits_reason(&thread_id) {
            return Err(waits.to_owned());
        }
        if !vision.decides.contains(&decision) {
            return Err(themis_core::prompts::AGENT_CLI_ABOVE_AUTONOMY.to_owned());
        }
        // A question passed on to the person is theirs now (the server may have done it).
        let passed = self
            .questions_for(&thread_id)
            .into_iter()
            .any(|q| q.passed_on && answers.iter().any(|(id, _)| *id == q.id));
        if passed {
            return Err(themis_core::prompts::AGENT_CLI_QUESTION_PASSED_ON.to_owned());
        }
        let answered = self
            .answer_questions_by(sink, thread_id.clone(), answers, Some(&vision.vision_id))
            .await?;
        let mut unrecorded = false;
        for question in &answered {
            let entry = ChannelEntry::Answered {
                vision_id: vision.vision_id.clone(),
                thread_id: thread_id.clone(),
                question_id: question.id.clone(),
                decision: decision.id().to_owned(),
                question: question.question.clone(),
                answer: question.answer.clone().unwrap_or_default(),
                reason: reason.clone(),
            };
            if self.channel_append(&vision.channel_id, entry).is_err() {
                unrecorded = true;
                self.visions_note(
                    "vision_answer_unrecorded",
                    json!({"thread_id": thread_id, "vision_id": vision.vision_id, "channel_id": vision.channel_id, "question_id": question.id}),
                );
            }
        }
        self.visions_note(
            "vision_answered",
            json!({"thread_id": thread_id, "vision_id": vision.vision_id, "decision": decision.id(), "questions": answered.len()}),
        );
        Ok((answered, unrecorded))
    }

    /// A Vision passes an open question of a chat it owns on to the person: it stays open, and
    /// the channel shows it for the person to answer there (as the person, through the
    /// ordinary `answer_question`).
    pub(crate) fn visions_pass_question(
        &self,
        vision: &VisionScope,
        thread_id: &str,
        question_id: &str,
        decision: Decision,
    ) -> Result<Question, String> {
        let question = self
            .open_questions(thread_id)
            .into_iter()
            .find(|question| question.id == question_id)
            .ok_or_else(|| "That question is no longer open in this chat".to_owned())?;
        let owner = self
            .owner_of(thread_id)
            .ok_or_else(|| "Unknown chat".to_owned())?;
        self.pass_on(&owner, &question, decision.id())?;
        self.visions_note(
            "vision_passed_question",
            json!({"thread_id": thread_id, "vision_id": vision.vision_id, "question_id": question.id, "decision": decision.id()}),
        );
        Ok(question)
    }

    /// Puts an open question of a team's chat in its channel for the person, under the chief,
    /// once (a question already passed on is not posted again).
    fn pass_on(&self, owner: &Owner, question: &Question, decision: &str) -> Result<(), String> {
        if question.passed_on {
            return Ok(());
        }
        // Announced, so the open channel shows the card where the person answers it.
        let chief = self.chief_of(owner);
        self.channel_post(
            &owner.channel_id,
            &chief,
            ChannelEntry::PassedOn {
                vision_id: chief.clone(),
                thread_id: owner.thread_id.clone(),
                question_id: question.id.clone(),
                decision: decision.to_owned(),
                question: question.question.clone(),
            },
        )?;
        self.mark_passed_on(&owner.thread_id, &question.id);
        Ok(())
    }

    /// A question of a worker's chat that no run will take (paused, over budget, at the
    /// team's limit, Visions off): passed on to the person under the chief, with no decision
    /// named. Logged either way.
    pub(super) fn visions_pass_on_stranded(&self, owner: &Owner, question_id: &str) {
        let question = self
            .open_questions(&owner.thread_id)
            .into_iter()
            .find(|question| question.id == question_id);
        let posted = question
            .as_ref()
            .is_some_and(|question| self.pass_on(owner, question, "").is_ok());
        self.visions_note(
            "question_stranded",
            json!({"thread_id": owner.thread_id, "question_id": question_id, "posted": posted}),
        );
    }

    /// A Vision stops the run of a chat it owns. Unlike the person's Stop, this holds nothing
    /// back from the Vision.
    pub(crate) fn visions_stop(&self, thread_id: &str) -> Result<(), String> {
        self.stop_run(thread_id)?;
        self.visions_note("vision_stopped_chat", json!({"thread_id": thread_id}));
        Ok(())
    }
}
