//! The `devtools` action (Settings › Browser › Developer mode): its `panel` picks console,
//! elements, network, debugger, storage, performance or memory on the chat's current tab. Gated
//! by the Debugging permission for the page's site (local apps always allowed); results are
//! bounded with secrets hidden unless Show secrets is on.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{action_error, parse_action, Action, Base, UNTRUSTED_NOTE};
use crate::browser::media::media_name;
use crate::browser::session::ChatBrowser;
use crate::tools::{RiskLevel, ToolCallError};

/// Arguments for `devtools`; each panel reads only its own.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(super) struct DevtoolsArgs {
    /// console, elements, network, debugger, storage, performance or memory.
    pub panel: String,
    /// What the panel does (`inspect`, `set`, `trace`…); each has a default.
    #[serde(default)]
    pub operation: String,
    #[serde(default)]
    pub expression: String,
    #[serde(default)]
    pub since: u64,
    #[serde(default)]
    pub selector: String,
    #[serde(default, alias = "ref")]
    pub reference: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub value: String,
    #[serde(default)]
    pub properties: Vec<String>,
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub block: Option<Vec<String>>,
    #[serde(default)]
    pub throttle: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub line: u64,
    #[serde(default)]
    pub condition: String,
    #[serde(default)]
    pub area: String,
    #[serde(default)]
    pub seconds: u64,
}

fn text(value: &str) -> Option<&str> {
    Some(value.trim()).filter(|value| !value.is_empty())
}

/// What a call changes: reading is Read; running code or changing the page is Execute;
/// changing storage is Write.
fn risk(panel: &str, operation: &str, args: &DevtoolsArgs) -> RiskLevel {
    match panel {
        "console" if text(&args.expression).is_some() => RiskLevel::Execute,
        "elements" if operation != "inspect" => RiskLevel::Execute,
        "network" if args.block.is_some() || text(&args.throttle).is_some() => RiskLevel::Execute,
        "debugger" if operation != "state" => RiskLevel::Execute,
        "storage" if operation != "read" => RiskLevel::Write,
        _ => RiskLevel::Read,
    }
}

/// The operation a call asks of `panel`, or the panel's default.
fn operation_of(panel: &str, given: &str) -> String {
    let default = match panel {
        "elements" => "inspect",
        "debugger" => "state",
        "storage" => "read",
        "performance" => "metrics",
        "memory" => "summary",
        _ => "",
    };
    // Models often repeat the panel as the operation ("console", "console").
    text(given)
        .filter(|operation| !operation.eq_ignore_ascii_case(panel))
        .unwrap_or(default)
        .to_ascii_lowercase()
}

pub(super) async fn devtools(base: &Base, args: Value) -> Result<Value, ToolCallError> {
    let args: DevtoolsArgs = parse_action(Action::Devtools, args)?;
    let panel = args.panel.trim().to_ascii_lowercase();
    let operation = operation_of(&panel, &args.operation);
    let site = base.site().await;
    let detail = format!("{panel} {operation}").trim().replace("  ", " ");
    base.gate(
        Action::Devtools,
        &site,
        &detail,
        risk(&panel, &operation, &args),
    )?;
    base.scope.chat.thaw_pages().await;
    let mut value = run_panel(base, &panel, &operation, &args)
        .await
        .map_err(|e| action_error(Action::Devtools, format!("{panel}: {e}")))?;
    if let Some(object) = value.as_object_mut() {
        object.insert("untrusted".into(), json!(true));
        object.insert("note".into(), json!(UNTRUSTED_NOTE));
    }
    Ok(value)
}

/// Runs one panel's operation on the current tab.
async fn run_panel(
    base: &Base,
    panel: &str,
    operation: &str,
    args: &DevtoolsArgs,
) -> Result<Value, String> {
    let chat: Arc<ChatBrowser> = Arc::clone(&base.scope.chat);
    let secrets = base.scope.permissions.config().show_secrets;
    match panel {
        "console" => chat.dev_console(text(&args.expression), args.since, secrets).await,
        "elements" => {
            chat.dev_elements(text(&args.selector), text(&args.reference), operation, &args.name, &args.value, &args.properties, secrets)
                .await
        }
        "network" => {
            chat.dev_network(text(&args.id), args.since, args.block.as_deref(), text(&args.throttle), secrets)
                .await
        }
        "debugger" => {
            chat.dev_debugger(operation, args.url.trim(), args.line, args.condition.trim(), args.id.trim(), secrets)
                .await
        }
        "storage" => {
            chat.dev_storage(operation, &args.area.trim().to_ascii_lowercase(), &args.name, &args.value, secrets)
                .await
        }
        "performance" => {
            let seconds = if args.seconds == 0 { 5 } else { args.seconds };
            with_file(base, "trace", "json", chat.dev_performance(operation, seconds).await)
        }
        "memory" => with_file(base, "heap", "heapsnapshot", chat.dev_memory(operation).await),
        other => Err(format!(
            "panel must be console, elements, network, debugger, storage, performance or memory, not '{}'",
            other.chars().take(30).collect::<String>()
        )),
    }
}

/// A summary with the file the panel produced saved beside the chat's media, as `file`.
fn with_file(
    base: &Base,
    kind: &str,
    extension: &str,
    result: Result<(Value, Option<Vec<u8>>), String>,
) -> Result<Value, String> {
    let (mut summary, bytes) = result?;
    if let Some(bytes) = bytes {
        summary["file"] = match &base.scope.media {
            Some(media) => json!(media.save(&media_name(kind, extension), &bytes)?),
            None => Value::Null,
        };
    }
    Ok(summary)
}
