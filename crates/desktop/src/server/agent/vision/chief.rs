//! The team grants. A chief manages its workers: it tasks, redirects, pauses and resumes them,
//! reads their team logs and work chats, and answers a question in a worker's chat within its
//! channel's level; it drives no chat itself. A worker keeps Phase 1's grant on its own work
//! chats and reports or pushes back to its chief through its team log. Each is decided on the
//! request (method, ids) and the owner records here; whether a named worker is in the chief's
//! channel, and whether the chief still leads it, is the server's state's to say
//! ([`super::state_refusal`]).

use super::*;
use crate::state::{TeamEntry, WakeReason};

/// A chief's own calls.
pub(in crate::server::agent) const CHIEF_CALLS: &[&str] = &[
    "assign_task",
    "redirect_worker",
    "pause_worker",
    "resume_worker",
    "get_team_log",
    "list_team",
    "request_channel",
    "answer_request",
    "ask_for_worker",
    "list_routines",
    "save_routine",
    "delete_routine",
];

/// A worker's own calls, to its chief.
pub(in crate::server::agent) const WORKER_CALLS: &[&str] = &["report", "push_back"];

/// The team calls that name a worker (`workerId`).
pub(super) const NAMES_WORKER: &[&str] = &[
    "assign_task",
    "redirect_worker",
    "pause_worker",
    "resume_worker",
    "get_team_log",
];

/// What a chief may do on its workers' chats beyond reading them.
const ON_WORKER_CHAT: &[&str] = &["answer_question", "pass_question", "list_questions"];

/// The team grant's decision, like [`super::refusal`]'s: `None` hands the call on to the Vision
/// The team grant's decision, like [`super::refusal`]'s: `None` hands the call on to the Vision
/// grant's own rules (a worker's driving of its chats, everyone's ordinary reads).
pub(super) fn refusal(
    scope: &VisionScope,
    method: &str,
    args: &Value,
    owned: &dyn Fn(&str) -> Option<Owner>,
) -> Option<Option<Refusal>> {
    let names_channel = args
        .get("channelId")
        .and_then(Value::as_str)
        .is_some_and(|channel| channel != scope.channel_id);
    if !scope.chief {
        return worker_refusal(scope, method, args, names_channel);
    }
    // A request names the other channel by design; the server checks it is one of this project.
    if CHIEF_CALLS.contains(&method) && (!names_channel || method == "request_channel") {
        return Some(None);
    }
    if CHIEF_CALLS.contains(&method) || WORKER_CALLS.contains(&method) || method == "create_thread"
    {
        return not_allowed(method);
    }
    on_worker_chat(scope, method, args, owned)
}

fn not_allowed(method: &str) -> Option<Option<Refusal>> {
    Some(Some(refused(
        Reason::MethodNotAllowed,
        method_label(method),
    )))
}

/// A worker's team calls: its own report, pushback and team log, never a chief's call. Anything
/// else goes by the Vision grant's rules on its own chats.
fn worker_refusal(
    scope: &VisionScope,
    method: &str,
    args: &Value,
    names_channel: bool,
) -> Option<Option<Refusal>> {
    if WORKER_CALLS.contains(&method) {
        return Some(None);
    }
    if method == "get_team_log" {
        let own = args
            .get("workerId")
            .and_then(Value::as_str)
            .is_none_or(|worker| worker == scope.vision_id);
        return if own && !names_channel {
            Some(None)
        } else {
            not_allowed(method)
        };
    }
    if CHIEF_CALLS.contains(&method) {
        return not_allowed(method);
    }
    None
}

/// A chief's call on a chat: reads and question handling on its own channel's workers' work
/// chats; driving any chat is refused.
fn on_worker_chat(
    scope: &VisionScope,
    method: &str,
    args: &Value,
    owned: &dyn Fn(&str) -> Option<Owner>,
) -> Option<Option<Refusal>> {
    let reads = OWN_CHAT.contains(&method);
    if !reads && !VISION_DRIVE.contains(&method) {
        return None;
    }
    let thread = args.get("threadId").and_then(Value::as_str);
    let workers = thread.and_then(owned).filter(|owner| {
        owner.channel_id == scope.channel_id
            && owner.vision_id != scope.vision_id
            && owner.kind == OwnedKind::Work
    });
    let Some(owner) = workers else {
        // Not its worker's chat: reads (and its channel's chat list) go by the Vision grant's
        // ordinary rules; driving any other chat is refused, even one it owned before it led.
        return if reads || method == "list_channel_threads" {
            None
        } else {
            not_allowed(method)
        };
    };
    if reads {
        return Some(None);
    }
    if !ON_WORKER_CHAT.contains(&method) {
        return not_allowed(method);
    }
    if held(owner.hold, method) {
        return Some(Some(held_by_person()));
    }
    Some(
        (method == "answer_question" && above_autonomy(scope, args)).then(|| Refusal {
            reason: Reason::AboveAutonomy,
            message: themis_core::prompts::AGENT_CLI_ABOVE_AUTONOMY.to_owned(),
        }),
    )
}

