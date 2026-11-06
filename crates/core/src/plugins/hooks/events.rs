use super::*;

/// Most of one tool's result a `PostToolBatch` hook is told, in bytes.
const BATCH_RESPONSE_BYTES: usize = 64 * 1024;

/// Most of a checkpoint's summary a `PostCompact` hook is told, in bytes.
const COMPACT_SUMMARY_BYTES: usize = 256 * 1024;

/// A step's tool calls have all finished and the next model request hasn't gone out: this is
/// `PostToolBatch`. What it adds is returned for that request; a hook that blocks ends the turn.
pub async fn post_tool_batch(
    calls: &[(String, Value, String)],
    events: &tokio::sync::mpsc::Sender<crate::runtime::RunEvent>,
) -> anyhow::Result<Option<String>> {
    if IN_HOOK.try_with(|running| *running).unwrap_or(false) {
        return Ok(None);
    }
    let Ok(runtime) = ACTIVE.try_with(Clone::clone) else {
        return Ok(None);
    };
    if !runtime.listens("PostToolBatch") {
        return Ok(None);
    }
    let tool_calls: Vec<Value> = calls
        .iter()
        .map(|(tool, arguments, response)| {
            let response = capped(response, BATCH_RESPONSE_BYTES);
            json!({
                "tool_name": runtime.claude_name(tool),
                "tool_input": claude_tool_input(tool, arguments),
                "tool_response": serde_json::from_str(response).unwrap_or_else(|_| json!(response)),
            })
        })
        .collect();
    let outputs = runtime
        .emit_collect("PostToolBatch", json!({"tool_calls": tool_calls}), events)
        .await?;
    let mut added = String::new();
    add_context(&mut added, &outputs);
    Ok((!added.is_empty()).then(|| capped(&added, PROMPT_CONTEXT_LIMIT).to_owned()))
}

/// A checkpoint is in place: this is `PostCompact`. Themis never compacts on request, so `trigger`
/// is always `auto`.
pub async fn post_compact(
    summary: &str,
    events: &tokio::sync::mpsc::Sender<crate::runtime::RunEvent>,
) {
    let payload =
        json!({"trigger": "auto", "compact_summary": capped(summary, COMPACT_SUMMARY_BYTES)});
    emit_current("PostCompact", payload, events).await.ok();
}

/// The person is about to be asked to approve a tool call: this is `PermissionRequest`, and then
/// `Notification`. A hook may answer deny, whose message is returned, or say nothing. `allow` is
/// ignored and logged, whatever it also asks to change: hooks never grant permissions.
pub async fn permission_request(
    tool: &str,
    arguments: &Value,
    events: &tokio::sync::mpsc::Sender<crate::runtime::RunEvent>,
) -> Option<String> {
    if IN_HOOK.try_with(|running| *running).unwrap_or(false) {
        return None;
    }
    let runtime = ACTIVE.try_with(Clone::clone).ok()?;
    if runtime.listens("PermissionRequest") {
        let payload = json!({"tool": tool, "arguments": arguments, "permission_suggestions": []});
        let outputs = runtime
            .emit_collect("PermissionRequest", payload, events)
            .await
            .unwrap_or_default();
        for output in &outputs {
            let decision = &output["hookSpecificOutput"]["decision"];
            match decision["behavior"].as_str() {
                Some("deny") => {
                    return Some(
                        decision["message"]
                            .as_str()
                            .map(str::trim)
                            .filter(|message| !message.is_empty())
                            .map_or_else(
                                || "A plugin hook denied this.".to_owned(),
                                |message| capped(message, BLOCK_REASON_LIMIT).to_owned(),
                            ),
                    )
                }
                Some("allow") => crate::diagnostics::emit(
                    "hook",
                    "permission_allow_ignored",
                    "info",
                    json!({"tool": tool}),
                ),
                _ => {}
            }
        }
    }
    if runtime.listens("Notification") {
        let payload = json!({"message": "Themis needs your permission",
            "title": "Permission needed", "notification_type": "permission_prompt"});
        runtime.emit("Notification", payload, events).await.ok();
    }
    None
}

/// An approval refused a tool call: this is `PermissionDenied`. Claude's `retry` isn't read, since
/// the model is told about a refusal either way.
pub async fn permission_denied(
    tool: &str,
    arguments: &Value,
    reason: &str,
    events: &tokio::sync::mpsc::Sender<crate::runtime::RunEvent>,
) {
    let payload = json!({"tool": tool, "arguments": arguments, "reason": reason});
    emit_current("PermissionDenied", payload, events).await.ok();
}

/// Asks this run's `Stop` hooks whether its turn may end, with the reply it ended on. `active`
/// says a hook already kept it going, as Claude's `stop_hook_active` does: a hook can use it to
/// let go. A subagent's end is `SubagentEnd`, so it has none, and a hook that merely fails doesn't
/// keep anything going. The run's own turn limit and the person's Stop bound the loop.
pub async fn stop_refused(
    reply: &str,
    active: bool,
    events: &tokio::sync::mpsc::Sender<crate::runtime::RunEvent>,
) -> Option<KeepGoing> {
    if IN_HOOK.try_with(|running| *running).unwrap_or(false) {
        return None;
    }
    let runtime = ACTIVE.try_with(Clone::clone).ok()?;
    if runtime.agent.is_some() {
        return None;
    }
    runtime.note_reply(reply);
    let payload = json!({"last_assistant_message": reply, "stop_hook_active": active});
    let error = runtime.emit("Stop", payload, events).await.err()?;
    error.downcast::<KeepGoing>().ok()
}

/// The `bin` folders of this run's plugins, for its commands' `PATH`.
#[must_use]
pub fn plugin_bins() -> Vec<std::path::PathBuf> {
    ACTIVE
        .try_with(|runtime| runtime.session.plugin_bins.clone())
        .unwrap_or_default()
}

/// What the agent said before its tool calls, for this chat's transcript.
pub fn note_reply_current(text: &str) {
    if let Ok(runtime) = ACTIVE.try_with(Clone::clone) {
        runtime.note_reply(text);
    }
}

pub async fn emit_current(
    event: &str,
    payload: Value,
    events: &tokio::sync::mpsc::Sender<crate::runtime::RunEvent>,
) -> anyhow::Result<()> {
    if IN_HOOK.try_with(|running| *running).unwrap_or(false) {
        return Ok(());
    }
    if let Ok(runtime) = ACTIVE.try_with(Clone::clone) {
        runtime.emit(event, payload, events).await?;
    }
    Ok(())
}
