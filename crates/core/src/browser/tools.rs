//! The agent's browser tools. Each acts on the chat's own tabs, approves per
//! site (the current page's host, or the target of `browser_open`), waits
//! while the person controls the browser, and answers with a fresh snapshot.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use autoagents_derive::{tool, ToolInput};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::media::{media_name, MediaSink, Recorder, RECORD_DEFAULT, RECORD_MAX};
use super::session::{site_of, ActionReport, ChatBrowser};
use crate::tools::site_permissions::{Permission, SitePermissions};
use crate::tools::web::checked_url;
use crate::tools::{
    async_trait, parse_tool_args, tool_error, ApprovalHook, RiskLevel, ToolCallError, ToolRuntime,
    ToolT,
};

const UNTRUSTED_NOTE: &str = crate::prompts::TOOL_BROWSER_UNTRUSTED;
/// Shown in approvals for a tab with no page yet.
const BLANK_SITE: &str = "about:blank";

/// What the browser tools act on for one run.
#[derive(Clone)]
pub struct BrowserScope {
    pub chat: Arc<ChatBrowser>,
    /// Set when the person stops the run.
    pub cancel: Option<Arc<AtomicBool>>,
    /// Where screenshots and recordings go; without it they are not offered.
    pub media: Option<Arc<dyn MediaSink>>,
    /// Settings › Browser: what the agent may do on each site.
    pub permissions: Arc<SitePermissions>,
}

/// The browser tools for one run.
pub fn browser_tools(approvals: Arc<dyn ApprovalHook>, scope: BrowserScope) -> Vec<Box<dyn ToolT>> {
    // Downloads the agent starts go to the Settings folder, by its Downloads permission.
    let config = scope.permissions.config();
    let directory = config
        .downloads_directory
        .map(std::path::PathBuf::from)
        .unwrap_or_else(super::session::default_downloads_directory);
    let gate: super::session::DownloadGate = {
        let (permissions, approvals) = (Arc::clone(&scope.permissions), Arc::clone(&approvals));
        Arc::new(move |site: &str| {
            permissions
                .gate(
                    approvals.as_ref(),
                    "browser_download",
                    Permission::Downloads,
                    site,
                    format!("download a file from {site}"),
                    RiskLevel::Network,
                )
                .map_err(|error| error.to_string())
        })
    };
    scope
        .chat
        .set_download_policy(Some(super::session::DownloadPolicy { directory, gate }));
    let base = Base { approvals, scope };
    let mut tools: Vec<Box<dyn ToolT>> = vec![
        Box::new(BrowserOpen { base: base.clone() }),
        Box::new(BrowserNavigate { base: base.clone() }),
        Box::new(BrowserSnapshot { base: base.clone() }),
        Box::new(BrowserClick { base: base.clone() }),
        Box::new(BrowserType { base: base.clone() }),
        Box::new(BrowserSelect { base: base.clone() }),
        Box::new(BrowserPress { base: base.clone() }),
        Box::new(BrowserScroll { base: base.clone() }),
        Box::new(BrowserMove { base: base.clone() }),
        Box::new(BrowserTabs { base: base.clone() }),
    ];
    tools.extend(devtools::devtools(&base));
    if base.scope.media.is_some() {
        tools.push(Box::new(BrowserScreenshot { base: base.clone() }));
        tools.push(Box::new(BrowserRecord { base }));
    }
    tools
}

mod devtools;

#[derive(Clone)]
struct Base {
    approvals: Arc<dyn ApprovalHook>,
    scope: BrowserScope,
}

fn quote(text: &str) -> String {
    let mut chars = text.chars();
    let mut short: String = chars.by_ref().take(60).collect();
    if chars.next().is_some() {
        short.push('…');
    }
    format!("“{short}”")
}

