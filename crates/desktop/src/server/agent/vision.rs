//! The Vision grant: what a Vision's run credential may do beyond an ordinary run's.
//!
//! It may start a work chat in its channel's project, and send, queue, steer, stop and answer
//! questions only in the work chats its Vision owns, plus the chat reads on them. Ownership is
//! the owner record of the request's `threadId`, never command text. While the person holds a
//! chat (stopped or took it over) the Vision may not write in it, and a paused Vision starts
//! and sends nothing. Everything else stays as refused as for any run: approvals, modes,
//! settings, secrets, other chats. A Vision's writes go the server's way, never as the
//! person's message (`state/visions/drive.rs`). It answers a question only on a decision its
//! run's autonomy leaves to it (`decision`, with its `reason`), and passes the rest on to the
//! person in its channel (`pass_question`).

use themis_core::visions::Decision;

use super::*;
use crate::state::{Hold, OwnedKind};
use crate::types::{QueuedMessage, ThreadChangeKind, VisionsChange};

/// Most characters of the reason a Vision gives with its answer.
const MAX_REASON_CHARS: usize = 200;

/// The methods a Vision's grant may use to drive chats, each only as its rules below allow.
pub(super) const VISION_DRIVE: &[&str] = &[
    "create_thread",
    "send_message",
    "enqueue_message",
    "steer_queued_message",
    "stop_thread",
    "answer_question",
    "pass_question",
    "list_questions",
    "list_channel_threads",
];

/// Methods that put the Vision's words into a chat: refused while the person holds it.
const WRITES: &[&str] = &[
    "send_message",
    "enqueue_message",
    "steer_queued_message",
    "answer_question",
];

/// Methods that start or send something: refused while the Vision is paused.
const STARTS: &[&str] = &[
    "create_thread",
    "send_message",
    "enqueue_message",
    "steer_queued_message",
    "answer_question",
    "pass_question",
];

/// The Vision grant's own decision on a call: `Some` when its rules decide (the inner value is
/// the refusal, if any), `None` when the ordinary rules of a run's credential do.
pub(super) fn refusal(
    scope: &VisionScope,
    method: &str,
    args: &Value,
    owned: &dyn Fn(&str) -> Option<Owner>,
) -> Option<Option<Refusal>> {
    match method {
        "create_thread" => return Some((!in_channel_project(scope, args)).then(other_project)),
        "list_channel_threads" => {
            let ours = args.get("channelId").and_then(Value::as_str) == Some(&scope.channel_id);
            return Some((!ours).then(|| refused(Reason::MethodNotAllowed, method)));
        }
        _ => {}
    }
    let drives = VISION_DRIVE.contains(&method);
    if !drives && !OWN_CHAT.contains(&method) {
        return None;
    }
    let owner = args
        .get("threadId")
        .and_then(Value::as_str)
        .and_then(owned)
        .filter(|owner| owner.vision_id == scope.vision_id && owner.kind == OwnedKind::Work);
    let Some(owner) = owner else {
        // Not one of its work chats: a driving call is refused; a read goes by the ordinary
        // rules, which allow the Vision's own chat only.
        return drives.then(|| Some(refused(Reason::MethodNotAllowed, method_label(method))));
    };
    if held(owner.hold, method) {
        return Some(Some(held_by_person()));
    }
    if matches!(method, "send_message" | "enqueue_message") && dressed_as_answer(args) {
        return Some(Some(Refusal {
            reason: Reason::AnswerMarker,
            message: themis_core::prompts::AGENT_CLI_VISION_ANSWER_MARKER.to_owned(),
        }));
    }
    Some(
        (method == "answer_question" && above_autonomy(scope, args)).then(|| Refusal {
            reason: Reason::AboveAutonomy,
            message: themis_core::prompts::AGENT_CLI_ABOVE_AUTONOMY.to_owned(),
        }),
    )
}

/// Whether a message's text starts with an answer marker, which only the server's answers carry.
fn dressed_as_answer(args: &Value) -> bool {
    args.get("text")
        .and_then(Value::as_str)
        .is_some_and(crate::state::forges_answer)
}

