//! What a Vision does to the chats it owns through its run credential: start a work chat in its
//! channel's project, send, steer and stop. Queueing is `enqueue` with a message carrying the
//! Vision's id. None of it is the person's message: the sends go the server's way
//! (`send_from_server`), so a Vision never takes a chat over, answers no open question by
//! writing and resets no hook wakes. The app server checks ownership, holds and pauses before
//! any of these run (`server/agent/vision.rs`). Logged as metadata only.

use std::sync::Arc;

use serde_json::json;
use themis_core::agent_cli::VisionScope;

use super::{ChannelEntry, Hold, OwnedKind, Owner};
use crate::sink::EventSink;
use crate::state::AppState;
use crate::types::{ProviderKind, RunHandle, ThreadInfo};

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
        let recorded = self
            .visions_adopt(vision, &info, channel.approval_mode)
            .await;
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
            json!({"thread_id": info.id, "vision_id": vision.vision_id, "channel_id": vision.channel_id, "mode": channel.approval_mode}),
        );
        self.get_thread(&info.id).await
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

    /// A Vision's message in a chat it owns, sent the server's way.
    pub(crate) async fn visions_send(
        &self,
        sink: Arc<dyn EventSink>,
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
        let run_id = uuid::Uuid::new_v4().to_string();
        let (sent, _) = self
            .send_from_server(
                sink,
                thread_id,
                text,
                &run_id,
                reasoning_effort,
                attachments,
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
    /// or its Vision is paused or gone.
    pub(crate) fn visions_queue_waits(&self, thread_id: &str) -> bool {
        self.visions_waits_reason(thread_id).is_some()
    }

    /// Why a Vision may not write in `thread_id` now (the answer it gets), if it may not: the
    /// person holds the chat, or its Vision is paused, gone or no longer owns it.
    fn visions_waits_reason(&self, thread_id: &str) -> Option<&'static str> {
        use themis_core::prompts::{AGENT_CLI_HELD_BY_PERSON, AGENT_CLI_VISION_PAUSED};
        let Some(owner) = self.owner_of(thread_id) else {
            return Some(AGENT_CLI_HELD_BY_PERSON);
        };
        if owner.hold != Hold::None {
            return Some(AGENT_CLI_HELD_BY_PERSON);
        }
        let paused = self
            .vision_get(&owner.vision_id)
            .is_none_or(|vision| vision.paused);
        paused.then_some(AGENT_CLI_VISION_PAUSED)
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

    /// A Vision answers questions in a chat it owns: never as the person, so no "Allow once"
    /// it writes lets anything run.
    pub(crate) async fn visions_answer(
        &self,
        sink: Arc<dyn EventSink>,
        thread_id: String,
        answers: Vec<(String, String)>,
    ) -> Result<Vec<crate::types::Question>, String> {
        self.answer_questions_by(sink, thread_id, answers, false)
            .await
    }

    /// A Vision stops the run of a chat it owns. Unlike the person's Stop, this holds nothing
    /// back from the Vision.
    pub(crate) fn visions_stop(&self, thread_id: &str) -> Result<(), String> {
        self.stop_run(thread_id)?;
        self.visions_note("vision_stopped_chat", json!({"thread_id": thread_id}));
        Ok(())
    }
}