/// Most bytes of a team log a page carries.
const PAGE_BYTES: usize = 32 * 1024;
/// Most characters of a task, a redirect, a report or a pushback.
const MAX_TEXT_CHARS: usize = 4000;

/// What the server's state refuses a team call beyond the pure rules: a chief that no longer
/// leads its channel or names someone not on its team, a paused chief giving work, and a
/// worker no longer on the team.
pub(super) fn state_refusal(
    state: &AppState,
    scope: &VisionScope,
    method: &str,
    args: &Value,
) -> Option<Refusal> {
    let not_yours = || Refusal {
        reason: Reason::NotYourTeam,
        message: themis_core::prompts::AGENT_CLI_NOT_YOUR_TEAM.to_owned(),
    };
    if WORKER_CALLS.contains(&method) || (method == "get_team_log" && !scope.chief) {
        return (!state.works_in(&scope.channel_id, &scope.vision_id)).then(not_yours);
    }
    if !CHIEF_CALLS.contains(&method) {
        return None;
    }
    if !state.leads(&scope.channel_id, &scope.vision_id) {
        return Some(not_yours());
    }
    if NAMES_WORKER.contains(&method) {
        let worker = args
            .get("workerId")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !state.works_in(&scope.channel_id, worker) {
            return Some(not_yours());
        }
    }
    // Declining a request changes nothing, so a paused chief may still do it.
    let accepts = method == "answer_request" && args.get("accept") == Some(&Value::Bool(true));
    let gives_work = accepts
        || matches!(
            method,
            "assign_task"
                | "redirect_worker"
                | "pause_worker"
                | "resume_worker"
                | "request_channel"
                | "ask_for_worker"
                | "save_routine"
        );
    let paused = state
        .vision_get(&scope.vision_id)
        .is_none_or(|vision| vision.paused);
    (gives_work && paused).then(|| Refusal {
        reason: Reason::Paused,
        message: themis_core::prompts::AGENT_CLI_VISION_PAUSED.to_owned(),
    })
}

/// A run's role is captured when it starts: once the channel's chief changed, the run may no
/// longer drive, manage or read the team's work chats as before (a demoted chief, or a worker
/// that now leads). Its own chat's reads and status stay.
pub(super) fn role_changed(
    state: &AppState,
    scope: &VisionScope,
    method: &str,
    args: &Value,
) -> Option<Refusal> {
    let team_call = VISION_DRIVE.contains(&method)
        || CHIEF_CALLS.contains(&method)
        || WORKER_CALLS.contains(&method);
    let work_read = OWN_CHAT.contains(&method)
        && args
            .get("threadId")
            .and_then(Value::as_str)
            .and_then(|thread| state.owner_of(thread))
            .is_some_and(|owner| owner.kind == OwnedKind::Work);
    let changed =
        (team_call || work_read) && state.leads(&scope.channel_id, &scope.vision_id) != scope.chief;
    changed.then(|| Refusal {
        reason: Reason::NotYourTeam,
        message: themis_core::prompts::AGENT_CLI_NOT_YOUR_TEAM.to_owned(),
    })
}

/// Some text, at most [`MAX_TEXT_CHARS`] characters.
fn text_arg(args: &Value, key: &str) -> Result<String, String> {
    let text: String = arg(args, key)?;
    let text = text.trim();
    if text.is_empty() || text.chars().count() > MAX_TEXT_CHARS {
        return Err(format!(
            "`{key}` is some text of at most {MAX_TEXT_CHARS} characters"
        ));
    }
    Ok(text.to_owned())
}

/// Runs a team call the grant let through, then announces the channel changed.
pub(super) async fn dispatch(
    state: &AppState,
    sink: &Arc<dyn EventSink>,
    scope: &VisionScope,
    method: &str,
    args: &Value,
) -> Result<Value, String> {
    let channel = scope.channel_id.as_str();
    let result = match method {
        "list_team" => to_value(state.team_members(channel)),
        "get_team_log" => {
            let worker: Option<String> = arg(args, "workerId")?;
            let worker = worker.unwrap_or_else(|| scope.vision_id.clone());
            let before: Option<u64> = arg(args, "beforeSeq")?;
            to_value(state.team_page(channel, &worker, before, PAGE_BYTES)?)
        }
        "report" | "push_back" => {
            let entry = match method {
                "report" => TeamEntry::Report {
                    text: text_arg(args, "text")?,
                },
                _ => TeamEntry::Pushback {
                    text: text_arg(args, "text")?,
                    safer: text_arg(args, "safer")?,
                },
            };
            let seq = state.team_append(channel, &scope.vision_id, entry)?;
            if let Some(chief) = state.channel_get(channel).map(|channel| channel.chief) {
                let note = WakeReason::WorkerNote(scope.vision_id.clone(), seq);
                state.visions_wake(sink, channel, &chief, note);
            }
            Ok(json!({"seq": seq}))
        }
        "request_channel" | "answer_request" | "ask_for_worker" => {
            request(state, sink, scope, method, args)
        }
        "list_routines" | "save_routine" | "delete_routine" => routine(state, scope, method, args),
        _ => to_worker(state, sink, scope, method, args).await,
    }?;
    sink.emit_visions_changed(&VisionsChange {
        channel_id: Some(scope.channel_id.clone()),
        vision_id: Some(scope.vision_id.clone()),
    });
    Ok(result)
}

