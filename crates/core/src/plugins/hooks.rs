//! Bounded hook commands. Hooks cannot grant tool permissions.
use crate::tools::{ApprovalHook, RiskLevel, ToolAction};
use anyhow::{bail, Context};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::Read as _;
use std::path::Path;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

mod blocking;
mod claude_fields;
mod context;
mod env_file;
mod events;
mod matchers;
mod process;
mod prompt_hooks;
mod rewake;
mod runtime;
mod session;
mod skills;
#[cfg(all(test, unix))]
mod tests;
mod transcript;

pub use blocking::KeepGoing;
use blocking::*;
use claude_fields::*;
use context::*;
pub use context::{prompt_context, PROMPT_CONTEXT_LIMIT};
#[cfg(all(test, unix))]
use env_file::*;
pub use env_file::{env_prefix, PluginEnvFile};
pub use events::{
    emit_current, note_reply_current, permission_denied, permission_request, plugin_bins,
    post_compact, post_tool_batch, stop_refused,
};
use matchers::*;
pub use matchers::{compile_matcher, glob_matches, moment_matches};
use process::*;
pub use process::{hook_identity, run, Failed};
use prompt_hooks::*;
pub use prompt_hooks::{parse_prompt_verdict, PromptModel, PromptVerdict, PROMPT_HOOK_BYTES};
use rewake::*;
pub use rewake::{
    rewake_notes_for_run, rewake_waiting, take_rewake_notes, RewakeNote, RewakeNotify,
    HOOK_WAKE_MARKER, REWAKE_LABEL_LIMIT, REWAKE_NOTES_PER_CHAT, REWAKE_OUTPUT_LIMIT,
};
use session::*;
pub use session::{HookSession, OpenSession, SessionEnding, UnattendedOutcome};
use skills::*;
pub use skills::{skill_label, skill_read_hooks};
use transcript::*;

pub const EVENTS: [&str; 19] = [
    "PromptSubmit",
    "RunStart",
    "BeforeTool",
    "AfterTool",
    "BeforeCompaction",
    "Stop",
    "RunEnd",
    "SubagentStart",
    "SubagentEnd",
    "SessionStart",
    "SessionEnd",
    "UserPromptExpansion",
    "PermissionRequest",
    "PermissionDenied",
    "PostToolBatch",
    "PostCompact",
    "Notification",
    "ConfigChange",
    "InstructionsLoaded",
];

/// Events no run may be there for: what they run was allowed when the session opened.
const UNATTENDED_EVENTS: [&str; 2] = ["SessionEnd", "Notification"];

