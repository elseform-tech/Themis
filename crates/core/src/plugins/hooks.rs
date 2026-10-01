//! Bounded hook commands. Hooks cannot grant tool permissions.
use crate::tools::{Approval, ApprovalHook, RiskLevel, ToolAction};
use anyhow::{bail, Context};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::Path;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub const EVENTS: [&str; 6] = [
    "RunStart",
    "BeforeToolCall",
    "AfterToolCall",
    "ToolCallFailed",
    "RunFinished",
    "BeforeCompaction",
];
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hook {
    pub name: String,
    pub event: String,
    pub command: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "timeout")]
    pub timeout_seconds: u64,
    #[serde(default)]
    pub blocking: bool,
    #[serde(skip)]
    pub plugin_root: Option<std::path::PathBuf>,
    #[serde(skip)]
    pub runtime_identity: Option<String>,
}
fn timeout() -> u64 {
    10
}
impl Hook {
    pub fn validate(&self) -> anyhow::Result<()> {
        if !super::safe_name(&self.name)
            || !EVENTS.contains(&self.event.as_str())
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
    let action = ToolAction {
        tool: format!(
            "hook_{}",
            hook.runtime_identity.as_ref().unwrap_or(&hook.name)
        ),
        summary: format!("{}: {}", hook.event, hook.command),
        risk: RiskLevel::Execute,
    };
    if approvals.approve(&action) == Approval::Deny {
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
        c.args(["/C", &hook.command]);
        c
    };
    command
        .current_dir(root)
        .env_clear()
        .kill_on_drop(true)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
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
            let value: Value = serde_json::from_str(&text).context("Hook output must be JSON")?;
            if value.get("block").and_then(Value::as_bool) == Some(true) {
                bail!("Hook blocked the action");
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
    pub root: std::path::PathBuf,
    pub approvals: Arc<dyn ApprovalHook>,
}
tokio::task_local! { pub static ACTIVE: HookRuntime; }
pub async fn emit_current(
    event: &str,
    payload: Value,
    events: &tokio::sync::mpsc::Sender<crate::runtime::RunEvent>,
) -> anyhow::Result<()> {
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
        for hook in self.hooks.iter().filter(|h| h.enabled && h.event == event) {
            let outcome = run(hook, &self.root, &payload, &self.approvals).await;
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
            if hook.blocking && matches!(event, "RunStart" | "BeforeToolCall" | "BeforeCompaction")
            {
                outcome?;
            }
        }
        Ok(())
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
                "RunFinished",
                json!({"event":"RunFinished","ok":false,"error":error.to_string()}),
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
                        "RunFinished",
                        json!({"event":"RunFinished","result":event}),
                        &outgoing,
                    )
                    .await
                    .ok();
            }
            outgoing.send(event).await.ok();
        }
    });
    let result = ACTIVE.scope(runtime.clone(), run(tx)).await;
    let _ = pump.await;
    if !terminal.load(std::sync::atomic::Ordering::SeqCst) {
        runtime
            .emit(
                "RunFinished",
                json!({"event":"RunFinished","ok":result.is_ok()}),
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
