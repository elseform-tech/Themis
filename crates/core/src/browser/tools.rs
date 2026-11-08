//! The agent's `browser` tool: one tool whose `action` picks what to do on the chat's own page
//! (open, click, type, devtools and the rest; see [`args`]). Each action approves per site (the
//! current page's host, or the target of `open`), keyed by the action, and answers with a
//! fresh snapshot.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde_json::{json, Value};

use super::media::MediaSink;
use super::session::{ActionReport, ChatBrowser};
use crate::tools::site_permissions::SitePermissions;
use crate::tools::{
    async_trait, tool_error, ApprovalHook, RiskLevel, ToolCallError, ToolRuntime, ToolT,
    BROWSER_TOOL,
};

const UNTRUSTED_NOTE: &str = crate::prompts::TOOL_BROWSER_UNTRUSTED;
/// Shown in approvals for a tab with no page yet.
const BLANK_SITE: &str = "about:blank";
/// The approval key of a file a page sends, under the Downloads permission.
const DOWNLOAD: &str = "download";
const DOWNLOAD_RISK: RiskLevel = RiskLevel::Network;

/// What the browser tool acts on for one run.
#[derive(Clone)]
pub struct BrowserScope {
    pub chat: Arc<ChatBrowser>,
    /// Set when the person stops the run.
    pub cancel: Option<Arc<AtomicBool>>,
    /// Where screenshots and recordings go; without it they are not offered.
    pub media: Option<Arc<dyn MediaSink>>,
    /// Settings › Browser: what the agent may do on each site.
    pub permissions: Arc<SitePermissions>,
    /// What screenshots give a model (`browser.screenshots_to_model` and whether the run's
    /// model reads images).
    pub screenshots: ScreenshotsToModel,
}

/// What a screenshot gives a model besides the PNG saved for the person.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ScreenshotsToModel {
    /// Nothing: the text result only (`browser.screenshots_to_model` is off).
    #[default]
    Off,
    /// The run's model reads images: it gets the model copy after the tool result.
    Seen,
}

/// The browser tool for one run.
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
                    DOWNLOAD,
                    site,
                    format!("download a file from {site}"),
                    DOWNLOAD_RISK,
                )
                .map_err(|error| error.to_string())
        })
    };
    scope
        .chat
        .set_download_policy(Some(super::session::DownloadPolicy { directory, gate }));
    let offered: Vec<Action> = args::ACTIONS
        .iter()
        .map(|(action, ..)| *action)
        .filter(|action| match action {
            Action::Screenshot | Action::Record => scope.media.is_some(),
            Action::Devtools => config.developer_mode,
            _ => true,
        })
        .collect();
    let description = description(&offered, scope.screenshots);
    vec![Box::new(BrowserTool {
        schema: args::schema(&offered),
        offered,
        description,
        base: Base { approvals, scope },
    })]
}

/// The tool's description: what every run has, then a line for each optional action on offer.
fn description(offered: &[Action], screenshots: ScreenshotsToModel) -> String {
    let screenshot = match screenshots {
        ScreenshotsToModel::Seen => crate::prompts::TOOL_BROWSER_SCREENSHOT_SEEING,
        ScreenshotsToModel::Off => crate::prompts::TOOL_BROWSER_SCREENSHOT,
    };
    let optional = [
        (Action::Screenshot, screenshot),
        (Action::Record, crate::prompts::TOOL_BROWSER_RECORD),
        (Action::Devtools, crate::prompts::TOOL_BROWSER_DEVTOOLS),
    ];
    let mut text = crate::prompts::TOOL_BROWSER.to_owned();
    for (action, line) in optional {
        if offered.contains(&action) {
            text.push('\n');
            text.push_str(line);
        }
    }
    text
}

mod args;
mod devtools;
mod input;
mod pages;

use args::Action;

struct BrowserTool {
    base: Base,
    offered: Vec<Action>,
    description: String,
    schema: Value,
}

impl std::fmt::Debug for BrowserTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BrowserTool").finish_non_exhaustive()
    }
}