fn canonical_event(event: &str) -> &str {
    match event {
        "BeforeToolCall" => "BeforeTool",
        "AfterToolCall" | "ToolCallFailed" => "AfterTool",
        "RunFinished" | "RunFailed" => "RunEnd",
        "SubagentStop" => "SubagentEnd",
        _ => event,
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HookFailurePolicy {
    Ignore,
    Abort,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hook {
    pub name: String,
    pub event: String,
    pub command: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "timeout", deserialize_with = "whole_seconds")]
    pub timeout_seconds: u64,
    #[serde(default)]
    pub blocking: bool,
    #[serde(default)]
    pub matcher: Option<String>,
    #[serde(default)]
    pub failure_policy: Option<HookFailurePolicy>,
    /// The shell for `command`; only `bash` is known. Absent, the platform's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell: Option<String>,
    /// Runs without being waited for: it can't block anything or add context.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub background: bool,
    /// Only for a shell call running a command that starts with these words. A script that
    /// can't be read counts as running it, so a guard errs towards running.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_prefix: Option<Vec<String>>,
    /// Only for a shell call running a command that matches this pattern, where `*` is any text
    /// (`git*commit*`). Like `command_prefix`, what can't be read counts as running it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_glob: Option<String>,
    /// A background hook that can report: when it exits 2 what it wrote reaches the agent as a
    /// labelled note. See [`RewakeNote`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rewake: Option<HookRewake>,
    /// A hook that asks the chat's model this instead of running `command`, which is then empty.
    /// `$ARGUMENTS` stands for its input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// For an event that isn't about a tool, what it must be about, in Claude's matcher language:
    /// `resume` on `SessionStart`, a reason on `SessionEnd`, a skill's name on
    /// `UserPromptExpansion` and so on (see [`moment_subject`]). Absent, all of them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub moment_matcher: Option<String>,
    /// Claude's convention: exit code 2 asks to block, with the reason on stderr.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub exit_two_blocks: bool,
    #[serde(skip)]
    pub plugin_root: Option<std::path::PathBuf>,
    /// `CLAUDE_PLUGIN_DATA`: the plugin's own folder, kept across runs and updates.
    #[serde(skip)]
    pub plugin_data: Option<std::path::PathBuf>,
    /// Where the plugin's session hooks leave variables for its later hooks.
    #[serde(skip)]
    pub env_file: Option<PluginEnvFile>,
    #[serde(skip)]
    pub runtime_identity: Option<String>,
}
/// What a hook that can report says about its report.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HookRewake {
    /// The plugin's line above the report, for the model (Claude's `rewakeMessage`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// The plugin's short description of it, for the person (Claude's `rewakeSummary`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

fn timeout() -> u64 {
    10
}
/// The longest a hook may ask for. A long one is cut short by Stop, not waited for.
pub const MAX_TIMEOUT_SECONDS: u64 = 600;
/// Seconds as a whole number; models often write `5.0`.
fn whole_seconds<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
    let seconds = f64::deserialize(deserializer)?;
    if seconds.fract() != 0.0 || !(0.0..=u32::MAX as f64).contains(&seconds) {
        return Err(serde::de::Error::custom(
            "timeout_seconds must be a whole number",
        ));
    }
    Ok(seconds as u64)
}
impl Hook {
    pub fn validate(&self) -> anyhow::Result<()> {
        if !super::safe_name(&self.name)
            || !EVENTS.contains(&canonical_event(&self.event))
            || self.command.trim().is_empty() == self.prompt.is_none()
            || !(1..=MAX_TIMEOUT_SECONDS).contains(&self.timeout_seconds)
        {
            bail!(
                "Invalid hook name, event, command, or timeout (1–{MAX_TIMEOUT_SECONDS} seconds)"
            );
        }
        if self.shell.as_deref().is_some_and(|shell| shell != "bash") {
            bail!("Hook shell must be bash");
        }
        if self.rewake.is_some() && !self.background {
            bail!("A hook that reports back runs in the background");
        }
        if self.rewake.as_ref().is_some_and(|rewake| {
            [&rewake.message, &rewake.summary]
                .into_iter()
                .flatten()
                .any(|text| text.len() > REWAKE_LABEL_LIMIT)
        }) {
            bail!("A hook's report labels are at most {REWAKE_LABEL_LIMIT} bytes");
        }
        if let Some(pattern) = self.matcher.as_deref().and_then(matcher_regex) {
            compile_matcher(pattern).context("Invalid hook matcher")?;
        }
        if let Some(pattern) = &self.moment_matcher {
            if !plain_matcher(pattern) {
                compile_matcher(pattern).context("Invalid hook matcher")?;
            }
        }
        Ok(())
    }

    /// Whether this hook runs for `payload` of its event.
    fn applies(&self, payload: &Value, mcp_names: &McpNames) -> bool {
        let tool = payload["tool"].as_str();
        (self.event != "ToolCallFailed" || payload["ok"] == false)
            && (self.event != "RunFailed" || payload["status"] == "failed")
            && self
                .matcher
                .as_ref()
                .is_none_or(|matcher| matcher_covers(matcher, tool, mcp_names))
            && self.moment_matcher.as_ref().is_none_or(|pattern| {
                moment_subject(canonical_event(&self.event), payload)
                    .is_some_and(|subject| moment_matches(pattern, subject))
            })
            && self.command_prefix.as_ref().is_none_or(|prefix| {
                tool.is_some_and(|tool| {
                    runs_command(tool, &payload["arguments"], |command| {
                        starts_with(command, prefix)
                    })
                })
            })
            && self.command_glob.as_ref().is_none_or(|pattern| {
                tool.is_some_and(|tool| {
                    runs_command(tool, &payload["arguments"], |command| {
                        command_matches_glob(command, pattern)
                    })
                })
            })
    }
}

#[derive(Clone)]
pub struct HookRuntime {
    pub hooks: Vec<Hook>,
    pub run_id: String,
    pub thread_id: String,
    pub root: std::path::PathBuf,
    pub approvals: Arc<dyn ApprovalHook>,
    /// Set inside a subagent: payloads carry `agent: {id, name}`.
    pub agent: Option<Value>,
    /// The Claude names of this run's MCP tools, by Themis tool name.
    pub mcp_names: McpNames,
    pub session: HookSession,
}