/// Runs `step` until it ends or the person stops the run; a stopped step is
/// dropped (so an encode in progress is cancelled) and reported as stopped.
async fn until_stopped<T>(
    cancel: Option<&AtomicBool>,
    step: impl std::future::Future<Output = Result<T, String>>,
) -> Result<T, String> {
    let Some(cancel) = cancel else {
        return step.await;
    };
    let stopped = async {
        while !cancel.load(Ordering::SeqCst) {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    };
    tokio::select! {
        // Finished work wins over a stop that came at the same moment.
        biased;
        result = step => result,
        () = stopped => Err("stopped by you".into()),
    }
}

impl Base {
    /// Settings › Browser for one browsing action on `site`: Block refuses in every mode,
    /// Allow runs without a prompt, Ask asks.
    fn gate(
        &self,
        tool: &str,
        site: &str,
        summary: String,
        risk: RiskLevel,
    ) -> Result<(), ToolCallError> {
        self.scope.permissions.gate(
            self.approvals.as_ref(),
            tool,
            Permission::Browsing,
            site,
            summary,
            risk,
        )
    }

    /// Waits for control, approves `tool` on the site, shows `activity` while
    /// `action` runs, and turns its report into the tool result.
    async fn act<F, Fut>(
        &self,
        tool: &str,
        risk: RiskLevel,
        site: Option<String>,
        summary: String,
        activity: String,
        action: F,
    ) -> Result<Value, ToolCallError>
    where
        F: FnOnce(Arc<ChatBrowser>) -> Fut,
        Fut: std::future::Future<Output = Result<ActionReport, String>>,
    {
        let chat = Arc::clone(&self.scope.chat);
        let site = match site {
            Some(site) => site,
            None => chat
                .current_site()
                .await
                .unwrap_or_else(|| BLANK_SITE.to_owned()),
        };
        self.gate(tool, &site, format!("{summary} on {site}"), risk)?;
        self.scope.chat.thaw_pages().await;
        chat.set_activity(Some(activity)).await;
        let started = std::time::Instant::now();
        let result = until_stopped(self.scope.cancel.as_deref(), action(Arc::clone(&chat))).await;
        chat.set_activity(None).await;
        let report = result.map_err(|e| tool_error(format!("{tool}: {e}")))?;
        let downloads = chat.agent_downloads_since(started).await;
        // A click, redirect or script can land on a site the agent may not browse.
        if let Some(landed) = chat.current_site().await {
            if self.scope.permissions.decide(Permission::Browsing, &landed)
                == crate::configuration::SiteAccess::Block
            {
                let _ = chat.open("about:blank", false).await;
                return Err(tool_error(format!(
                    "{tool}: the page went to {landed}, which is blocked in Settings › Browser, so the tab was cleared. {}",
                    crate::tools::REFUSAL_NOTE
                )));
            }
        }
        let mut value = serde_json::to_value(&report).unwrap_or_default();
        if !downloads.is_empty() {
            value["downloads"] = json!(downloads);
        }
        value["untrusted"] = json!(true);
        value["note"] = json!(UNTRUSTED_NOTE);
        Ok(value)
    }
}

/// Arguments for `browser_open`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToolInput)]
pub struct OpenArgs {
    #[input(description = "Full http or https URL")]
    pub url: String,
    #[serde(default)]
    #[input(description = "Open in a new tab instead of the current one")]
    pub new_tab: bool,
}

#[tool(
    name = "browser_open",
    description = "Open a URL in the built-in browser (the person can watch it in Workspace). Runs JavaScript, unlike fetch_url. Returns a snapshot of the page: an outline with refs like [e12] for elements you can act on, plus new_since_last_action (console errors, failed requests, dialogs). Example: {\"url\":\"http://localhost:5173/signup\"}.",
    input = OpenArgs,
)]
struct BrowserOpen {
    base: Base,
}

#[async_trait]
impl ToolRuntime for BrowserOpen {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let args: OpenArgs = parse_tool_args("browser_open", args)?;
        let url = checked_url(&args.url).map_err(|e| tool_error(format!("browser_open: {e}")))?;
        let site = site_of(url.as_str()).unwrap_or_default();
        let target = url.to_string();
        self.base
            .act(
                "browser_open",
                RiskLevel::Network,
                Some(site.clone()),
                format!("browser_open {target}"),
                format!("opening {site}"),
                |chat| async move { chat.open(&target, args.new_tab).await },
            )
            .await
    }
}

/// Arguments for `browser_navigate`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToolInput)]
pub struct NavigateArgs {
    #[input(description = "back, forward or reload")]
    pub action: String,
}

#[tool(
    name = "browser_navigate",
    description = "Go back, forward or reload the current page in the built-in browser. Returns a fresh snapshot.",
    input = NavigateArgs,
)]
struct BrowserNavigate {
    base: Base,
}

