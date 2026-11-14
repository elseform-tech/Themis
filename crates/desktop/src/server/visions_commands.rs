//! The person's Visions methods: profiles, channels and their logs, the chats Visions own, and
//! Settings › Visions. None of them is open to an agent's credential. Every write announces
//! `visions-changed` with the channel and Vision it touched.

use super::*;
use crate::state::{Channel, OwnedKind, Vision};
use crate::types::VisionsChange;

/// A channel page holds this many bytes of log lines unless the caller asks for fewer.
const DEFAULT_PAGE_BYTES: usize = 64 * 1024;
/// The most a caller may ask a channel page to hold.
const MAX_CHANNEL_PAGE_BYTES: usize = 1024 * 1024;

/// The methods this module answers.
pub(super) const METHODS: &[&str] = &[
    "list_visions",
    "save_vision",
    "pause_vision",
    "delete_vision",
    "list_channels",
    "save_channel",
    "delete_channel",
    "get_channel_page",
    "post_channel_message",
    "list_channel_threads",
    "hand_back_thread",
    "answer_channel_request",
    "list_routines",
    "set_routine_enabled",
    "delete_routine",
    "get_team_log",
    "get_channel_spend",
    "get_visions_settings",
    "update_visions_settings",
];

fn change(channel_id: Option<String>, vision_id: Option<String>) -> Option<VisionsChange> {
    Some(VisionsChange {
        channel_id,
        vision_id,
    })
}

fn to_value(value: impl Serialize) -> Result<Value, String> {
    serde_json::to_value(value).map_err(|err| err.to_string())
}

/// The person's calls on a team's routines and on a request their chief left open.
fn team_method(
    state: &AppState,
    sink: &Arc<dyn EventSink>,
    method: &str,
    args: &Value,
) -> Result<(Value, Option<VisionsChange>), String> {
    match method {
        "get_team_log" | "get_channel_spend" => team_read(state, method, args),
        "list_routines" | "set_routine_enabled" | "delete_routine" => {
            routine_method(state, method, args)
        }
        _ => {
            let request_id: String = arg(args, "requestId")?;
            state.visions_person_answers_request(sink, &request_id, arg(args, "accept")?)?;
            Ok((Value::Null, change(None, None)))
        }
    }
}

/// The person's reads of a team: a worker's thread with its chief, the channel's spend today.
fn team_read(
    state: &AppState,
    method: &str,
    args: &Value,
) -> Result<(Value, Option<VisionsChange>), String> {
    let channel_id: String = arg(args, "channelId")?;
    if method == "get_channel_spend" {
        let spent = state.visions_channel_spent_today(&channel_id);
        return Ok((json!({"spent_usd": spent}), None));
    }
    let worker: String = arg(args, "workerId")?;
    let before: Option<u64> = arg(args, "beforeSeq")?;
    let page = state.team_page(&channel_id, &worker, before, 64 * 1024)?;
    Ok((to_value(page)?, None))
}

/// The person's view of a channel's routines: listed, switched on or off, removed.
fn routine_method(
    state: &AppState,
    method: &str,
    args: &Value,
) -> Result<(Value, Option<VisionsChange>), String> {
    if method == "list_routines" {
        let channel_id: String = arg(args, "channelId")?;
        return Ok((to_value(state.visions_routines(&channel_id))?, None));
    }
    let routine_id: String = arg(args, "routineId")?;
    if method == "set_routine_enabled" {
        state.visions_set_routine_enabled(&routine_id, arg(args, "enabled")?)?;
    } else {
        state.visions_delete_routine(&routine_id)?;
    }
    Ok((Value::Null, change(None, None)))
}

/// Runs one of [`METHODS`], then announces what it wrote.
pub(super) async fn dispatch(
    state: &AppState,
    sink: &Arc<dyn EventSink>,
    method: &str,
    args: &Value,
) -> Result<Value, String> {
    let released = released_by(state, method, args);
    let (value, changed) = run(state, sink, method, args).await?;
    if let Some(changed) = changed {
        sink.emit_visions_changed(&changed);
    }
    // Plain chat drops their Vision's mark.
    for id in released {
        super::commands::announce_thread(state, sink.as_ref(), ThreadChangeKind::Updated, id).await;
    }
    Ok(value)
}

/// The work chats deleting a Vision or a channel makes ordinary again, read before the call.
fn released_by(state: &AppState, method: &str, args: &Value) -> Vec<String> {
    let (vision, channel) = match method {
        "delete_vision" => (arg::<String>(args, "visionId").ok(), None),
        "delete_channel" => (None, arg::<String>(args, "channelId").ok()),
        _ => return Vec::new(),
    };
    state
        .owners_where(|owner| {
            owner.kind == OwnedKind::Work
                && (vision.as_deref() == Some(owner.vision_id.as_str())
                    || channel.as_deref() == Some(owner.channel_id.as_str()))
        })
        .into_iter()
        .map(|owner| owner.thread_id)
        .collect()
}

