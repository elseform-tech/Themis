//! Per-chat state: subagent labels, questions, the message queue and goals.

use serde::{Deserialize, Serialize};

/// Which subagent raised an approval (`ApprovalRequest.subagent`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubagentLabel {
    pub id: String,
    pub name: String,
}

/// A question the agent asked the user (`questions/<chat id>.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Question {
    pub id: String,
    /// The run that asked it.
    pub run_id: String,
    pub question: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<String>,
    /// The agent's recommended answer (one of the choices, or its suggestion),
    /// which it works on until the answer comes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assumption: Option<String>,
    /// Each answer's hint, by answer: one paragraph on what picking it means (what happens,
    /// advantages, disadvantages, consequences); the recommended one's may say why.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub reasons: std::collections::BTreeMap<String, String>,
    /// The questions asked together in one `ask_user` call share this id: one box shows
    /// them as pages and sends their answers together. Absent for a question asked alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// Nothing more could be done without the answer: the run ended on it.
    #[serde(default)]
    pub blocking: bool,
    /// Asking again on an open topic updates this question.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    /// The subagent that asked, by id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagent: Option<String>,
    /// Unix milliseconds.
    pub asked_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
}

/// A message waiting in a chat's queue for the current run to end.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueuedMessage {
    pub id: String,
    pub text: String,
    #[serde(default)]
    pub attachments: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    /// Being edited, or refused (see `note`): the queue skips it until it is saved.
    #[serde(default)]
    pub held: bool,
    /// Why the server held it (it could not be sent); cleared by an edit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// The Vision that queued it; absent for the person's. Sent, it is not the person's
    /// message (hooks may not wake the chat again for it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vision_id: Option<String>,
}

impl QueuedMessage {
    /// A new waiting message (not held), the person's unless `vision_id` names a Vision.
    #[must_use]
    pub fn new(
        text: String,
        attachments: Vec<String>,
        reasoning_effort: Option<String>,
        vision_id: Option<String>,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            text,
            attachments,
            reasoning_effort,
            held: false,
            note: None,
            vision_id,
        }
    }
}

/// Where a chat's goal stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalStatus {
    /// Turns keep starting until the goal is reached.
    Running,
    /// Paused by the user; Resume continues.
    Paused,
    /// Waiting for the user's answer; their reply resumes it.
    Waiting,
    /// The agent marked it blocked after the same blocker held for three goal turns; Resume
    /// continues.
    Blocked,
    /// The model quota is used up; it retries on its own.
    Quota,
    /// The agent marked it complete, with evidence for each requirement.
    Reached,
}

/// A chat's goal (`goals/<chat id>.json` in the app data dir).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Goal {
    pub text: String,
    pub status: GoalStatus,
    /// Why it paused or is blocked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Unix milliseconds.
    pub started_at: i64,
    /// Running time already counted, in milliseconds.
    #[serde(default)]
    pub active_ms: u64,
    /// Unix milliseconds since the goal last started running.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub running_since: Option<i64>,
    /// Goal turns started so far.
    #[serde(default)]
    pub turns: u32,
    /// Consecutive failed turns (sets the retry wait).
    #[serde(default)]
    pub failures: u32,
    /// History position when the current goal turn started: what the person wrote after it
    /// lets the agent pause the goal.
    #[serde(default)]
    pub turn_seq: i64,
    /// Where the goal turn before it started: words the person sent between turns (folded into
    /// the goal as "Also asked") still let the agent pause on the next turn.
    #[serde(default)]
    pub prev_turn_seq: i64,
    /// History position when the goal began: where what the work ran into starts.
    #[serde(default)]
    pub start_seq: i64,
    /// What the agent named as proof of each requirement when it marked the goal complete.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<themis_core::goal::Evidence>,
    /// Each message the person sent while the goal ran, word for word; part of the objective
    /// every goal run reads, so compaction can't lose it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub also_asked: Vec<String>,
    /// The person edited the text since the last goal run: the next one is told once.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub objective_updated: bool,
    /// Unix milliseconds when the next turn is due, while the goal waits to retry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_at: Option<i64>,
}

/// The kind of a goal note in the chat.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalNoteKind {
    Reached,
    /// A goal check's "not yet" in chats from before goals judged themselves.
    NotYet,
    Blocked,
    Waiting,
    Quota,
    Failed,
    /// What the goal is doing now and why: started, paused, resumed, removed, carrying on.
    Status,
    /// The goal started with a small context window (a warning; the goal runs on).
    SmallWindow,
}