/// The channel a request names: by its name in the asking channel's project (a chief knows
/// names, not ids), else by `channelId`.
fn channel_named(state: &AppState, channel: &str, args: &Value) -> Result<String, String> {
    let Some(name) = arg::<Option<String>>(args, "channel")? else {
        return arg(args, "channelId");
    };
    let root = state
        .channel_get(channel)
        .map(|own| own.project_root)
        .ok_or("Unknown channel")?;
    let name = name.trim().trim_start_matches('#').to_owned();
    state
        .channel_list(&root)
        .into_iter()
        .find(|other| other.name == name)
        .map(|other| other.id)
        .ok_or_else(|| format!("This project has no channel named {name:?}"))
}

/// A chief's request to another channel of its project, or its answer to one it received.
fn request(
    state: &AppState,
    sink: &Arc<dyn EventSink>,
    scope: &VisionScope,
    method: &str,
    args: &Value,
) -> Result<Value, String> {
    let channel = scope.channel_id.as_str();
    if method == "ask_for_worker" {
        let role: String = arg(args, "role")?;
        let why: String = arg(args, "why")?;
        let id = state.visions_ask_for_worker(sink, channel, &scope.vision_id, &role, &why)?;
        return Ok(json!({"requestId": id}));
    }
    if method == "request_channel" {
        let to = channel_named(state, channel, args)?;
        let ask: String = arg(args, "ask")?;
        let id = state.visions_request(sink, channel, &scope.vision_id, &to, &ask)?;
        return Ok(json!({"requestId": id}));
    }
    let id: String = arg(args, "requestId")?;
    let note: Option<String> = arg(args, "note")?;
    let note = note.unwrap_or_default();
    state.visions_answer_request(
        sink,
        channel,
        &scope.vision_id,
        &id,
        arg(args, "accept")?,
        &note,
    )?;
    Ok(json!({"answered": true}))
}

/// A chief's routines: listed, added or changed (its `undoable` decision), or removed.
fn routine(
    state: &AppState,
    scope: &VisionScope,
    method: &str,
    args: &Value,
) -> Result<Value, String> {
    let channel = scope.channel_id.as_str();
    match method {
        "list_routines" => to_value(state.visions_routines(channel)),
        "save_routine" => {
            let routine: crate::state::Routine = arg(args, "routine")?;
            let now = state.visions_clock();
            to_value(state.visions_save_routine(channel, &scope.vision_id, routine, now)?)
        }
        _ => {
            let id: String = arg(args, "routineId")?;
            state.visions_chief_deletes_routine(&scope.vision_id, &id)?;
            Ok(Value::Null)
        }
    }
}

/// A chief's word to a worker: a task or a redirect (which wake it), or a pause or a resume.
async fn to_worker(
    state: &AppState,
    sink: &Arc<dyn EventSink>,
    scope: &VisionScope,
    method: &str,
    args: &Value,
) -> Result<Value, String> {
    let channel = scope.channel_id.as_str();
    let worker: String = arg(args, "workerId")?;
    let entry = match method {
        "assign_task" => TeamEntry::Task {
            from_chief: scope.vision_id.clone(),
            text: text_arg(args, "task")?,
        },
        "redirect_worker" => TeamEntry::Redirect {
            text: text_arg(args, "text")?,
        },
        "pause_worker" => {
            state.visions_team_pause(channel, &worker, true)?;
            TeamEntry::Paused
        }
        _ => {
            state.visions_team_pause(channel, &worker, false)?;
            TeamEntry::Resumed
        }
    };
    let wakes = matches!(entry, TeamEntry::Task { .. } | TeamEntry::Redirect { .. });
    let seq = state.team_append(channel, &worker, entry)?;
    if wakes {
        state.visions_wake(sink, channel, &worker, WakeReason::Tasked(seq));
    }
    Ok(json!({"seq": seq}))
}

#[cfg(test)]
#[path = "chief_tests.rs"]
mod tests;
