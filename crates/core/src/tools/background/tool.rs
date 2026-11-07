//! The chat-scoped `background_task` tool and the scope `shell` starts tasks through.

use super::*;

/// The chat a tool belongs to plus the registry holding its tasks.
#[derive(Clone)]
pub struct BackgroundScope {
    pub registry: Arc<BackgroundRegistry>,
    pub thread_id: String,
    /// The run whose tasks these are.
    pub run_id: Option<String>,
    /// Only that run's tasks can be seen or stopped: a subagent's reach ends at its own.
    pub own_tasks_only: bool,
}

fn blocking_error(error: tokio::task::JoinError) -> ToolCallError {
    tool_error(format!("background_task: worker failed: {error}"))
}

impl BackgroundScope {
    /// Scope for `thread_id` backed by `registry`.
    pub fn new(registry: Arc<BackgroundRegistry>, thread_id: impl Into<String>) -> Self {
        Self {
            registry,
            thread_id: thread_id.into(),
            run_id: None,
            own_tasks_only: false,
        }
    }

    /// Only this run's tasks can be seen or stopped through this scope.
    #[must_use]
    pub fn own_tasks_only(mut self) -> Self {
        self.own_tasks_only = true;
        self
    }

    /// Tasks started through this scope record `run_id`.
    pub fn with_run(mut self, run_id: impl Into<String>) -> Self {
        self.run_id = Some(run_id.into());
        self
    }

    /// Starts a background task (the caller already obtained approval).
    pub(in crate::tools) async fn start(
        &self,
        program: String,
        args: Vec<String>,
        workdir: PathBuf,
        timeout_secs: u64,
        sandbox: Option<Arc<crate::sandbox::SandboxPlan>>,
    ) -> Result<Value, ToolCallError> {
        let registry = Arc::clone(&self.registry);
        let sandboxed = sandbox.as_ref().is_some_and(|plan| plan.is_active());
        let spec = StartSpec {
            thread_id: self.thread_id.clone(),
            run_id: self.run_id.clone(),
            program,
            args,
            workdir,
            timeout_secs,
            sandbox,
            plugin_bins: crate::plugins::hooks::plugin_bins(),
        };
        let view = tokio::task::spawn_blocking(move || registry.start(spec))
            .await
            .map_err(blocking_error)?
            .map_err(|error| tool_error(format!("shell: {error}")))?;
        Ok(json!({
            "background": true,
            "task_id": view.record.id,
            "state": view.record.state,
            "pid": view.record.pid,
            "command": view.summary,
            "timeout_secs": view.record.timeout_secs,
            "sandboxed": sandboxed,
            "next": crate::prompts::TOOL_BACKGROUND_NEXT,
        }))
    }
}

fn default_offset() -> i64 {
    -1
}

/// Arguments for [`BackgroundTool`].
#[derive(Debug, Clone, Serialize, Deserialize, ToolInput)]
pub struct BackgroundArgs {
    /// `list`, `status`, `log` or `stop`.
    #[input(description = "list | status | log | stop")]
    pub action: String,
    /// Task id returned by shell with background=true.
    #[serde(default)]
    #[input(description = "task_id from shell background=true (not needed for list)")]
    pub task_id: String,
    /// Byte offset to read the log from; negative reads the newest output.
    #[serde(default = "default_offset")]
    #[input(
        description = "log only: byte offset to read from (next_offset of the previous page); -1 (default) returns the newest output"
    )]
    pub offset: i64,
    /// Largest log page, in bytes.
    #[serde(default)]
    #[input(description = "log only: page size in bytes (default 8192, max 16384)")]
    pub max_bytes: u64,
}

/// Whether a `background_task` call is one this tool only inspects or stops with. It has no
/// way to start a program; anything that doesn't read as such a call is not vouched for.
pub fn only_inspects(arguments: &Value) -> bool {
    arguments.get("command").is_none()
        && arguments
            .get("action")
            .and_then(Value::as_str)
            .is_some_and(|action| matches!(action, "list" | "status" | "log" | "stop"))
}

