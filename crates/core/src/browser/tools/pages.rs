//! The actions about the page as a whole: open, navigate, snapshot, check_looks, screenshot
//! and record.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{
    action_error, model_reference, parse_action, until_stopped, Action, Base, ScreenshotsToModel,
    UNTRUSTED_NOTE,
};
use crate::browser::media::{media_name, Recorder, RECORD_DEFAULT, RECORD_MAX};
use crate::browser::session::site_of;
use crate::tools::web::checked_url;
use crate::tools::ToolCallError;

/// Arguments for `open`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(super) struct OpenArgs {
    pub url: String,
}

pub(super) async fn open(base: &Base, args: Value) -> Result<Value, ToolCallError> {
    let args: OpenArgs = parse_action(Action::Open, args)?;
    let url = checked_url(&args.url).map_err(|e| action_error(Action::Open, e))?;
    let site = site_of(url.as_str()).unwrap_or_default();
    let target = url.to_string();
    base.act(
        Action::Open,
        Some(site.clone()),
        target.clone(),
        format!("opening {site}"),
        |chat| async move { chat.open(&target).await },
    )
    .await
}

/// Arguments for `navigate`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(super) struct NavigateArgs {
    /// back, forward or reload.
    pub direction: String,
}

pub(super) async fn navigate(base: &Base, args: Value) -> Result<Value, ToolCallError> {
    let args: NavigateArgs = parse_action(Action::Navigate, args)?;
    let direction = args.direction.trim().to_ascii_lowercase();
    let label = match direction.as_str() {
        "back" => "going back",
        "forward" => "going forward",
        _ => "reloading",
    };
    base.act(
        Action::Navigate,
        None,
        direction.clone(),
        label.to_owned(),
        |chat| async move { chat.navigate(&direction).await },
    )
    .await
}

/// Arguments for `snapshot`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(super) struct SnapshotArgs {
    #[serde(default, alias = "ref")]
    pub reference: String,
    #[serde(default)]
    pub layout: bool,
}

pub(super) async fn snapshot(base: &Base, args: Value) -> Result<Value, ToolCallError> {
    base.scope.chat.thaw_pages().await;
    let args: SnapshotArgs = parse_action(Action::Snapshot, args)?;
    let reference = Some(model_reference(&args.reference)).filter(|r| !r.is_empty());
    base.act(
        Action::Snapshot,
        None,
        String::new(),
        "reading the page".to_owned(),
        |chat| async move { chat.snapshot(reference.as_deref(), args.layout).await },
    )
    .await
}

/// Arguments for `check_looks` (none).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(super) struct CheckLooksArgs {}

pub(super) async fn check_looks(base: &Base, args: Value) -> Result<Value, ToolCallError> {
    let _: CheckLooksArgs = parse_action(Action::CheckLooks, args)?;
    let chat = Arc::clone(&base.scope.chat);
    let site = base.site().await;
    base.gate(Action::CheckLooks, &site, "", Action::CheckLooks.risk())?;
    chat.thaw_pages().await;
    chat.set_activity(Some("checking how the page looks".to_owned()))
        .await;
    let secrets = base.scope.permissions.config().show_secrets;
    let result = until_stopped(base.scope.cancel.as_deref(), chat.check_looks(secrets)).await;
    chat.set_activity(None).await;
    let mut value = result.map_err(|e| action_error(Action::CheckLooks, e))?;
    value["untrusted"] = json!(true);
    value["note"] = json!(UNTRUSTED_NOTE);
    Ok(value)
}

/// Arguments for `screenshot`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(super) struct ScreenshotArgs {
    #[serde(default, alias = "ref")]
    pub reference: String,
    #[serde(default)]
    pub full_page: bool,
}

