//! The person's Visions methods: profiles, channels and their logs, the chats Visions own, and
//! Settings › Visions. None of them is open to an agent's credential. Every write announces
//! `visions-changed` with the channel and Vision it touched.

use super::*;
use crate::state::{Channel, Vision};
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

/// Runs one of [`METHODS`], then announces what it wrote.
pub(super) async fn dispatch(
    state: &AppState,
    sink: &Arc<dyn EventSink>,
    method: &str,
    args: &Value,
) -> Result<Value, String> {
    let (value, changed) = run(state, sink, method, args).await?;
    if let Some(changed) = changed {
        sink.emit_visions_changed(&changed);
    }
    Ok(value)
}

async fn run(
    state: &AppState,
    sink: &Arc<dyn EventSink>,
    method: &str,
    args: &Value,
) -> Result<(Value, Option<VisionsChange>), String> {
    match method {
        "list_visions" | "save_vision" | "pause_vision" | "delete_vision" => {
            vision_method(state, method, args)
        }
        "list_channels" | "save_channel" | "delete_channel" | "get_channel_page" => {
            channel_method(state, method, args)
        }
        "get_visions_settings" | "update_visions_settings" => settings_method(state, method, args),
        _ => chat_method(state, sink, method, args).await,
    }
}

fn vision_method(
    state: &AppState,
    method: &str,
    args: &Value,
) -> Result<(Value, Option<VisionsChange>), String> {
    match method {
        "save_vision" => {
            let revision: Option<String> = arg(args, "revision")?;
            let saved = state.vision_save(arg::<Vision>(args, "vision")?, revision.as_deref())?;
            let changed = change(None, Some(saved.id.clone()));
            Ok((to_value(saved)?, changed))
        }
        "pause_vision" => {
            let vision_id: String = arg(args, "visionId")?;
            let saved = state.visions_pause(&vision_id, arg(args, "paused")?)?;
            Ok((to_value(saved)?, change(None, Some(vision_id))))
        }
        "delete_vision" => {
            let vision_id: String = arg(args, "visionId")?;
            state.visions_delete(&vision_id)?;
            Ok((Value::Null, change(None, Some(vision_id))))
        }
        _ => Ok((to_value(state.visions_list())?, None)),
    }
}

fn channel_method(
    state: &AppState,
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
            state.visions_delete_channel(&channel_id)?;
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
            Ok((to_value(state.visions_project_channels(&root)?)?, None))
        }
    }
}

fn settings_method(
    state: &AppState,
    method: &str,
    args: &Value,
) -> Result<(Value, Option<VisionsChange>), String> {
    if method == "get_visions_settings" {
        let (settings, revision) = state.user_visions_view()?;
        return Ok((json!({"settings": settings, "revision": revision}), None));
    }
    let changes: serde_json::Map<String, Value> = arg(args, "changes")?;
    let revision: Option<String> = arg(args, "revision")?;
    let (settings, revision) = state.patch_user_visions_config(changes, revision.as_deref())?;
    let value = json!({"settings": settings, "revision": revision});
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
            let seq = state.visions_post(sink, &channel_id, &arg::<String>(args, "text")?)?;
            Ok((json!({"seq": seq}), change(Some(channel_id), None)))
        }
        "list_channel_threads" => {
            let channel_id: String = arg(args, "channelId")?;
            let threads = state.visions_channel_threads(&channel_id).await?;
            Ok((to_value(threads)?, None))
        }
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

/// For the person's chat methods that can change a Vision's chat (stop, send, discard): the
/// chat's channel and Vision, read before the call, to announce after it succeeds.
pub(super) fn owned_chat(
    state: &AppState,
    method: &str,
    thread_id: Option<&str>,
) -> Option<VisionsChange> {
    if !matches!(method, "stop_thread" | "send_message" | "discard_thread") {
        return None;
    }
    let owner = state.owner_of(thread_id?)?;
    change(Some(owner.channel_id), Some(owner.vision_id))
}