impl ToolT for BrowserTool {
    fn name(&self) -> &str {
        BROWSER_TOOL
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn args_schema(&self) -> Value {
        self.schema.clone()
    }

    fn output_schema(&self) -> Option<Value> {
        None
    }
}

#[async_trait]
impl ToolRuntime for BrowserTool {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let base = &self.base;
        match args::action_of(&args, &self.offered)? {
            Action::Open => pages::open(base, args).await,
            Action::Navigate => pages::navigate(base, args).await,
            Action::Snapshot => pages::snapshot(base, args).await,
            Action::CheckLooks => pages::check_looks(base, args).await,
            Action::Screenshot => pages::screenshot(base, args).await,
            Action::Record => pages::record(base, args).await,
            Action::Click => input::click(base, args).await,
            Action::Type => input::type_text(base, args).await,
            Action::Select => input::select(base, args).await,
            Action::Press => input::press(base, args).await,
            Action::Scroll => input::scroll(base, args).await,
            Action::Move => input::move_mouse(base, args).await,
            Action::Drag => input::drag(base, args).await,
            Action::Devtools => devtools::devtools(base, args).await,
        }
    }
}

/// Parses `args` for `action`; a failure names the action and the field.
fn parse_action<T: serde::de::DeserializeOwned>(
    action: Action,
    args: Value,
) -> Result<T, ToolCallError> {
    crate::tools::parse_tool_args(&format!("browser {}", action.name()), args)
}

/// An error from `action`, named as the model called it.
fn action_error(action: Action, message: impl std::fmt::Display) -> ToolCallError {
    tool_error(format!("browser {}: {message}", action.name()))
}

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

/// A ref as a model sends it: models quote refs they saw in JSON, and fill unused fields with
/// the word for nothing.
fn model_reference(text: &str) -> String {
    let text = text.trim().trim_matches('"').trim();
    match text.to_ascii_lowercase().as_str() {
        "null" | "none" | "nil" | "undefined" => String::new(),
        _ => text.to_owned(),
    }
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
    /// The site the current page is on, or `about:blank`.
    async fn site(&self) -> String {
        self.scope
            .chat
            .current_site()
            .await
            .unwrap_or_else(|| BLANK_SITE.to_owned())
    }

    /// Settings › Browser for `action` on `site`: Block refuses in every mode, Allow runs
    /// without a prompt, Ask asks. `detail` follows the action's name in the approval.
    fn gate(
        &self,
        action: Action,
        site: &str,
        detail: &str,
        risk: RiskLevel,
    ) -> Result<(), ToolCallError> {
        let summary = if detail.is_empty() {
            format!("browser {} on {site}", action.name())
        } else {
            format!("browser {} {detail} on {site}", action.name())
        };
        self.scope
            .permissions
            .gate(self.approvals.as_ref(), action.name(), site, summary, risk)
    }

    /// Waits for control, approves `action` on the site, shows `activity` while
    /// `step` runs, and turns its report into the tool result.
    async fn act<F, Fut>(
        &self,
        action: Action,
        site: Option<String>,
        detail: String,
        activity: String,
        step: F,
    ) -> Result<Value, ToolCallError>
    where
        F: FnOnce(Arc<ChatBrowser>) -> Fut,
        Fut: std::future::Future<Output = Result<ActionReport, String>>,
    {
        let chat = Arc::clone(&self.scope.chat);
        let site = match site {
            Some(site) => site,
            None => self.site().await,
        };
        self.gate(action, &site, &detail, action.risk())?;
        self.scope.chat.thaw_pages().await;
        chat.set_activity(Some(activity)).await;
        let started = std::time::Instant::now();
        let result = until_stopped(self.scope.cancel.as_deref(), step(Arc::clone(&chat))).await;
        chat.set_activity(None).await;
        let report = result.map_err(|e| action_error(action, e))?;
        let downloads = chat.agent_downloads_since(started).await;
        // A click, redirect or script can land on a site the agent may not browse.
        if let Some(landed) = chat.current_site().await {
            if self.scope.permissions.decide(
                crate::tools::site_permissions::Permission::Browsing,
                &landed,
            ) == crate::configuration::SiteAccess::Block
            {
                let _ = chat.open("about:blank").await;
                return Err(action_error(action, format!(
                    "the page went to {landed}, which is blocked in Settings › Browser, so the tab was cleared. {}",
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

#[cfg(test)]
mod tests;
