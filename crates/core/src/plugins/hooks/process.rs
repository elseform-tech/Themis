use super::*;

pub async fn run(
    hook: &Hook,
    root: &Path,
    payload: &Value,
    approvals: &Arc<dyn ApprovalHook>,
) -> anyhow::Result<Value> {
    authorize(hook, approvals)?;
    execute(hook, root, payload).await
}

/// A hook and its exact definition, as approvals see it: another definition is another hook.
#[must_use]
pub fn hook_identity(hook: &Hook) -> String {
    use std::hash::{Hash, Hasher};
    let mut definition = std::collections::hash_map::DefaultHasher::new();
    serde_json::to_string(hook)
        .unwrap_or_default()
        .hash(&mut definition);
    format!(
        "hook_{}_{:016x}",
        hook.runtime_identity.as_ref().unwrap_or(&hook.name),
        definition.finish()
    )
}

/// Validates a hook and asks whether it may run.
pub(super) fn authorize(hook: &Hook, approvals: &Arc<dyn ApprovalHook>) -> anyhow::Result<()> {
    hook.validate()?;
    let what = hook.prompt.as_deref().unwrap_or(&hook.command);
    let action = ToolAction {
        display: Some(crate::tools::CommandDisplay {
            argv: vec![what.to_owned()],
            cwd: None,
            background: false,
            sandbox_blocked: None,
        }),
        command: Vec::new(),
        tool: hook_identity(hook),
        summary: format!("{}: {what}", hook.event),
        risk: RiskLevel::Execute,
    };
    if approvals.approve(&action).is_denied() {
        bail!("Hook execution denied");
    }
    Ok(())
}

/// Most of a hook's stderr that is kept, in bytes.
const STDERR_LIMIT: usize = 4096;

/// Drains a hook's stderr, keeping up to `limit` bytes of its start so a refusal can give the
/// reason.
fn keep_stderr(
    mut stderr: tokio::process::ChildStderr,
    limit: usize,
) -> (Arc<std::sync::Mutex<Vec<u8>>>, tokio::task::JoinHandle<()>) {
    let kept = Arc::new(std::sync::Mutex::new(Vec::new()));
    let filling = Arc::clone(&kept);
    let reader = tokio::spawn(async move {
        let mut chunk = [0u8; 1024];
        while let Ok(count) = stderr.read(&mut chunk).await {
            if count == 0 {
                break;
            }
            let mut kept = filling.lock().unwrap_or_else(|e| e.into_inner());
            let room = limit.saturating_sub(kept.len());
            kept.extend_from_slice(&chunk[..count.min(room)]);
        }
        let kept = filling.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if !kept.is_empty() {
            crate::diagnostics::capture_stderr(std::io::Cursor::new(kept), "hook", Vec::new());
        }
    });
    (kept, reader)
}