async fn run(
    state: &AppState,
    sink: &Arc<dyn EventSink>,
    method: &str,
    args: &Value,
) -> Result<(Value, Option<VisionsChange>), String> {
    match method {
        "list_visions" => Ok((to_value(state.visions_listed().await)?, None)),
        "save_vision" => {
            let revision: Option<String> = arg(args, "revision")?;
            let saved = state
                .visions_save(arg::<Vision>(args, "vision")?, revision.as_deref())
                .await?;
            let changed = change(None, Some(saved.id.clone()));
            Ok((to_value(saved)?, changed))
        }
        "pause_vision" | "delete_vision" => vision_method(state, sink, method, args),
        "list_channels" | "save_channel" | "delete_channel" | "get_channel_page" => {
            channel_method(state, sink, method, args)
        }
        "get_visions_settings" | "update_visions_settings" => settings_method(state, method, args),
        _ => chat_method(state, sink, method, args).await,
    }
}

fn vision_method(
    state: &AppState,
    sink: &Arc<dyn EventSink>,
    method: &str,
    args: &Value,
) -> Result<(Value, Option<VisionsChange>), String> {
    match method {
        "pause_vision" => {
            let vision_id: String = arg(args, "visionId")?;
            let saved = state.visions_pause(&vision_id, arg(args, "paused")?)?;
            Ok((to_value(saved)?, change(None, Some(vision_id))))
        }
        "delete_vision" => {
            let vision_id: String = arg(args, "visionId")?;
            state.visions_delete(sink, &vision_id)?;
            // Its chats become ordinary and leave every channel's list: everything is read.
            Ok((Value::Null, change(None, None)))
        }
        _ => Err(format!("unknown server method '{method}'")),
    }
}

fn channel_method(
    state: &AppState,
    sink: &Arc<dyn EventSink>,
    method: &str,
    args: &Value,
) -> Result<(Value, Option<VisionsChange>), String> {
    match method {
        "save_channel" => {
            let revision: Option<String> = arg(args, "revision")?;
            let saved = state
                .visions_save_channel(arg::<Channel>(args, "channel")?, revision.as_deref())?;
            let changed = change(Some(saved.id.clone()), None);
            Ok((to_value(saved)?, changed))
        }
        "delete_channel" => {
            let channel_id: String = arg(args, "channelId")?;
            state.visions_delete_channel(sink, &channel_id)?;
            Ok((Value::Null, change(Some(channel_id), None)))
        }
        "get_channel_page" => {
            let max_bytes = arg::<Option<usize>>(args, "maxBytes")?
                .unwrap_or(DEFAULT_PAGE_BYTES)
                .clamp(1, MAX_CHANNEL_PAGE_BYTES);
            let page = state.channel_page(
                &arg::<String>(args, "channelId")?,
                arg(args, "beforeSeq")?,
                max_bytes,
            )?;
            Ok((to_value(page)?, None))
        }
        _ => {
            let root: String = arg(args, "projectRoot")?;
            Ok((to_value(state.visions_channels_listed(&root)?)?, None))
        }
    }
}

fn settings_method(
    state: &AppState,
    method: &str,
    args: &Value,
) -> Result<(Value, Option<VisionsChange>), String> {
    // Whether Visions can run on this computer at all, beside the settings.
    let can_run = state.visions_can_run_here();
    if method == "get_visions_settings" {
        let (settings, revision) = state.user_visions_view()?;
        let value = json!({"settings": settings, "revision": revision, "can_run": can_run});
        return Ok((value, None));
    }
    let changes: serde_json::Map<String, Value> = arg(args, "changes")?;
    let revision: Option<String> = arg(args, "revision")?;
    let (settings, revision) = state.patch_user_visions_config(changes, revision.as_deref())?;
    let value = json!({"settings": settings, "revision": revision, "can_run": can_run});
    Ok((value, Some(VisionsChange::default())))
}

async fn chat_method(
    state: &AppState,
    sink: &Arc<dyn EventSink>,
    method: &str,
    args: &Value,
) -> Result<(Value, Option<VisionsChange>), String> {
    match method {
        "post_channel_message" => {
            let channel_id: String = arg(args, "channelId")?;
            let (seq, woken) =
                state.visions_post(sink, &channel_id, &arg::<String>(args, "text")?)?;
            let posted = json!({"seq": seq, "woken": woken});
            Ok((posted, change(Some(channel_id), None)))
        }
        "list_channel_threads" => {
            let channel_id: String = arg(args, "channelId")?;
            let threads = state.visions_channel_threads(&channel_id).await?;
            Ok((to_value(threads)?, None))
        }
        "list_routines"
        | "set_routine_enabled"
        | "delete_routine"
        | "answer_channel_request"
        | "get_team_log"
        | "get_channel_spend" => team_method(state, sink, method, args),
        "hand_back_thread" => {
            let owner = state.visions_hand_back(sink, &arg::<String>(args, "threadId")?)?;
            let changed = change(
                Some(owner.channel_id.clone()),
                Some(owner.vision_id.clone()),
            );
            Ok((to_value(owner)?, changed))
        }
        _ => Err(format!("unknown server method '{method}'")),
    }
}

/// For the person's chat methods that can change a Vision's chat (stop, send, queue, discard) or what
/// waits on the person there (an answer, an approval): the chat's channel and Vision, read
/// before the call, to announce after it succeeds.
pub(super) fn owned_chat(
    state: &AppState,
    method: &str,
    thread_id: Option<&str>,
) -> Option<VisionsChange> {
    let touches = matches!(
        method,
        "stop_thread"
            | "send_message"
            | "enqueue_message"
            | "discard_thread"
            | "answer_question"
            | "approve_action"
    );
    if !touches {
        return None;
    }
    let owner = state.owner_of(thread_id?)?;
    change(Some(owner.channel_id), Some(owner.vision_id))
}