pub(super) async fn screenshot(base: &Base, args: Value) -> Result<Value, ToolCallError> {
    let action = Action::Screenshot;
    let args: ScreenshotArgs = parse_action(action, args)?;
    let chat = Arc::clone(&base.scope.chat);
    let media = base
        .scope
        .media
        .clone()
        .ok_or_else(|| action_error(action, "unavailable"))?;
    let site = base.site().await;
    base.gate(action, &site, "", action.risk())?;
    base.scope.chat.thaw_pages().await;
    let reference = Some(model_reference(&args.reference)).filter(|r| !r.is_empty());
    let (png, width, height) = chat
        .screenshot(reference.as_deref(), args.full_page)
        .await
        .map_err(|e| action_error(action, e))?;
    let path = media
        .save(&media_name("screenshot", "png"), &png)
        .map_err(|e| action_error(action, e))?;
    let screenshots = base.scope.screenshots;
    // The model's copy: the same view, scaled down by the browser. It is kept in memory
    // only (see `crate::vision`); the runtime attaches it after this result.
    let kept = screenshots != ScreenshotsToModel::Off
        && match chat
            .screenshot_for_model(
                reference.as_deref(),
                args.full_page,
                crate::vision::MAX_SIDE,
            )
            .await
        {
            Ok(jpeg) => crate::vision::keep(
                &path,
                crate::vision::Image {
                    mime: autoagents::llm::chat::ImageMime::JPEG,
                    bytes: jpeg,
                },
            ),
            Err(_) => false,
        };
    let mut result = json!({
        "path": path,
        "width": width,
        "height": height,
        "note": screenshot_note(screenshots, kept),
    });
    if screenshots == ScreenshotsToModel::Seen && kept {
        result["image_attached"] = json!(true);
    }
    Ok(result)
}

/// The note on a screenshot result: a model that reads text only gets the note it always
/// had; one that reads images is told the image follows, or that this one was too large.
pub(super) fn screenshot_note(screenshots: ScreenshotsToModel, kept: bool) -> &'static str {
    match (screenshots, kept) {
        (ScreenshotsToModel::Seen, true) => crate::prompts::TOOL_SCREENSHOT_SEEN_NOTE,
        (ScreenshotsToModel::Seen, false) => crate::prompts::TOOL_SCREENSHOT_UNATTACHED_NOTE,
        _ => crate::prompts::TOOL_SCREENSHOT_NOTE,
    }
}

/// Arguments for `record`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(super) struct RecordArgs {
    /// start or stop.
    pub operation: String,
    #[serde(default)]
    pub max_seconds: u64,
}

pub(super) async fn record(base: &Base, args: Value) -> Result<Value, ToolCallError> {
    let action = Action::Record;
    let args: RecordArgs = parse_action(action, args)?;
    let chat = Arc::clone(&base.scope.chat);
    let media = base
        .scope
        .media
        .clone()
        .ok_or_else(|| action_error(action, "unavailable"))?;
    let operation = args.operation.trim().to_ascii_lowercase();
    let site = base.site().await;
    base.gate(action, &site, &operation, action.risk())?;
    base.scope.chat.thaw_pages().await;
    match operation.as_str() {
        "start" => {
            let max = match args.max_seconds {
                0 => RECORD_DEFAULT,
                seconds => std::time::Duration::from_secs(seconds).min(RECORD_MAX),
            };
            chat.start_recording(Recorder::Agent, max)
                .await
                .map_err(|e| action_error(action, e))?;
            Ok(
                json!({"recording": true, "max_seconds": max.as_secs(), "note": "Recording. Act on the page, then call browser with action record and operation stop."}),
            )
        }
        "stop" => {
            let (video, length) = until_stopped(
                base.scope.cancel.as_deref(),
                chat.stop_recording(Recorder::Agent),
            )
            .await
            .map_err(|e| action_error(action, e))?;
            let path = media
                .save(&media_name("recording", "mp4"), &video)
                .map_err(|e| action_error(action, e))?;
            Ok(json!({
                "path": path,
                "seconds": length.as_secs_f64().round(),
                "note": "Saved. Call attach_file with this path to show it in your reply.",
            }))
        }
        other => Err(action_error(
            action,
            format!("operation must be start or stop, not '{other}'"),
        )),
    }
}
