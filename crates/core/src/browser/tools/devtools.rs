//! The agent's debugging tools (Settings › Browser › Developer mode): console, elements,
//! network, debugger, storage, performance and memory on the chat's current tab. Each is
//! gated by the Debugging permission for the page's site (local apps always allowed), and
//! results are bounded with secrets hidden unless Show secrets is on.

use std::sync::Arc;

use autoagents_derive::{tool, ToolInput};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{Base, BLANK_SITE, UNTRUSTED_NOTE};
use crate::browser::media::media_name;
use crate::browser::session::ChatBrowser;
use crate::tools::site_permissions::Permission;
use crate::tools::{
    async_trait, parse_tool_args, tool_error, RiskLevel, ToolCallError, ToolRuntime, ToolT,
};

/// The debugging tools, offered only with Developer mode on.
pub(super) fn devtools(base: &Base) -> Vec<Box<dyn ToolT>> {
    if !base.scope.permissions.config().developer_mode {
        return Vec::new();
    }
    vec![
        Box::new(BrowserConsole { base: base.clone() }),
        Box::new(BrowserElements { base: base.clone() }),
        Box::new(BrowserNetwork { base: base.clone() }),
        Box::new(BrowserDebugger { base: base.clone() }),
        Box::new(BrowserStorage { base: base.clone() }),
        Box::new(BrowserPerformance { base: base.clone() }),
        Box::new(BrowserMemory { base: base.clone() }),
    ]
}

impl Base {
    /// Approves one debugging action on the current page and returns its chat browser and
    /// whether secrets may be shown.
    async fn debug(
        &self,
        tool: &str,
        summary: String,
        risk: RiskLevel,
    ) -> Result<(Arc<ChatBrowser>, bool), ToolCallError> {
        let chat = Arc::clone(&self.scope.chat);
        let site = chat
            .current_site()
            .await
            .unwrap_or_else(|| BLANK_SITE.to_owned());
        self.scope.permissions.gate(
            self.approvals.as_ref(),
            tool,
            Permission::Debugging,
            &site,
            format!("{summary} on {site}"),
            risk,
        )?;
        Ok((chat, self.scope.permissions.config().show_secrets))
    }
}

fn answer(tool: &str, result: Result<Value, String>) -> Result<Value, ToolCallError> {
    let mut value = result.map_err(|e| tool_error(format!("{tool}: {e}")))?;
    if let Some(object) = value.as_object_mut() {
        object.insert("untrusted".into(), json!(true));
        object.insert("note".into(), json!(UNTRUSTED_NOTE));
    }
    Ok(value)
}

fn text(value: &str) -> Option<&str> {
    Some(value.trim()).filter(|value| !value.is_empty())
}

/// Arguments for `browser_console`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToolInput)]
pub struct ConsoleArgs {
    #[serde(default)]
    #[input(
        description = "JavaScript to run in the page (or the paused frame); awaited if it returns a promise. Omit to read console messages"
    )]
    pub expression: String,
    #[serde(default)]
    #[input(description = "Read only messages newer than this (next_since from the last read)")]
    pub since: u64,
}

#[tool(
    name = "browser_console",
    description = "The page's DevTools console. With an expression, runs it in the page like the console does (or in the paused call frame) and returns its value or exception. Without one, returns console messages at every level (log, info, warn, error, exceptions with stacks) newer than `since`.",
    input = ConsoleArgs,
)]
struct BrowserConsole {
    base: Base,
}

#[async_trait]
impl ToolRuntime for BrowserConsole {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let args: ConsoleArgs = parse_tool_args("browser_console", args)?;
        let expression = text(&args.expression).map(str::to_owned);
        let (risk, summary) = match &expression {
            Some(code) => (
                RiskLevel::Execute,
                format!("browser_console run {}", super::quote(code)),
            ),
            None => (RiskLevel::Read, "browser_console read messages".to_owned()),
        };
        let (chat, secrets) = self.base.debug("browser_console", summary, risk).await?;
        answer(
            "browser_console",
            chat.dev_console(expression.as_deref(), args.since, secrets)
                .await,
        )
    }
}

/// Arguments for `browser_elements`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToolInput)]
pub struct ElementsArgs {
    #[serde(default)]
    #[input(description = "CSS selector of the element (first match)")]
    pub selector: String,
    #[serde(default, alias = "ref")]
    #[input(description = "Or a ref from the last snapshot, such as e12")]
    pub reference: String,
    #[serde(default)]
    #[input(
        description = "inspect (default), set_attribute, remove_attribute, set_style, set_text or remove"
    )]
    pub action: String,
    #[serde(default)]
    #[input(
        description = "Attribute or CSS property name, for set_attribute, remove_attribute and set_style"
    )]
    pub name: String,
    #[serde(default)]
    #[input(description = "New value, for set_attribute, set_style and set_text")]
    pub value: String,
    #[serde(default)]
    #[input(
        description = "Computed style properties to read when inspecting; omit for a short default set"
    )]
    pub properties: Vec<String>,
}

