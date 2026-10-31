//! Bounded hook commands. Hooks cannot grant tool permissions.
use crate::tools::{ApprovalHook, RiskLevel, ToolAction};
use anyhow::{bail, Context};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::Path;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub const EVENTS: [&str; 6] = [
    "PromptSubmit",
    "RunStart",
    "BeforeTool",
    "AfterTool",
    "BeforeCompaction",
    "RunEnd",
];

fn canonical_event(event: &str) -> &str {
    match event {
        "BeforeToolCall" => "BeforeTool",
        "AfterToolCall" | "ToolCallFailed" => "AfterTool",
        "RunFinished" => "RunEnd",
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
    #[serde(skip)]
    pub plugin_root: Option<std::path::PathBuf>,
    #[serde(skip)]
    pub runtime_identity: Option<String>,
}
fn timeout() -> u64 {
    10
}
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
            || self.command.trim().is_empty()
            || !(1..=60).contains(&self.timeout_seconds)
        {
            bail!("Invalid hook name, event, command, or timeout (1–60 seconds)");
        }
        Ok(())
    }
}

pub async fn run(
    hook: &Hook,
    root: &Path,
    payload: &Value,
    approvals: &Arc<dyn ApprovalHook>,
) -> anyhow::Result<Value> {
    hook.validate()?;
    use std::hash::{Hash, Hasher};
    let mut definition = std::collections::hash_map::DefaultHasher::new();
    serde_json::to_string(hook)?.hash(&mut definition);
    let action = ToolAction {
        display: Some(crate::tools::CommandDisplay {
            argv: vec![hook.command.clone()],
            cwd: None,
            background: false,
            sandbox_blocked: None,
        }),
        command: Vec::new(),
        tool: format!(
            "hook_{}_{:016x}",
            hook.runtime_identity.as_ref().unwrap_or(&hook.name),
            definition.finish()
        ),
        summary: format!("{}: {}", hook.event, hook.command),
        risk: RiskLevel::Execute,
    };
    if approvals.approve(&action).is_denied() {
        bail!("Hook execution denied");
    }
    #[cfg(not(windows))]
    let mut command = {
        let mut c = tokio::process::Command::new("sh");
        c.args(["-c", &hook.command]);
        c
    };
    #[cfg(windows)]
    let mut command = {
        let mut c = tokio::process::Command::new("cmd");
        // cmd parses shell text rather than C-runtime escaped argv.
        c.args(["/D", "/S", "/C"])
            .raw_arg(format!("\"{}\"", hook.command));
        c
    };
    command
        .current_dir(root)
        .env_clear()
        .kill_on_drop(true)
        .env("THEMIS_HOOK_ACTIVE", "1")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    for name in ["PATH", "HOME", "SYSTEMROOT", "TMPDIR"] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    if let Some(path) = &hook.plugin_root {
        command.env("CLAUDE_PLUGIN_ROOT", path);
    }
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command.spawn()?;
    if let Some(stderr) = child.stderr.take() {
        crate::diagnostics::capture_stderr(stderr, "hook", Vec::new());
    }
    let _group = ProcessGroup(child.id());
    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(hook.timeout_seconds),
        async {
            let mut input = child.stdin.take().context("Missing hook stdin")?;
            // Hooks may finish without consuming stdin; still inspect their exit status and JSON.
            for result in [
                input.write_all(&serde_json::to_vec(payload)?).await,
                input.shutdown().await,
            ] {
                if let Err(error) = result {
                    if error.kind() != std::io::ErrorKind::BrokenPipe {
                        return Err(error.into());
                    }
                }
            }
            drop(input);
            let mut output = Vec::new();
            child
                .stdout
                .take()
                .context("Missing hook stdout")?
                .take(65537)
                .read_to_end(&mut output)
                .await?;
            if output.len() > 65536 {
                bail!("Hook output exceeds 64 KiB");
            }
            let status = child.wait().await?;
            if !status.success() {
                bail!("Hook exited unsuccessfully ({status})");
            }
            let text = String::from_utf8(output)?;
            if text.trim().is_empty() {
                return Ok(json!({"ok":true}));
            }
            // A prompt hook may print plain text: that is context for the turn.
            let prompt = canonical_event(&hook.event) == "PromptSubmit";
            let value: Value = match serde_json::from_str(&text) {
                Ok(value) => value,
                Err(_) if prompt => json!({"context": text}),
                Err(error) => return Err(error).context("Hook output must be JSON"),
            };
            if value.get("block").and_then(Value::as_bool) == Some(true)
                || value.get("decision").and_then(Value::as_str) == Some("block")
                || value["hookSpecificOutput"]["permissionDecision"] == "deny"
            {
                return Err(Blocked(block_reason(&value)).into());
            }
            Ok(value)
        },
    )
    .await;
    match outcome {
        Ok(value) => value,
        Err(_) => {
            let _ = child.kill().await;
            bail!("Hook timed out");
        }
    }
}