/// Whether an answer names a decision its run's autonomy leaves to the person. One naming no
/// known decision is refused when it runs ([`decision_arg`]).
fn above_autonomy(scope: &VisionScope, args: &Value) -> bool {
    args.get("decision")
        .and_then(Value::as_str)
        .and_then(Decision::from_id)
        .is_some_and(|decision| !scope.decides.contains(&decision))
}

/// The call's `decision`: an autonomy row id.
fn decision_arg(args: &Value) -> Result<Decision, String> {
    args.get("decision")
        .and_then(Value::as_str)
        .and_then(Decision::from_id)
        .ok_or_else(|| themis_core::prompts::AGENT_CLI_VISION_DECISION.to_owned())
}

/// The call's `reason`: some text, at most [`MAX_REASON_CHARS`] characters.
fn reason_arg(args: &Value) -> Result<String, String> {
    args.get("reason")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|reason| !reason.is_empty() && reason.chars().count() <= MAX_REASON_CHARS)
        .map(str::to_owned)
        .ok_or_else(|| themis_core::prompts::AGENT_CLI_VISION_REASON.to_owned())
}

/// Whether the person's hold keeps `method` from the Vision: writes while held in any way, and
/// a stop while the person took the chat over (its run is theirs).
fn held(hold: Hold, method: &str) -> bool {
    match hold {
        Hold::None => false,
        Hold::StoppedByPerson => WRITES.contains(&method),
        Hold::TakenOver => WRITES.contains(&method) || method == "stop_thread",
    }
}

/// Methods that would make a model spend: refused once the Vision reached today's budget.
const SPENDS: &[&str] = &[
    "create_thread",
    "send_message",
    "enqueue_message",
    "steer_queued_message",
    "answer_question",
];

/// What the server's state refuses a Vision beyond the pure rules: an answer to a permission
/// question, anything starting or sending from a paused (or deleted) Vision, and anything that
/// would spend from one over its daily budget.
pub(super) fn state_refusal(
    state: &AppState,
    scope: &VisionScope,
    method: &str,
    args: &Value,
) -> Option<Refusal> {
    if method == "answer_question" && asks_permission(state, args) {
        return Some(Refusal {
            reason: Reason::PermissionQuestion,
            message: themis_core::prompts::AGENT_CLI_PERMISSION_QUESTION.to_owned(),
        });
    }
    if !STARTS.contains(&method) {
        return None;
    }
    let paused = state
        .vision_get(&scope.vision_id)
        .is_none_or(|vision| vision.paused);
    if paused {
        return Some(Refusal {
            reason: Reason::Paused,
            message: themis_core::prompts::AGENT_CLI_VISION_PAUSED.to_owned(),
        });
    }
    if !SPENDS.contains(&method) {
        return None;
    }
    let over = state.visions_budget_gate(&scope.vision_id)?;
    let reason = match over {
        crate::state::BudgetOver::Unknown(_) => Reason::BudgetUnknown,
        _ => Reason::OverBudget,
    };
    Some(Refusal {
        reason,
        message: over.refusal_text().to_owned(),
    })
}

/// Whether an `answer_question` names a permission question; arguments it cannot read count
/// as one (the call would fail anyway).
fn asks_permission(state: &AppState, args: &Value) -> bool {
    match crate::server::commands::answer_args(args) {
        Ok((thread_id, answers, _)) => {
            state.visions_permission_question(&thread_id, answers.iter().map(|(id, _)| id.as_str()))
        }
        Err(_) => true,
    }
}

/// `projectRoot` names the channel's project, compared canonical; a call naming none is not.
fn in_channel_project(scope: &VisionScope, args: &Value) -> bool {
    args.get("projectRoot")
        .and_then(Value::as_str)
        .and_then(|named| PathBuf::from(named.trim()).canonicalize().ok())
        .is_some_and(|named| named == scope.project_root)
}

fn other_project() -> Refusal {
    Refusal {
        reason: Reason::OtherProject,
        message: themis_core::prompts::AGENT_CLI_VISION_OTHER_PROJECT.to_owned(),
    }
}