/// Inspects and stops this chat's background shell tasks.
///
/// No approval prompt: it only reaches tasks this chat already started with an
/// approved `shell` call, and the output is bounded.
#[tool(
    name = "background_task",
    description = "Inspect or stop background programs started with shell background=true. list: this chat's tasks. status: state, exit code and the last output lines. log: bounded page of the rolling log (last 64 KiB; page with next_offset). stop: kills the program and everything it started.",
    input = BackgroundArgs,
)]
pub struct BackgroundTool {
    scope: Option<BackgroundScope>,
}

impl BackgroundTool {
    /// Tool for one chat; without a scope every call reports it is unavailable.
    pub fn new(scope: Option<BackgroundScope>) -> Self {
        Self { scope }
    }
}

#[async_trait]
impl ToolRuntime for BackgroundTool {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let BackgroundArgs {
            action,
            task_id,
            offset,
            max_bytes,
        } = parse_tool_args("background_task", args)?;
        let scope = self.scope.clone().ok_or_else(|| {
            tool_error("background_task: background tasks are not available in this context".into())
        })?;
        if action != "list" && task_id.trim().is_empty() {
            return Err(tool_error(format!(
                "background_task: '{action}' needs a task_id (see action=list)"
            )));
        }
        let fail = |error: String| tool_error(format!("background_task: {error}"));
        tokio::task::spawn_blocking(move || {
            let registry = &scope.registry;
            let thread = scope.thread_id.as_str();
            let mine = |record: &TaskRecord| !scope.own_tasks_only || record.run_id == scope.run_id;
            if action != "list" {
                let found = registry.status(thread, &task_id).map_err(fail)?;
                if !mine(&found.record) {
                    return Err(fail(format!(
                        "unknown background task '{task_id}' in this chat"
                    )));
                }
            }
            match action.as_str() {
                "list" => {
                    let tasks: Vec<Value> = registry
                        .list(thread, 0)
                        .into_iter()
                        .filter(|view| mine(&view.record))
                        .map(|view| {
                            json!({
                                "task_id": view.record.id,
                                "state": view.record.state,
                                "exit_code": view.record.exit_code,
                                "command": view.summary,
                                "log_bytes": view.log_bytes,
                            })
                        })
                        .collect();
                    Ok(json!({"tasks": tasks, "running": registry.running_count()}))
                }
                "status" => {
                    let view = registry.status(thread, &task_id).map_err(fail)?;
                    Ok(json!({
                        "task_id": view.record.id,
                        "state": view.record.state,
                        "exit_code": view.record.exit_code,
                        "command": view.summary,
                        "log_bytes": view.log_bytes,
                        "last_output": view.tail,
                    }))
                }
                "log" => {
                    let size = match max_bytes {
                        0 => DEFAULT_LOG_PAGE_BYTES,
                        bytes => usize::try_from(bytes).unwrap_or(MAX_LOG_PAGE_BYTES),
                    };
                    let offset = u64::try_from(offset).ok();
                    let (record, chunk) =
                        registry.log(thread, &task_id, offset, size).map_err(fail)?;
                    Ok(json!({
                        "task_id": record.id,
                        "state": record.state,
                        "exit_code": record.exit_code,
                        "text": chunk.text,
                        "offset": chunk.offset,
                        "next_offset": chunk.next_offset,
                        "total_bytes": chunk.total,
                        "oldest_retained_offset": chunk.retained_from,
                        "more": chunk.next_offset < chunk.total,
                    }))
                }
                "stop" => {
                    let view = registry.stop(thread, &task_id, STOP_WAIT).map_err(fail)?;
                    Ok(json!({
                        "task_id": view.record.id,
                        "state": view.record.state,
                        "exit_code": view.record.exit_code,
                        "last_output": view.tail,
                    }))
                }
                other => Err(tool_error(format!(
                    "background_task: unknown action '{other}' (use list, status, log or stop)"
                ))),
            }
        })
        .await
        .map_err(blocking_error)?
    }
}