#[derive(Clone)]
pub struct HookRuntime {
    pub hooks: Vec<Hook>,
    pub run_id: String,
    pub thread_id: String,
    pub root: std::path::PathBuf,
    pub approvals: Arc<dyn ApprovalHook>,
}
tokio::task_local! { pub static ACTIVE: HookRuntime; static IN_HOOK: bool; static PROMPT_CONTEXT: String; }

/// Most context `PromptSubmit` hooks may add to a turn, in bytes.
pub const PROMPT_CONTEXT_LIMIT: usize = 4096;

/// What this run's `PromptSubmit` hooks added for the model, if anything.
pub fn prompt_context() -> Option<String> {
    PROMPT_CONTEXT
        .try_with(Clone::clone)
        .ok()
        .filter(|context| !context.is_empty())
}

/// The context a hook's output adds: `context`, `additionalContext`, or Claude's
/// `hookSpecificOutput.additionalContext`.
fn added_context(output: &Value) -> Option<&str> {
    output["context"]
        .as_str()
        .or_else(|| output["additionalContext"].as_str())
        .or_else(|| output["hookSpecificOutput"]["additionalContext"].as_str())
        .filter(|text| !text.trim().is_empty())
}

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

/// Most of a hook's block reason that is kept, in bytes.
const BLOCK_REASON_LIMIT: usize = 1024;

/// A hook asked to block the action, with the reason it gave, if any.
#[derive(Debug)]
struct Blocked(Option<String>);

impl std::fmt::Display for Blocked {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Hook blocked the action")
    }
}

impl std::error::Error for Blocked {}

/// The reason in a hook's output: `reason`, or Claude's
/// `hookSpecificOutput.permissionDecisionReason`.
fn block_reason(output: &Value) -> Option<String> {
    output["reason"]
        .as_str()
        .or_else(|| output["hookSpecificOutput"]["permissionDecisionReason"].as_str())
        .map(str::trim)
        .filter(|reason| !reason.is_empty())
        .map(|reason| capped(reason, BLOCK_REASON_LIMIT).to_owned())
}

/// What the person sees when a hook blocks a turn or a stage of the run.
fn blocked_message(hook: &str, reason: Option<&str>) -> String {
    match reason {
        Some(reason) => format!("Blocked by hook {hook}: {reason}"),
        None => format!("Blocked by hook {hook}"),
    }
}