#[async_trait]
impl ToolRuntime for BrowserNavigate {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let args: NavigateArgs = parse_tool_args("browser_navigate", args)?;
        let action = args.action.trim().to_ascii_lowercase();
        let label = match action.as_str() {
            "back" => "going back",
            "forward" => "going forward",
            _ => "reloading",
        };
        self.base
            .act(
                "browser_navigate",
                RiskLevel::Network,
                None,
                format!("browser_navigate {action}"),
                label.to_owned(),
                |chat| async move { chat.navigate(&action).await },
            )
            .await
    }
}

/// Arguments for `browser_snapshot`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToolInput)]
pub struct SnapshotArgs {
    #[serde(default, alias = "ref")]
    #[input(
        description = "Only this element's part of the page (a ref like e12); omit for the whole page"
    )]
    pub reference: String,
}

#[tool(
    name = "browser_snapshot",
    description = "Read the current page of the built-in browser again as an outline with refs. Every other browser tool already returns one, so call this only when the page changed on its own or to read one part (ref).",
    input = SnapshotArgs,
)]
struct BrowserSnapshot {
    base: Base,
}

#[async_trait]
impl ToolRuntime for BrowserSnapshot {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        self.base.scope.chat.thaw_pages().await;
        let args: SnapshotArgs = parse_tool_args("browser_snapshot", args)?;
        let reference = Some(args.reference.trim().to_owned()).filter(|r| !r.is_empty());
        self.base
            .act(
                "browser_snapshot",
                RiskLevel::Read,
                None,
                "browser_snapshot".to_owned(),
                "reading the page".to_owned(),
                |chat| async move { chat.snapshot(reference.as_deref()).await },
            )
            .await
    }
}

/// Arguments for `browser_click`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToolInput)]
pub struct ClickArgs {
    #[serde(default, alias = "ref")]
    #[input(
        description = "The element's ref from the latest snapshot, like e14; omit when clicking a point"
    )]
    pub reference: String,
    #[serde(default)]
    #[input(
        description = "A point to click instead of a ref: CSS pixels from the left of the visible page (for canvas, maps and games)"
    )]
    pub x: Option<f64>,
    #[serde(default)]
    #[input(description = "The point's CSS pixels from the top of the visible page; goes with x")]
    pub y: Option<f64>,
    #[serde(default)]
    #[input(description = "Double-click instead")]
    pub double: bool,
}

#[tool(
    name = "browser_click",
    description = "Click an element of the current page by its ref (buttons, links, checkboxes, tabs), or a point of the visible page by x and y in CSS pixels when there is no ref (a canvas, map or game). Waits for the page to settle and returns a fresh snapshot. Examples: {\"ref\":\"e14\"}, {\"x\":412,\"y\":230}.",
    input = ClickArgs,
)]
struct BrowserClick {
    base: Base,
}

#[async_trait]
impl ToolRuntime for BrowserClick {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let args: ClickArgs = parse_tool_args("browser_click", args)?;
        let reference = args.reference.trim().to_owned();
        let point = match (args.x, args.y) {
            (None, None) if reference.is_empty() => {
                return Err(tool_error(
                    "browser_click: give a ref, or x and y".to_owned(),
                ))
            }
            (None, None) => None,
            (Some(x), Some(y)) if reference.is_empty() => Some((x, y)),
            (Some(_), Some(_)) => {
                return Err(tool_error(
                    "browser_click: give a ref or a point, not both".to_owned(),
                ))
            }
            _ => {
                return Err(tool_error(
                    "browser_click: a point needs both x and y".to_owned(),
                ))
            }
        };
        if let Some((x, y)) = point {
            let label = format!("the point ({}, {})", x.round(), y.round());
            return self
                .base
                .act(
                    "browser_click",
                    RiskLevel::Network,
                    None,
                    format!("browser_click {label}"),
                    format!("clicking {label}"),
                    |chat| async move { chat.click_point(x, y, args.double).await },
                )
                .await;
        }
        let name = self.base.scope.chat.name_of(&reference).await;
        let label = name.as_deref().map_or_else(|| reference.clone(), quote);
        self.base
            .act(
                "browser_click",
                RiskLevel::Network,
                None,
                format!("browser_click {reference} {label}"),
                format!("clicking {label}"),
                |chat| async move { chat.click(&reference, args.double).await },
            )
            .await
    }
}