fn held_by_person() -> Refusal {
    Refusal {
        reason: Reason::HeldByPerson,
        message: themis_core::prompts::AGENT_CLI_HELD_BY_PERSON.to_owned(),
    }
}

/// Runs a Vision's allowed driving call. Starting, sending, queueing, steering and stopping
/// take the Vision's own paths (never the person's, so nothing counts as the person's message
/// or Stop); questions and the channel's chat list go through the shared dispatcher.
pub(super) async fn dispatch(
    state: &AppState,
    sink: &Arc<dyn EventSink>,
    scope: &VisionScope,
    method: &str,
    args: Value,
) -> Result<Value, String> {
    let result = match method {
        // The chat's provider and model are the Vision's, never the call's.
        "create_thread" => {
            let info = state.visions_start_work_chat(scope).await?;
            // Plain chat lists it too, marked with its Vision.
            let (kind, id) = (ThreadChangeKind::Created, info.id.clone());
            crate::server::commands::announce_thread(state, sink.as_ref(), kind, id).await;
            to_value(info)
        }
        "answer_question" | "pass_question" => question(state, sink, scope, method, &args).await,
        "send_message" | "enqueue_message" => write(state, sink, scope, method, &args).await,
        "steer_queued_message" => state
            .visions_steer(
                Arc::clone(sink),
                &scope.vision_id,
                arg(&args, "threadId")?,
                arg(&args, "messageId")?,
            )
            .await
            .map(|()| Value::Null),
        "stop_thread" => state
            .visions_stop(&arg::<String>(&args, "threadId")?)
            .map(|()| Value::Null),
        _ => return crate::server::dispatch(state, Arc::clone(sink), method, args).await,
    }?;
    sink.emit_visions_changed(&VisionsChange {
        channel_id: Some(scope.channel_id.clone()),
        vision_id: Some(scope.vision_id.clone()),
    });
    Ok(result)
}

/// A Vision's answer to a question (on a decision it may make, with its reason) or its
/// question passed on to the person.
async fn question(
    state: &AppState,
    sink: &Arc<dyn EventSink>,
    scope: &VisionScope,
    method: &str,
    args: &Value,
) -> Result<Value, String> {
    if method == "pass_question" {
        let thread_id: String = arg(args, "threadId")?;
        let question_id: String = arg(args, "questionId")?;
        let decision = decision_arg(args)?;
        return to_value(state.visions_pass_question(scope, &thread_id, &question_id, decision)?);
    }
    let (thread_id, answers, many) = crate::server::commands::answer_args(args)?;
    let terms = (decision_arg(args)?, reason_arg(args)?);
    let (answered, unrecorded) = state
        .visions_answer(Arc::clone(sink), scope, thread_id, answers, terms)
        .await?;
    let answered = crate::server::commands::answered_value(answered, many)?;
    // Delivered but not in the channel: the Vision is told, so it can say so itself.
    Ok(if unrecorded {
        json!({"answered": answered, "warning": themis_core::prompts::AGENT_CLI_VISION_UNRECORDED})
    } else {
        answered
    })
}

/// A Vision's `send_message` (sent the server's way) or `enqueue_message` (queued with its id).
async fn write(
    state: &AppState,
    sink: &Arc<dyn EventSink>,
    scope: &VisionScope,
    method: &str,
    args: &Value,
) -> Result<Value, String> {
    let thread_id: String = arg(args, "threadId")?;
    let text: String = arg(args, "text")?;
    let effort: Option<String> = arg(args, "reasoningEffort")?;
    let attachments = arg::<Option<Vec<String>>>(args, "attachments")?.unwrap_or_default();
    if method == "send_message" {
        let sent = state
            .visions_send(Arc::clone(sink), &thread_id, text, effort, attachments)
            .await?;
        return to_value(sent);
    }
    let message = QueuedMessage::new(text, attachments, effort, Some(scope.vision_id.clone()));
    to_value(state.enqueue(Arc::clone(sink), thread_id, message).await?)
}

fn to_value(value: impl Serialize) -> Result<Value, String> {
    serde_json::to_value(value).map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests;
