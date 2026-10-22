//! The agent's debugging tool (Settings › Browser › Developer mode): one `browser_devtools`
//! tool whose `panel` picks console, elements, network, debugger, storage, performance or
//! memory on the chat's current tab. One tool, not seven, keeps its schema small, since every
//! request of a run carries it. Gated by the Debugging permission for the page's site (local
//! apps always allowed); results are bounded with secrets hidden unless Show secrets is on.

use std::sync::Arc;

use autoagents_derive::{tool, ToolInput};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{Base, BLANK_SITE, UNTRUSTED_NOTE};
use crate::browser::media::media_name;
use crate::tools::site_permissions::Permission;
use crate::tools::{
    async_trait, parse_tool_args, tool_error, RiskLevel, ToolCallError, ToolRuntime, ToolT,
};

/// The debugging tool, offered only with Developer mode on.
pub(super) fn devtools(base: &Base) -> Vec<Box<dyn ToolT>> {
    if !base.scope.permissions.config().developer_mode {
        return Vec::new();
    }
    vec![Box::new(BrowserDevtools { base: base.clone() })]
}

/// Arguments for `browser_devtools`; each panel reads only its own.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToolInput)]
pub struct DevtoolsArgs {
    #[input(description = "console, elements, network, debugger, storage, performance or memory")]
    pub panel: String,
    #[serde(default)]
    #[input(
        description = "elements: inspect|set_attribute|remove_attribute|set_style|set_text|remove; debugger: set|remove|pause_on_exceptions|pause|resume|step_over|step_into|step_out|state; storage: read|set|remove|clear; performance: metrics|trace; memory: summary|snapshot"
    )]
    pub action: String,
    #[serde(default)]
    #[input(description = "console: JavaScript to run (awaited); omit to read messages")]
    pub expression: String,
    #[serde(default)]
    #[input(description = "console/network: only entries newer than this next_since")]
    pub since: u64,
    #[serde(default)]
    #[input(description = "elements: CSS selector")]
    pub selector: String,
    #[serde(default, alias = "ref")]
    #[input(description = "elements: or a snapshot ref")]
    pub reference: String,
    #[serde(default)]
    #[input(description = "elements: attribute or CSS property; storage: cookie name or key")]
    pub name: String,
    #[serde(default)]
    #[input(description = "New value (elements, storage)")]
    pub value: String,
    #[serde(default)]
    #[input(description = "elements: computed styles to read")]
    pub properties: Vec<String>,
    #[serde(default)]
    #[input(description = "network: request id for headers and body; debugger: breakpoint id")]
    pub id: String,
    #[serde(default)]
    #[input(description = "network: URL patterns to block (* wildcards); [] unblocks")]
    pub block: Option<Vec<String>>,
    #[serde(default)]
    #[input(description = "network: off|offline|slow3g|fast3g")]
    pub throttle: String,
    #[serde(default)]
    #[input(description = "debugger set: script URL or part of it")]
    pub url: String,
    #[serde(default)]
    #[input(description = "debugger set: line (1-based)")]
    pub line: u64,
    #[serde(default)]
    #[input(
        description = "debugger: breakpoint condition, or all|uncaught|none for pause_on_exceptions"
    )]
    pub condition: String,
    #[serde(default)]
    #[input(description = "storage: cookies, local or session")]
    pub area: String,
    #[serde(default)]
    #[input(description = "performance trace: seconds, 1-15 (default 5)")]
    pub seconds: u64,
}

#[tool(
    name = "browser_devtools",
    description = "Chrome DevTools on the current tab. console: run JS (or in the paused frame) or read messages at all levels; elements: inspect or live-edit one element; network: list requests, one's headers and body, block, throttle; debugger: breakpoints, stepping, stack and variables (pauses resume after 30 s); storage: cookies, localStorage, sessionStorage; performance: metrics and web vitals, or a trace file; memory: heap summary with growth, or a snapshot file. Secrets are hidden.",
    input = DevtoolsArgs,
)]
struct BrowserDevtools {
    base: Base,
}

fn text(value: &str) -> Option<&str> {
    Some(value.trim()).filter(|value| !value.is_empty())
}