/// Runs an authorized hook.
pub(super) async fn execute(hook: &Hook, root: &Path, payload: &Value) -> anyhow::Result<Value> {
    #[cfg(not(windows))]
    let mut command = {
        let mut c = tokio::process::Command::new(hook.shell.as_deref().unwrap_or("sh"));
        c.args(["-c", &hook.command]);
        c
    };
    #[cfg(windows)]
    let mut command = if let Some(shell) = &hook.shell {
        let mut c = tokio::process::Command::new(shell);
        c.args(["-c", &hook.command]);
        c
    } else {
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
    command.env("CLAUDE_PROJECT_DIR", root);
    if let Some(path) = &hook.plugin_data {
        std::fs::create_dir_all(path)?;
        command.env("CLAUDE_PLUGIN_DATA", path);
    }
    if let Some(file) = &hook.env_file {
        let (variables, refused) = file.read();
        command.envs(variables);
        if !refused.is_empty() {
            command.env("THEMIS_ENV_REFUSED", refused.join(","));
        }
        // Claude gives it only to session hooks.
        if hook.event == "SessionStart" {
            if let Some(parent) = file.path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            command.env("CLAUDE_ENV_FILE", &file.path);
        }
    }
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command.spawn()?;
    let stderr_limit = if hook.rewake.is_some() {
        REWAKE_OUTPUT_LIMIT
    } else {
        STDERR_LIMIT
    };
    let stderr = child
        .stderr
        .take()
        .map(|stderr| keep_stderr(stderr, stderr_limit));
    let mut group = ProcessGroup(child.id());
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
            // Only exit code 2 reports; whatever else it printed is its own business.
            if hook.rewake.is_some() && status.code() != Some(2) {
                return if status.success() {
                    Ok(json!({"ok":true}))
                } else {
                    Err(anyhow::anyhow!("Hook exited unsuccessfully ({status})"))
                };
            }
            if hook.exit_two_blocks && status.code() == Some(2) {
                let mut reason = String::new();
                if let Some((kept, reader)) = stderr {
                    // A process the hook left behind may hold stderr open: don't wait for it.
                    let _ =
                        tokio::time::timeout(std::time::Duration::from_millis(200), reader).await;
                    let kept = kept.lock().unwrap_or_else(|e| e.into_inner());
                    reason = String::from_utf8_lossy(&kept).trim().to_owned();
                }
                if hook.rewake.is_some() {
                    return Err(HookReport::read(&reason, &output).into());
                }
                return Err(Blocked(
                    (!reason.is_empty()).then(|| capped(&reason, reason_limit(hook)).to_owned()),
                )
                .into());
            }
            if !status.success() {
                bail!("Hook exited unsuccessfully ({status})");
            }
            let text = String::from_utf8(output)?;
            if text.trim().is_empty() {
                return Ok(json!({"ok":true}));
            }
            // These may print plain text: that is context for the turn.
            let prompt = matches!(
                canonical_event(&hook.event),
                "PromptSubmit" | "SessionStart" | "UserPromptExpansion"
            );
            let value: Value = match serde_json::from_str(&text) {
                Ok(value) => value,
                Err(_) if prompt => json!({"context": text}),
                Err(error) => return Err(error).context("Hook output must be JSON"),
            };
            if value.get("block").and_then(Value::as_bool) == Some(true)
                || value.get("decision").and_then(Value::as_str) == Some("block")
                || value["hookSpecificOutput"]["permissionDecision"] == "deny"
            {
                return Err(Blocked(block_reason(&value, reason_limit(hook))).into());
            }
            Ok(value)
        },
    )
    .await;
    match outcome {
        Ok(value) => {
            // It ended by itself, so what it detached on purpose (a save handed off so that the
            // hook returns at once, as Claude's short budgets make plugins do) is left alone. A
            // hook cut short keeps the group and takes everything with it.
            if matches!(child.try_wait(), Ok(Some(_))) {
                group.0 = None;
            }
            value
        }
        Err(_) => {
            let _ = child.kill().await;
            bail!("Hook timed out");
        }
    }
}

/// What a reporting hook left when it exited 2: stderr, or stdout when that is empty, and a
/// `rewakeSummary` from the first JSON line of stdout.
#[derive(Debug)]
pub(super) struct HookReport {
    pub(super) output: String,
    pub(super) summary: Option<String>,
}

impl std::fmt::Display for HookReport {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Hook reported back")
    }
}

impl std::error::Error for HookReport {}

impl HookReport {
    pub(super) fn read(stderr: &str, stdout: &[u8]) -> Self {
        let stdout = String::from_utf8_lossy(stdout);
        let summary = stdout
            .lines()
            .find(|line| line.trim_start().starts_with('{'))
            .and_then(|line| serde_json::from_str::<Value>(line).ok())
            .and_then(|line| {
                line["rewakeSummary"]
                    .as_str()
                    .map(str::trim)
                    .map(str::to_owned)
            })
            .filter(|summary| !summary.is_empty());
        let output = if stderr.trim().is_empty() {
            stdout.trim()
        } else {
            stderr.trim()
        };
        Self {
            output: capped(output, REWAKE_OUTPUT_LIMIT).to_owned(),
            summary,
        }
    }
}

/// Finishes when the flag goes up; never without one.
pub(super) async fn stopped(flag: Option<Arc<std::sync::atomic::AtomicBool>>) {
    match flag {
        Some(flag) => {
            while !flag.load(std::sync::atomic::Ordering::SeqCst) {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        }
        None => std::future::pending::<()>().await,
    }
}