/// Arguments for `browser_type`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToolInput)]
pub struct TypeArgs {
    #[serde(alias = "ref")]
    #[input(description = "The text field's ref from the latest snapshot")]
    pub reference: String,
    #[input(description = "Text to type")]
    pub text: String,
    #[serde(default)]
    #[input(description = "Press Enter afterwards (submits most forms)")]
    pub submit: bool,
    #[serde(default)]
    #[input(description = "Clear the field first")]
    pub clear: bool,
}

#[tool(
    name = "browser_type",
    description = "Type text into a field of the current page by its ref, optionally clearing it first and pressing Enter after. Returns a fresh snapshot. Example: {\"ref\":\"e12\",\"text\":\"test@example.com\",\"clear\":true}.",
    input = TypeArgs,
)]
struct BrowserType {
    base: Base,
}

#[async_trait]
impl ToolRuntime for BrowserType {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let args: TypeArgs = parse_tool_args("browser_type", args)?;
        if args.text.chars().count() > 10_000 {
            return Err(tool_error(
                "browser_type: text is longer than 10,000 characters".into(),
            ));
        }
        let reference = args.reference.trim().to_owned();
        let name = self.base.scope.chat.name_of(&reference).await;
        let label = name.as_deref().map_or_else(|| reference.clone(), quote);
        self.base
            .act(
                "browser_type",
                RiskLevel::Network,
                None,
                format!("browser_type {reference} {label}: {}", quote(&args.text)),
                format!("typing into {label}"),
                |chat| async move {
                    chat.type_text(&reference, &args.text, args.submit, args.clear)
                        .await
                },
            )
            .await
    }
}

/// Arguments for `browser_select`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToolInput)]
pub struct SelectArgs {
    #[serde(alias = "ref")]
    #[input(description = "The dropdown's ref from the latest snapshot")]
    pub reference: String,
    #[input(
        description = "Option values or labels to choose (one, or several for a multi-select)"
    )]
    pub values: Vec<String>,
}

#[tool(
    name = "browser_select",
    description = "Choose options of a dropdown (<select>) by value or label. For custom dropdowns, click them and then the option. Returns a fresh snapshot. Example: {\"ref\":\"e9\",\"values\":[\"Germany\"]}.",
    input = SelectArgs,
)]
struct BrowserSelect {
    base: Base,
}

#[async_trait]
impl ToolRuntime for BrowserSelect {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let args: SelectArgs = parse_tool_args("browser_select", args)?;
        if args.values.is_empty() || args.values.len() > 50 {
            return Err(tool_error("browser_select: give 1..50 values".into()));
        }
        let reference = args.reference.trim().to_owned();
        let name = self.base.scope.chat.name_of(&reference).await;
        let label = name.as_deref().map_or_else(|| reference.clone(), quote);
        self.base
            .act(
                "browser_select",
                RiskLevel::Network,
                None,
                format!(
                    "browser_select {reference} {label}: {}",
                    quote(&args.values.join(", "))
                ),
                format!("choosing in {label}"),
                |chat| async move { chat.select(&reference, &args.values).await },
            )
            .await
    }
}

const MAX_KEY_HOLD_MS: u64 = 5000;

/// Arguments for `browser_press`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToolInput)]
pub struct PressArgs {
    #[input(
        description = "Key name: Enter, Escape, Tab, ArrowDown, Backspace, PageDown, a letter or digit like w or 1, or a shortcut like Control+a"
    )]
    pub key: String,
    #[serde(default)]
    #[input(
        description = "Milliseconds to keep the key down, up to 5000, to move in a game; omit for a plain press"
    )]
    pub hold_ms: Option<u64>,
}

#[tool(
    name = "browser_press",
    description = "Press a key in the focused element of the current page (Enter, Escape, Tab, arrows, letters, or shortcuts like Control+a), or hold it with hold_ms. Returns a fresh snapshot.",
    input = PressArgs,
)]
struct BrowserPress {
    base: Base,
}

#[async_trait]
impl ToolRuntime for BrowserPress {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let args: PressArgs = parse_tool_args("browser_press", args)?;
        let key = args.key.trim().to_owned();
        let hold = std::time::Duration::from_millis(args.hold_ms.unwrap_or(0).min(MAX_KEY_HOLD_MS));
        self.base
            .act(
                "browser_press",
                RiskLevel::Network,
                None,
                format!("browser_press {key}"),
                format!("pressing {key}"),
                |chat| async move { chat.press(&key, hold).await },
            )
            .await
    }
}