/// What a call changes: reading is Read; running code or changing the page is Execute;
/// changing storage is Write.
fn risk(panel: &str, action: &str, args: &DevtoolsArgs) -> RiskLevel {
    match panel {
        "console" if text(&args.expression).is_some() => RiskLevel::Execute,
        "elements" if action != "inspect" => RiskLevel::Execute,
        "network" if args.block.is_some() || text(&args.throttle).is_some() => RiskLevel::Execute,
        "debugger" if action != "state" => RiskLevel::Execute,
        "storage" if action != "read" => RiskLevel::Write,
        _ => RiskLevel::Read,
    }
}

#[async_trait]
impl ToolRuntime for BrowserDevtools {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let args: DevtoolsArgs = parse_tool_args("browser_devtools", args)?;
        let panel = args.panel.trim().to_ascii_lowercase();
        let default_action = match panel.as_str() {
            "elements" => "inspect",
            "debugger" => "state",
            "storage" => "read",
            "performance" => "metrics",
            "memory" => "summary",
            _ => "",
        };
        let action = text(&args.action)
            .unwrap_or(default_action)
            .to_ascii_lowercase();
        let chat = Arc::clone(&self.base.scope.chat);
        let site = chat
            .current_site()
            .await
            .unwrap_or_else(|| BLANK_SITE.to_owned());
        self.base.scope.permissions.gate(
            self.base.approvals.as_ref(),
            "browser_devtools",
            Permission::Debugging,
            &site,
            format!("browser_devtools {panel} {action} on {site}")
                .trim()
                .replace("  ", " "),
            risk(&panel, &action, &args),
        )?;
        let secrets = self.base.scope.permissions.config().show_secrets;
        let saved = |kind: &str, extension: &str, bytes: Vec<u8>| -> Result<Value, ToolCallError> {
            match &self.base.scope.media {
                Some(media) => Ok(json!(media
                    .save(&media_name(kind, extension), &bytes)
                    .map_err(|e| tool_error(format!("browser_devtools: {e}")))?)),
                None => Ok(Value::Null),
            }
        };
        let result = match panel.as_str() {
            "console" => chat.dev_console(text(&args.expression), args.since, secrets).await,
            "elements" => {
                chat.dev_elements(text(&args.selector), text(&args.reference), &action, &args.name, &args.value, &args.properties, secrets)
                    .await
            }
            "network" => {
                chat.dev_network(text(&args.id), args.since, args.block.as_deref(), text(&args.throttle), secrets)
                    .await
            }
            "debugger" => {
                chat.dev_debugger(&action, args.url.trim(), args.line, args.condition.trim(), args.id.trim(), secrets)
                    .await
            }
            "storage" => {
                chat.dev_storage(&action, &args.area.trim().to_ascii_lowercase(), &args.name, &args.value, secrets)
                    .await
            }
            "performance" => {
                let seconds = if args.seconds == 0 { 5 } else { args.seconds };
                match chat.dev_performance(&action, seconds).await {
                    Ok((mut summary, Some(bytes))) => {
                        summary["file"] = saved("trace", "json", bytes)?;
                        Ok(summary)
                    }
                    Ok((summary, None)) => Ok(summary),
                    Err(error) => Err(error),
                }
            }
            "memory" => match chat.dev_memory(&action).await {
                Ok((mut summary, Some(bytes))) => {
                    summary["file"] = saved("heap", "heapsnapshot", bytes)?;
                    Ok(summary)
                }
                Ok((summary, None)) => Ok(summary),
                Err(error) => Err(error),
            },
            other => Err(format!(
                "panel must be console, elements, network, debugger, storage, performance or memory, not '{}'",
                other.chars().take(30).collect::<String>()
            )),
        };
        let mut value = result.map_err(|e| tool_error(format!("browser_devtools {panel}: {e}")))?;
        if let Some(object) = value.as_object_mut() {
            object.insert("untrusted".into(), json!(true));
            object.insert("note".into(), json!(UNTRUSTED_NOTE));
        }
        Ok(value)
    }
}