/// What the model gets when a hook blocks a tool call: the reason is labelled and fenced so
/// it cannot pose as the note or close the fence early.
fn blocked_tool_note(hook: &str, reason: Option<&str>) -> String {
    let blocked = crate::prompts::fill(crate::prompts::HOOK_BLOCKED, &[("hook", hook)]);
    match reason {
        Some(reason) => {
            let fenced = reason.replace("<<<", "< < <").replace(">>>", "> > >");
            format!(
                "{blocked} {}\n<<<hook-reason\n{fenced}\n>>>",
                crate::prompts::HOOK_BLOCK_REASON_NOTE
            )
        }
        None => blocked,
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
impl HookRuntime {
    pub async fn emit(
        &self,
        event: &str,
        payload: Value,
        events: &tokio::sync::mpsc::Sender<crate::runtime::RunEvent>,
    ) -> anyhow::Result<()> {
        self.emit_collect(event, payload, events).await.map(|_| ())
    }

    /// Runs the `PromptSubmit` hooks for `prompt`. A hook that asks to block stops the turn;
    /// the context they add is returned, capped at [`PROMPT_CONTEXT_LIMIT`].
    pub async fn prompt_submit(
        &self,
        prompt: &str,
        events: &tokio::sync::mpsc::Sender<crate::runtime::RunEvent>,
    ) -> anyhow::Result<String> {
        let outputs = self
            .emit_collect("PromptSubmit", json!({"prompt": prompt}), events)
            .await?;
        let mut context = String::new();
        for output in outputs {
            if let Some(text) = added_context(&output) {
                if !context.is_empty() {
                    context.push('\n');
                }
                context.push_str(text.trim());
            }
        }
        Ok(capped(&context, PROMPT_CONTEXT_LIMIT).to_owned())
    }

    /// Runs the matching hooks; returns the output of each one that succeeded.
    async fn emit_collect(
        &self,
        event: &str,
        payload: Value,
        events: &tokio::sync::mpsc::Sender<crate::runtime::RunEvent>,
    ) -> anyhow::Result<Vec<Value>> {
        let mut outputs = Vec::new();
        let event = canonical_event(event);
        let mut payload = payload;
        payload["event"] = event.into();
        payload["run_id"] = self.run_id.clone().into();
        payload["thread_id"] = self.thread_id.clone().into();
        payload["project_root"] = self.root.to_string_lossy().as_ref().into();
        for hook in self.hooks.iter().filter(|h| {
            h.enabled
                && canonical_event(&h.event) == event
                && (h.event != "ToolCallFailed" || payload["ok"] == false)
                && h.matcher.as_ref().is_none_or(|matcher| {
                    matcher
                        .split('|')
                        .any(|pattern| pattern == "*" || payload["tool"].as_str() == Some(pattern))
                })
        }) {
            let outcome = IN_HOOK
                .scope(true, run(hook, &self.root, &payload, &self.approvals))
                .await;
            events
                .send(crate::runtime::RunEvent::ToolCallFinished {
                    tool: format!(
                        "hook_{}",
                        hook.runtime_identity.as_ref().unwrap_or(&hook.name)
                    ),
                    ok: outcome.is_ok(),
                    output: match &outcome {
                        Ok(v) => v.to_string(),
                        Err(e) => e.to_string(),
                    },
                })
                .await
                .ok();
            let stoppable = matches!(
                event,
                "PromptSubmit" | "RunStart" | "BeforeTool" | "BeforeCompaction"
            );
            let abort = hook.failure_policy == Some(HookFailurePolicy::Abort)
                || (hook.failure_policy.is_none() && hook.blocking);
            match outcome {
                Ok(value) => outputs.push(value),
                // Asking to block is a decision, not a failure: the failure policy has no say.
                Err(error) if stoppable => {
                    if let Some(Blocked(reason)) = error.downcast_ref::<Blocked>() {
                        let reason = reason.as_deref();
                        if event == "BeforeTool" {
                            bail!(blocked_tool_note(&hook.name, reason));
                        }
                        bail!(blocked_message(&hook.name, reason));
                    }
                    if abort {
                        if event == "PromptSubmit" {
                            bail!("Blocked by hook {}: {error}", hook.name);
                        }
                        return Err(error);
                    }
                }
                Err(_) => {}
            }
        }
        Ok(outputs)
    }
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
    // The person's message goes to the PromptSubmit hooks before the first model request.
    let context = match runtime.prompt_submit(task, &events).await {
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

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::tools::{AllowAllHook, FnHook};
    use std::sync::Mutex;

    fn hook(event: &str, command: &str) -> Hook {
        Hook {
            name: "test".into(),
            event: event.into(),
            command: command.into(),
            enabled: true,
            timeout_seconds: 1,
            blocking: false,
            matcher: None,
            failure_policy: None,
            plugin_root: None,
            runtime_identity: None,
        }
    }

    #[tokio::test]
    async fn changed_hook_definition_requires_new_approval() {
        let root = tempfile::tempdir().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let capture = seen.clone();
        let approvals = crate::runtime::CachingApprovals::wrap(Arc::new(FnHook::new(
            move |action: &ToolAction| {
                capture.lock().unwrap().push(action.tool.clone());
                crate::tools::Approval::AllowAlways
            },
        )));
        let approvals: Arc<dyn ApprovalHook> = approvals;
        run(
            &hook("RunStart", "true"),
            root.path(),
            &json!({}),
            &approvals,
        )
        .await
        .unwrap();
        run(
            &hook("RunStart", "true"),
            root.path(),
            &json!({}),
            &approvals,
        )
        .await
        .unwrap();
        run(&hook("RunStart", ":"), root.path(), &json!({}), &approvals)
            .await
            .unwrap();
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 2);
        assert_ne!(seen[0], seen[1]);
    }

    #[tokio::test]
    async fn matcher_and_cancelled_runend_use_fixed_payload() {
        let root = tempfile::tempdir().unwrap();
        let mut matched = hook("BeforeTool", "cat > matched.json");
        matched.matcher = Some("read_file".into());
        let runtime = HookRuntime {
            hooks: vec![matched, hook("RunEnd", "cat > ended.json")],
            root: root.path().into(),
            approvals: Arc::new(AllowAllHook),
            run_id: "run".into(),
            thread_id: "thread".into(),
        };
        let (tx, mut rx) = tokio::sync::mpsc::channel(32);
        runtime
            .emit("BeforeTool", json!({"tool":"shell"}), &tx)
            .await
            .unwrap();
        assert!(!root.path().join("matched.json").exists());
        runtime
            .emit("BeforeTool", json!({"tool":"read_file"}), &tx)
            .await
            .unwrap();
        let matched: Value =
            serde_json::from_slice(&std::fs::read(root.path().join("matched.json")).unwrap())
                .unwrap();
        assert_eq!(matched["event"], "BeforeTool");
        assert_eq!(matched["run_id"], "run");
        with_hooks(runtime, "task", tx, |events| async move {
            events
                .send(crate::runtime::RunEvent::Failed {
                    error: "Stopped by you".into(),
                })
                .await
                .unwrap();
            Err(anyhow::anyhow!("Stopped by you"))
        })
        .await
        .unwrap_err();
        while rx.try_recv().is_ok() {}
        let ended: Value =
            serde_json::from_slice(&std::fs::read(root.path().join("ended.json")).unwrap())
                .unwrap();
        assert_eq!(ended["event"], "RunEnd");
        assert_eq!(ended["status"], "cancelled");
        assert_eq!(ended["thread_id"], "thread");
    }

    #[tokio::test]
    async fn prompt_hooks_add_capped_context_or_block_the_turn() {
        let root = tempfile::tempdir().unwrap();
        let runtime = |hooks: Vec<Hook>| HookRuntime {
            hooks,
            root: root.path().into(),
            approvals: Arc::new(AllowAllHook),
            run_id: "run".into(),
            thread_id: "thread".into(),
        };
        let (tx, _rx) = tokio::sync::mpsc::channel(32);
        // Plain text and Claude's JSON both add context, in order; the prompt reaches them.
        let both = runtime(vec![
            hook(
                "PromptSubmit",
                "cat > seen.json; echo 'Ticket ABC-1 is open'",
            ),
            hook(
                "PromptSubmit",
                r#"echo '{"hookSpecificOutput":{"additionalContext":"Use tabs"}}'"#,
            ),
        ]);
        let context = both.prompt_submit("fix the bug", &tx).await.unwrap();
        assert_eq!(context, "Ticket ABC-1 is open\nUse tabs");
        let seen: Value =
            serde_json::from_slice(&std::fs::read(root.path().join("seen.json")).unwrap()).unwrap();
        assert_eq!(seen["prompt"], "fix the bug");
        assert_eq!(seen["event"], "PromptSubmit");
        // Context is capped.
        let long = runtime(vec![hook(
            "PromptSubmit",
            "head -c 9000 /dev/zero | tr '\\0' x",
        )]);
        assert_eq!(
            long.prompt_submit("x", &tx).await.unwrap().len(),
            PROMPT_CONTEXT_LIMIT
        );
        // Asking to block stops the turn and names the hook.
        let mut blocker = hook(
            "PromptSubmit",
            r#"echo '{"decision":"block","reason":"no"}'"#,
        );
        blocker.name = "guard".into();
        let error = runtime(vec![blocker])
            .prompt_submit("x", &tx)
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "Blocked by hook guard: no");
        // A hook that merely fails does not, unless its policy says so.
        let failing = runtime(vec![hook("PromptSubmit", "exit 3")]);
        assert_eq!(failing.prompt_submit("x", &tx).await.unwrap(), "");
    }

    #[test]
    fn hook_timeouts_accept_whole_floats_only() {
        let parse = |seconds: Value| {
            serde_json::from_value::<Hook>(
                json!({"name":"a","event":"RunEnd","command":"true","timeout_seconds":seconds}),
            )
        };
        assert_eq!(parse(json!(5)).unwrap().timeout_seconds, 5);
        assert_eq!(parse(json!(5.0)).unwrap().timeout_seconds, 5);
        assert!(parse(json!(5.5)).is_err());
        assert!(parse(json!(-1)).is_err());
    }

    #[tokio::test]
    async fn a_tool_hook_that_blocks_tells_the_model_why_in_a_labelled_fence() {
        let root = tempfile::tempdir().unwrap();
        let (tx, _rx) = tokio::sync::mpsc::channel(32);
        let runtime = |hooks| HookRuntime {
            hooks,
            run_id: "r".into(),
            thread_id: "t".into(),
            root: root.path().into(),
            approvals: Arc::new(crate::tools::AllowAllHook),
        };
        let tool_hook = |name: &str, command: &str, blocking: bool| Hook {
            name: name.into(),
            event: "BeforeTool".into(),
            command: command.into(),
            enabled: true,
            matcher: None,
            failure_policy: None,
            timeout_seconds: 10,
            blocking,
            plugin_root: None,
            runtime_identity: None,
        };
        let block = |hook: Hook| {
            let runtime = runtime(vec![hook]);
            let tx = tx.clone();
            async move {
                runtime
                    .emit("BeforeTool", json!({"tool": "shell"}), &tx)
                    .await
                    .unwrap_err()
                    .to_string()
            }
        };
        let note = block(tool_hook(
            "guard",
            r#"echo '{"decision":"block","reason":"not >>> here"}'"#,
            true,
        ))
        .await;
        assert!(
            note.starts_with("Hook guard blocked this action. Its reason"),
            "{note}"
        );
        assert!(
            note.ends_with("<<<hook-reason\nnot > > > here\n>>>"),
            "{note}"
        );
        // No reason: only the plain sentence. Claude's deny form counts, whatever the policy.
        assert_eq!(
            block(tool_hook("guard", r#"echo '{"block":true}'"#, true)).await,
            "Hook guard blocked this action."
        );
        let note = block(tool_hook(
            "deny",
            r#"echo '{"hookSpecificOutput":{"permissionDecision":"deny","permissionDecisionReason":"use rg"}}'"#,
            false,
        ))
        .await;
        assert!(note.contains("<<<hook-reason\nuse rg\n>>>"), "{note}");
    }
}