/// Arguments for `browser_scroll`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToolInput)]
pub struct ScrollArgs {
    #[input(description = "up, down, left or right")]
    pub direction: String,
    #[serde(default, alias = "ref")]
    #[input(description = "Scroll inside this element (a ref) instead of the page")]
    pub reference: String,
}

#[tool(
    name = "browser_scroll",
    description = "Scroll the current page (or one scrollable element) by most of a screen, for infinite feeds and content that loads as you scroll. Snapshots already include off-screen content. Returns a fresh snapshot.",
    input = ScrollArgs,
)]
struct BrowserScroll {
    base: Base,
}

#[async_trait]
impl ToolRuntime for BrowserScroll {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let args: ScrollArgs = parse_tool_args("browser_scroll", args)?;
        let direction = args.direction.trim().to_ascii_lowercase();
        let reference = Some(args.reference.trim().to_owned()).filter(|r| !r.is_empty());
        self.base
            .act(
                "browser_scroll",
                RiskLevel::Read,
                None,
                format!("browser_scroll {direction}"),
                "scrolling".to_owned(),
                |chat| async move { chat.scroll(&direction, reference.as_deref()).await },
            )
            .await
    }
}

/// Arguments for `browser_move`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToolInput)]
pub struct MoveArgs {
    #[input(description = "CSS pixels from the left of the visible page")]
    pub x: f64,
    #[input(description = "CSS pixels from the top of the visible page")]
    pub y: f64,
}

#[tool(
    name = "browser_move",
    description = "Move the mouse to a point of the visible page without clicking: to aim in a game, where the view turns with the distance moved, or to open a hover menu. A snapshot cannot show what a canvas draws: read where things are with browser_devtools, then act here and with browser_click, since only real input tests the page.",
    input = MoveArgs,
)]
struct BrowserMove {
    base: Base,
}

#[async_trait]
impl ToolRuntime for BrowserMove {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let MoveArgs { x, y } = parse_tool_args("browser_move", args)?;
        let label = format!("({}, {})", x.round(), y.round());
        self.base
            .act(
                "browser_move",
                RiskLevel::Network,
                None,
                format!("browser_move {label}"),
                format!("moving the mouse to {label}"),
                |chat| async move { chat.move_point(x, y).await },
            )
            .await
    }
}

/// Arguments for `browser_tabs`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToolInput)]
pub struct TabsArgs {
    #[input(description = "list, switch or close")]
    pub action: String,
    #[serde(default)]
    #[input(description = "Tab number from list (for switch and close)")]
    pub tab: u64,
}

#[tool(
    name = "browser_tabs",
    description = "List this chat's browser tabs, switch to one, or close one (by its number from list).",
    input = TabsArgs,
)]
struct BrowserTabs {
    base: Base,
}

#[async_trait]
impl ToolRuntime for BrowserTabs {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let args: TabsArgs = parse_tool_args("browser_tabs", args)?;
        let action = args.action.trim().to_ascii_lowercase();
        let chat = Arc::clone(&self.base.scope.chat);
        let risk = if action == "close" {
            RiskLevel::Network
        } else {
            RiskLevel::Read
        };
        let site = chat
            .current_site()
            .await
            .unwrap_or_else(|| BLANK_SITE.to_owned());
        self.base.gate(
            "browser_tabs",
            &site,
            format!("browser_tabs {action} on {site}"),
            risk,
        )?;
        self.base.scope.chat.thaw_pages().await;
        let tab = usize::try_from(args.tab).ok().filter(|tab| *tab > 0);
        chat.tabs(&action, tab)
            .await
            .map_err(|e| tool_error(format!("browser_tabs: {e}")))
    }
}

/// Arguments for `browser_screenshot`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToolInput)]
pub struct ScreenshotArgs {
    #[serde(default, alias = "ref")]
    #[input(description = "Only this element (a ref); omit for the visible page")]
    pub reference: String,
    #[serde(default)]
    #[input(description = "The whole scrollable page instead of the visible part")]
    pub full_page: bool,
}