/// A Themis MCP tool's Claude names (`mcp__server__tool`, then the form Claude gives a server
/// inside a plugin, `mcp__plugin_name_server__tool`), by its Themis name.
pub type McpNames = Arc<std::collections::HashMap<String, Vec<String>>>;

tokio::task_local! { pub static ACTIVE: HookRuntime; static IN_HOOK: bool; static PROMPT_CONTEXT: String; }

/// `text` cut to at most `limit` bytes on a character boundary.
fn capped(text: &str, limit: usize) -> &str {
    if text.len() <= limit {
        return text;
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

pub async fn with_hooks<F, Fut>(
    runtime: HookRuntime,
    task: &str,
    events: tokio::sync::mpsc::Sender<crate::runtime::RunEvent>,
    run: F,
) -> anyhow::Result<String>
where
    F: FnOnce(tokio::sync::mpsc::Sender<crate::runtime::RunEvent>) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<String>>,
{
    use crate::runtime::RunEvent;
    if let Err(error) = runtime
        .emit("RunStart", json!({"event":"RunStart","task":task}), &events)
        .await
    {
        runtime
            .emit(
                "RunEnd",
                json!({"event":"RunEnd","status":"failed","ok":false,"error":error.to_string()}),
                &events,
            )
            .await
            .ok();
        events
            .send(RunEvent::Failed {
                error: error.to_string(),
            })
            .await
            .ok();
        return Err(error);
    }
    let (tx, mut rx) = tokio::sync::mpsc::channel(256);
    let terminal = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let seen = terminal.clone();
    let outgoing = events.clone();
    let pump_runtime = runtime.clone();
    let pump = tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            if matches!(
                event,
                RunEvent::Finished { .. } | RunEvent::Failed { .. } | RunEvent::Incomplete { .. }
            ) {
                seen.store(true, std::sync::atomic::Ordering::SeqCst);
                pump_runtime
                    .emit(
                        "RunEnd",
                        json!({"event":"RunEnd","status":match &event { RunEvent::Failed { error } if error == "Stopped by you" => "cancelled", RunEvent::Failed { .. } => "failed", RunEvent::Incomplete { .. } => "incomplete", _ => "completed" },"result":event}),
                        &outgoing,
                    )
                    .await
                    .ok();
            }
            outgoing.send(event).await.ok();
        }
    });
    // A session opens before anything else in it happens, and what it said stays for every turn.
    let session = runtime.session_hooks(&events).await;
    // The person's message goes to the PromptSubmit hooks before the first model request.
    let submitted = match runtime.prompt_submit(task, &events).await {
        Ok(context) => runtime
            .attached_skill_hooks(task, &events)
            .await
            .map(|expansion| prompt_context_of(&session, &expansion, &context)),
        Err(error) => Err(error),
    };
    let context = match submitted {
        Ok(context) => context,
        Err(error) => {
            runtime
                .emit(
                    "RunEnd",
                    json!({"event":"RunEnd","status":"failed","ok":false,"error":error.to_string()}),
                    &events,
                )
                .await
                .ok();
            drop(tx);
            let _ = pump.await;
            events
                .send(RunEvent::Failed {
                    error: error.to_string(),
                })
                .await
                .ok();
            return Err(error);
        }
    };
    let result = ACTIVE
        .scope(runtime.clone(), PROMPT_CONTEXT.scope(context, run(tx)))
        .await;
    let _ = pump.await;
    if !terminal.load(std::sync::atomic::Ordering::SeqCst) {
        runtime
            .emit(
                "RunEnd",
                json!({"event":"RunEnd","status": if result.is_ok() {"completed"} else {"failed"},"ok":result.is_ok(),"error":result.as_ref().err().map(ToString::to_string)}),
                &events,
            )
            .await
            .ok();
        if let Err(error) = &result {
            events
                .send(RunEvent::Failed {
                    error: error.to_string(),
                })
                .await
                .ok();
        }
    }
    result
}

// Kill descendants as well as the launcher when a bounded command finishes or is cancelled.
pub(super) struct ProcessGroup(pub Option<u32>);
impl Drop for ProcessGroup {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(id) = self.0 {
            // SAFETY: the child's freshly-created group id is a positive process id.
            unsafe {
                libc::kill(-(id as i32), libc::SIGKILL);
            }
        }
    }
}