#[tool(
    name = "browser_elements",
    description = "The DevTools Elements panel for one element: inspect its attributes, box, computed styles and HTML, or change an attribute, style or text, or remove it, live (nothing is saved; fix the source to keep a change).",
    input = ElementsArgs,
)]
struct BrowserElements {
    base: Base,
}

#[async_trait]
impl ToolRuntime for BrowserElements {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let args: ElementsArgs = parse_tool_args("browser_elements", args)?;
        let action = text(&args.action).unwrap_or("inspect").to_ascii_lowercase();
        let risk = if action == "inspect" {
            RiskLevel::Read
        } else {
            RiskLevel::Execute
        };
        let what = text(&args.selector)
            .or(text(&args.reference))
            .unwrap_or("?")
            .to_owned();
        let (chat, secrets) = self
            .base
            .debug(
                "browser_elements",
                format!("browser_elements {action} {}", super::quote(&what)),
                risk,
            )
            .await?;
        answer(
            "browser_elements",
            chat.dev_elements(
                text(&args.selector),
                text(&args.reference),
                &action,
                &args.name,
                &args.value,
                &args.properties,
                secrets,
            )
            .await,
        )
    }
}

/// Arguments for `browser_network`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToolInput)]
pub struct NetworkArgs {
    #[serde(default)]
    #[input(description = "A request id from the list, for its headers and response body")]
    pub id: String,
    #[serde(default)]
    #[input(description = "List only requests newer than this (next_since from the last list)")]
    pub since: u64,
    #[serde(default)]
    #[input(
        description = "URL patterns to block for this tab (with * wildcards); an empty list unblocks"
    )]
    pub block: Option<Vec<String>>,
    #[serde(default)]
    #[input(description = "Throttle this tab: off, offline, slow3g or fast3g")]
    pub throttle: String,
}

#[tool(
    name = "browser_network",
    description = "The DevTools Network panel for the current tab: list requests (method, URL, status, type, size, time, failure) newer than `since`; with an id, that request's headers and response body; or block URL patterns and throttle the connection.",
    input = NetworkArgs,
)]
struct BrowserNetwork {
    base: Base,
}

#[async_trait]
impl ToolRuntime for BrowserNetwork {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let args: NetworkArgs = parse_tool_args("browser_network", args)?;
        let changes = args.block.is_some() || text(&args.throttle).is_some();
        let risk = if changes {
            RiskLevel::Execute
        } else {
            RiskLevel::Read
        };
        let (chat, secrets) = self
            .base
            .debug("browser_network", "browser_network".to_owned(), risk)
            .await?;
        answer(
            "browser_network",
            chat.dev_network(
                text(&args.id),
                args.since,
                args.block.as_deref(),
                text(&args.throttle),
                secrets,
            )
            .await,
        )
    }
}

/// Arguments for `browser_debugger`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToolInput)]
pub struct DebuggerArgs {
    #[input(
        description = "set, remove, pause_on_exceptions, pause, resume, step_over, step_into, step_out or state"
    )]
    pub action: String,
    #[serde(default)]
    #[input(description = "Script URL, or part of it, for set")]
    pub url: String,
    #[serde(default)]
    #[input(description = "Line number (1-based), for set")]
    pub line: u64,
    #[serde(default)]
    #[input(
        description = "For set, an optional JavaScript condition; for pause_on_exceptions, all, uncaught or none"
    )]
    pub condition: String,
    #[serde(default)]
    #[input(description = "Breakpoint id, for remove")]
    pub id: String,
}

#[tool(
    name = "browser_debugger",
    description = "The DevTools debugger: set or remove breakpoints (by script URL and line, optionally conditional), pause on exceptions, pause, resume and step. While paused it returns the call stack and the top frame's variables; browser_console then runs in that frame. A pause resumes by itself after 30 s.",
    input = DebuggerArgs,
)]
struct BrowserDebugger {
    base: Base,
}

#[async_trait]
impl ToolRuntime for BrowserDebugger {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let args: DebuggerArgs = parse_tool_args("browser_debugger", args)?;
        let action = args.action.trim().to_ascii_lowercase();
        let risk = if action == "state" {
            RiskLevel::Read
        } else {
            RiskLevel::Execute
        };
        let (chat, secrets) = self
            .base
            .debug(
                "browser_debugger",
                format!("browser_debugger {action}"),
                risk,
            )
            .await?;
        answer(
            "browser_debugger",
            chat.dev_debugger(
                &action,
                args.url.trim(),
                args.line,
                args.condition.trim(),
                args.id.trim(),
                secrets,
            )
            .await,
        )
    }
}