#[tool(
    name = "browser_screenshot",
    description = "Save a PNG screenshot of the current page (or one element, or the whole page). Returns its path; to show it to the user, call attach_file with that path. Use snapshots, not screenshots, to read a page.",
    input = ScreenshotArgs,
)]
struct BrowserScreenshot {
    base: Base,
}

#[async_trait]
impl ToolRuntime for BrowserScreenshot {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let args: ScreenshotArgs = parse_tool_args("browser_screenshot", args)?;
        let chat = Arc::clone(&self.base.scope.chat);
        let media = self
            .base
            .scope
            .media
            .clone()
            .ok_or_else(|| tool_error("browser_screenshot: unavailable".into()))?;
        let site = chat
            .current_site()
            .await
            .unwrap_or_else(|| BLANK_SITE.to_owned());
        self.base.gate(
            "browser_screenshot",
            &site,
            format!("browser_screenshot on {site}"),
            RiskLevel::Read,
        )?;
        self.base.scope.chat.thaw_pages().await;
        let reference = Some(args.reference.trim().to_owned()).filter(|r| !r.is_empty());
        let (png, width, height) = chat
            .screenshot(reference.as_deref(), args.full_page)
            .await
            .map_err(|e| tool_error(format!("browser_screenshot: {e}")))?;
        let path = media
            .save(&media_name("screenshot", "png"), &png)
            .map_err(|e| tool_error(format!("browser_screenshot: {e}")))?;
        Ok(json!({
            "path": path,
            "width": width,
            "height": height,
            "note": "Saved. Call attach_file with this path to show it in your reply.",
        }))
    }
}

/// Arguments for `browser_record`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToolInput)]
pub struct RecordArgs {
    #[input(description = "start or stop")]
    pub action: String,
    #[serde(default)]
    #[input(description = "For start: longest length in seconds (default 30, max 120)")]
    pub max_seconds: u64,
}

#[tool(
    name = "browser_record",
    description = "Record the built-in browser as a video: start, act with the other browser tools, then stop to save an MP4 and get its path (it also stops at max_seconds). One recording at a time. To show it to the user, call attach_file with the path.",
    input = RecordArgs,
)]
struct BrowserRecord {
    base: Base,
}

#[async_trait]
impl ToolRuntime for BrowserRecord {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let args: RecordArgs = parse_tool_args("browser_record", args)?;
        let chat = Arc::clone(&self.base.scope.chat);
        let media = self
            .base
            .scope
            .media
            .clone()
            .ok_or_else(|| tool_error("browser_record: unavailable".into()))?;
        let action = args.action.trim().to_ascii_lowercase();
        let site = chat
            .current_site()
            .await
            .unwrap_or_else(|| BLANK_SITE.to_owned());
        self.base.gate(
            "browser_record",
            &site,
            format!("browser_record {action} on {site}"),
            RiskLevel::Read,
        )?;
        self.base.scope.chat.thaw_pages().await;
        match action.as_str() {
            "start" => {
                let max = match args.max_seconds {
                    0 => RECORD_DEFAULT,
                    seconds => std::time::Duration::from_secs(seconds).min(RECORD_MAX),
                };
                chat.start_recording(Recorder::Agent, max)
                    .await
                    .map_err(|e| tool_error(format!("browser_record: {e}")))?;
                Ok(
                    json!({"recording": true, "max_seconds": max.as_secs(), "note": "Recording. Act on the page, then call browser_record with action stop."}),
                )
            }
            "stop" => {
                let (video, length) = until_stopped(
                    self.base.scope.cancel.as_deref(),
                    chat.stop_recording(Recorder::Agent),
                )
                .await
                .map_err(|e| tool_error(format!("browser_record: {e}")))?;
                let path = media
                    .save(&media_name("recording", "mp4"), &video)
                    .map_err(|e| tool_error(format!("browser_record: {e}")))?;
                Ok(json!({
                    "path": path,
                    "seconds": length.as_secs_f64().round(),
                    "note": "Saved. Call attach_file with this path to show it in your reply.",
                }))
            }
            other => Err(tool_error(format!(
                "browser_record: action must be start or stop, not '{other}'"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_stopped_run_drops_the_browser_step_in_progress() {
        struct Dropped(Arc<AtomicBool>);
        impl Drop for Dropped {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let dropped = Arc::new(AtomicBool::new(false));
        let guard = Dropped(Arc::clone(&dropped));
        let step = async move {
            let _guard = guard;
            std::future::pending::<Result<(), String>>().await
        };
        let flag = Arc::clone(&cancel);
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            flag.store(true, Ordering::SeqCst);
        });
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            until_stopped(Some(&cancel), step),
        )
        .await
        .expect("a stopped step returns promptly");
        assert_eq!(result, Err("stopped by you".to_owned()));
        assert!(dropped.load(Ordering::SeqCst));
        assert_eq!(
            until_stopped(Some(&cancel), async { Ok(1) }).await.ok(),
            Some(1)
        );
        assert_eq!(
            until_stopped(None, async { Ok::<_, String>(2) }).await,
            Ok(2)
        );
    }

    struct NoMedia;
    impl MediaSink for NoMedia {
        fn save(&self, _: &str, _: &[u8]) -> Result<String, String> {
            Err("unused".into())
        }
    }

    fn parses(tool: &str, args: Value) -> Result<(), ToolCallError> {
        match tool {
            "browser_open" => parse_tool_args::<OpenArgs>(tool, args).map(|_| ()),
            "browser_navigate" => parse_tool_args::<NavigateArgs>(tool, args).map(|_| ()),
            "browser_snapshot" => parse_tool_args::<SnapshotArgs>(tool, args).map(|_| ()),
            "browser_click" => parse_tool_args::<ClickArgs>(tool, args).map(|_| ()),
            "browser_type" => parse_tool_args::<TypeArgs>(tool, args).map(|_| ()),
            "browser_select" => parse_tool_args::<SelectArgs>(tool, args).map(|_| ()),
            "browser_press" => parse_tool_args::<PressArgs>(tool, args).map(|_| ()),
            "browser_scroll" => parse_tool_args::<ScrollArgs>(tool, args).map(|_| ()),
            "browser_move" => parse_tool_args::<MoveArgs>(tool, args).map(|_| ()),
            "browser_tabs" => parse_tool_args::<TabsArgs>(tool, args).map(|_| ()),
            "browser_screenshot" => parse_tool_args::<ScreenshotArgs>(tool, args).map(|_| ()),
            "browser_record" => parse_tool_args::<RecordArgs>(tool, args).map(|_| ()),
            "browser_devtools" => parse_tool_args::<devtools::DevtoolsArgs>(tool, args).map(|_| ()),
            other => panic!("untested tool {other}"),
        }
    }

    /// A model fills in exactly what a tool's schema advertises; every one of
    /// those names must parse (a live run once lost three calls to `reference`
    /// in the schema but `ref` in the parser).
    #[test]
    fn every_advertised_argument_parses_and_ref_is_accepted_too() {
        let hub = super::super::BrowserHub::new(super::super::BrowserConfig {
            root: std::env::temp_dir(),
            executable: None,
            headless: true,
        });
        let tools = browser_tools(
            Arc::new(crate::tools::AllowAllHook),
            BrowserScope {
                permissions: crate::tools::site_permissions::SitePermissions::new(
                    Default::default(),
                    None,
                ),
                chat: hub.chat("schema-test", &std::env::temp_dir()),
                cancel: None,
                media: Some(Arc::new(NoMedia)),
            },
        );
        let sample = |schema: &Value| match schema["type"].as_str() {
            Some("boolean") => json!(true),
            Some("integer" | "number") => json!(1),
            Some("array") => json!(["x"]),
            _ => json!("e1"),
        };
        for tool in &tools {
            let schema = tool.args_schema();
            let properties = schema["properties"]
                .as_object()
                .cloned()
                .unwrap_or_default();
            let args: serde_json::Map<String, Value> = properties
                .iter()
                .map(|(name, spec)| (name.clone(), sample(spec)))
                .collect();
            let parsed = parses(tool.name(), Value::Object(args));
            assert!(parsed.is_ok(), "{}: {parsed:?} from {schema}", tool.name());
        }
        assert_eq!(tools.len(), 13);
        let click: ClickArgs = parse_tool_args("browser_click", json!({"ref": "e4"})).unwrap();
        assert_eq!(click.reference, "e4");
        let click: ClickArgs =
            parse_tool_args("browser_click", json!({"reference": "e5"})).unwrap();
        assert_eq!(click.reference, "e5");
    }
}