/// Arguments for `browser_storage`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToolInput)]
pub struct StorageArgs {
    #[input(description = "cookies, local (localStorage) or session (sessionStorage)")]
    pub area: String,
    #[serde(default)]
    #[input(description = "read (default), set, remove or clear")]
    pub action: String,
    #[serde(default)]
    #[input(description = "Cookie name or storage key, for set and remove")]
    pub key: String,
    #[serde(default)]
    #[input(description = "Value, for set")]
    pub value: String,
}

#[tool(
    name = "browser_storage",
    description = "The DevTools Application panel for the current page: read, set, remove or clear its cookies, localStorage or sessionStorage. Secret-looking values are hidden unless the person turned on Show secrets; httpOnly cookie values are never shown.",
    input = StorageArgs,
)]
struct BrowserStorage {
    base: Base,
}

#[async_trait]
impl ToolRuntime for BrowserStorage {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let args: StorageArgs = parse_tool_args("browser_storage", args)?;
        let action = text(&args.action).unwrap_or("read").to_ascii_lowercase();
        let area = args.area.trim().to_ascii_lowercase();
        let risk = if action == "read" {
            RiskLevel::Read
        } else {
            RiskLevel::Write
        };
        let (chat, secrets) = self
            .base
            .debug(
                "browser_storage",
                format!("browser_storage {action} {area}"),
                risk,
            )
            .await?;
        answer(
            "browser_storage",
            chat.dev_storage(&action, &area, &args.key, &args.value, secrets)
                .await,
        )
    }
}

/// Arguments for `browser_performance`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToolInput)]
pub struct PerformanceArgs {
    #[serde(default)]
    #[input(
        description = "metrics (default: load timing, web vitals, resource sizes, page metrics) or trace (record the main thread)"
    )]
    pub action: String,
    #[serde(default)]
    #[input(description = "Seconds to record a trace, 1 to 15 (default 5)")]
    pub seconds: u64,
}

#[tool(
    name = "browser_performance",
    description = "The DevTools Performance panel: metrics returns load timing, largest contentful paint, layout shift, resource sizes and page metrics; trace records the main thread for some seconds while the page runs and returns long tasks and time spent scripting, rendering and painting, saving the full trace as a file.",
    input = PerformanceArgs,
)]
struct BrowserPerformance {
    base: Base,
}

#[async_trait]
impl ToolRuntime for BrowserPerformance {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let args: PerformanceArgs = parse_tool_args("browser_performance", args)?;
        let action = text(&args.action).unwrap_or("metrics").to_ascii_lowercase();
        let (chat, _) = self
            .base
            .debug(
                "browser_performance",
                format!("browser_performance {action}"),
                RiskLevel::Read,
            )
            .await?;
        let seconds = if args.seconds == 0 { 5 } else { args.seconds };
        let (mut summary, file) = chat
            .dev_performance(&action, seconds)
            .await
            .map_err(|e| tool_error(format!("browser_performance: {e}")))?;
        if let (Some(bytes), Some(media)) = (file, &self.base.scope.media) {
            summary["file"] = json!(media
                .save(&media_name("trace", "json"), &bytes)
                .map_err(|e| tool_error(format!("browser_performance: {e}")))?);
        }
        answer("browser_performance", Ok(summary))
    }
}

/// Arguments for `browser_memory`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToolInput)]
pub struct MemoryArgs {
    #[serde(default)]
    #[input(
        description = "summary (default: heap, DOM nodes, listeners, and growth since the last summary) or snapshot (a full heap snapshot with the largest object kinds)"
    )]
    pub action: String,
}

#[tool(
    name = "browser_memory",
    description = "The DevTools Memory panel: summary returns heap size, DOM nodes, event listeners and documents, and their growth since the last summary (take one, act, take another to find a leak); snapshot takes a full heap snapshot, returns the largest object kinds and saves the snapshot as a file.",
    input = MemoryArgs,
)]
struct BrowserMemory {
    base: Base,
}

#[async_trait]
impl ToolRuntime for BrowserMemory {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let args: MemoryArgs = parse_tool_args("browser_memory", args)?;
        let action = text(&args.action).unwrap_or("summary").to_ascii_lowercase();
        let (chat, _) = self
            .base
            .debug(
                "browser_memory",
                format!("browser_memory {action}"),
                RiskLevel::Read,
            )
            .await?;
        let (mut summary, file) = chat
            .dev_memory(&action)
            .await
            .map_err(|e| tool_error(format!("browser_memory: {e}")))?;
        if let (Some(bytes), Some(media)) = (file, &self.base.scope.media) {
            summary["file"] = json!(media
                .save(&media_name("heap", "heapsnapshot"), &bytes)
                .map_err(|e| tool_error(format!("browser_memory: {e}")))?);
        }
        answer("browser_memory", Ok(summary))
    }
}
