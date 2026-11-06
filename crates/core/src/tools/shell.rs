use super::*;

// ---------------------------------------------------------------------------
// Shell tool
// ---------------------------------------------------------------------------

/// Arguments for [`ShellTool`].
#[derive(Debug, Clone, Serialize, Deserialize, ToolInput)]
pub struct ShellArgs {
    /// Program to execute (resolved via `PATH`; never interpreted by a shell).
    #[input(
        description = "Program name or path ONLY, e.g. \"python3\" or \"git\" (resolved via PATH; no shell interpretation). Never include arguments or spaces here; put them in args"
    )]
    pub command: String,
    /// Arguments passed verbatim to the program.
    #[serde(default, deserialize_with = "argv_or_json_text")]
    #[input(
        description = "Every argument as a separate string, passed verbatim, e.g. [\"-m\", \"http.server\", \"8000\"]. Do not join several arguments into one string"
    )]
    pub args: Vec<String>,
    /// Working directory, relative to the project root; empty means the root.
    #[serde(default)]
    #[input(description = "Working directory relative to the project root; empty means the root")]
    pub cwd: String,
    /// Start the program and return at once with a task id instead of waiting.
    #[serde(default)]
    #[input(
        description = "true starts the program in the background (dev servers, watchers) and returns a task_id immediately; manage it with the background_task tool. false (default) waits for it to finish"
    )]
    pub background: bool,
    /// Deadline for a background task, in seconds; 0 means the 2 hour default.
    #[serde(default)]
    #[input(
        description = "Only with background=true: kill the task after this many seconds (0 = default 7200, max 86400)"
    )]
    pub background_timeout_secs: u64,
}

/// Runs a program with argv (no shell string) and `cwd` pinned inside root.
///
/// Always requires approval ([`RiskLevel::Execute`]). Combined stdout/stderr
/// is captured up to [`MAX_COMMAND_OUTPUT`] bytes. The child runs with a
/// scrubbed environment (see [`scrubbed_env`]) and is killed past the
/// execution timeout ([`SHELL_TIMEOUT_SECS`] by default).
#[tool(
    name = "shell",
    description = "Run a program with explicit argv and a project-root working directory (no shell). `command` is ONLY the program name; every argument is its own element of `args`. Example: command \"python3\", args [\"-m\", \"http.server\", \"8000\"] (never command \"python3 -m http.server 8000\"). Pipes, redirects and `&` are literal; use command \"sh\", args [\"-c\", \"...\"] for those. Always requires approval. Waits for the program to finish; for dev servers and watchers pass background=true to get a task_id at once, then use the background_task tool.",
    input = ShellArgs,
)]
pub struct ShellTool {
    root: PathBuf,
    approvals: Arc<dyn ApprovalHook>,
    timeout: Duration,
    unrestricted: bool,
    background: Option<background::BackgroundScope>,
    cancel: Option<Arc<AtomicBool>>,
    sandbox: Option<Arc<SandboxPlan>>,
    escalate: bool,
}

impl ShellTool {
    /// Create a shell tool rooted at `root` with the default execution timeout.
    pub fn new(root: PathBuf, approvals: Arc<dyn ApprovalHook>) -> Self {
        Self {
            root,
            approvals,
            timeout: Duration::from_secs(SHELL_TIMEOUT_SECS),
            unrestricted: false,
            background: None,
            cancel: None,
            sandbox: None,
            escalate: false,
        }
    }

    /// Runs commands inside `plan`. With `escalate` (Custom mode) a command the
    /// sandbox blocked is offered to run outside it through the approval hook
    /// (tool [`UNSANDBOXED_TOOL`]); without it (YOLO) the failure, with a short
    /// explanation, goes back to the agent.
    #[must_use]
    pub fn with_sandbox(mut self, plan: Arc<SandboxPlan>, escalate: bool) -> Self {
        self.sandbox = Some(plan);
        self.escalate = escalate;
        self
    }

    /// Enables `background: true`, owned by the chat in `scope`.
    #[must_use]
    pub fn with_background(mut self, scope: background::BackgroundScope) -> Self {
        self.background = Some(scope);
        self
    }

    /// Kills a running blocking command as soon as `cancel` is set (Stop).
    #[must_use]
    pub fn with_cancel(mut self, cancel: Arc<AtomicBool>) -> Self {
        self.cancel = Some(cancel);
        self
    }

    pub fn with_unrestricted_paths(mut self, unrestricted: bool) -> Self {
        self.unrestricted = unrestricted;
        self
    }

    /// Override the execution timeout (tests use a short budget).
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// The execution timeout: a child still running past it is killed.
    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    /// The project root this tool is scoped to.
    pub fn root(&self) -> &Path {
        &self.root
    }
}

#[async_trait]
impl ToolRuntime for ShellTool {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let raw = args.clone();
        let ShellArgs {
            command,
            args,
            cwd,
            background,
            background_timeout_secs,
        } = parse_tool_args("shell", args)?;
        let mut argv = normalize_argv(std::iter::once(command).chain(args).collect());
        let command = argv.remove(0);
        let args = argv;
        if command.trim().is_empty() {
            return Err(tool_error("shell: 'command' must not be empty".to_string()));
        }
        let dir = if cwd.trim().is_empty() {
            "."
        } else {
            cwd.as_str()
        };
        let workdir = resolve_tool_path(&self.root, dir, self.unrestricted)
            .map_err(|e| tool_error(format!("shell: {e}")))?;
        if !workdir.is_dir() {
            return Err(tool_error(format!(
                "shell: working directory '{}' is not a directory",
                workdir.display()
            )));
        }
        let argv: Vec<String> = std::iter::once(command.clone())
            .chain(args.iter().cloned())
            .collect();
        let action = ToolAction {
            display: Some(CommandDisplay {
                argv: argv.clone(),
                cwd: Some(workdir.display().to_string()),
                background,
                sandbox_blocked: None,
            }),
            risk: shell_risk(&argv),
            command: argv,
            tool: "shell".to_string(),
            summary: format!(
                "run `{command} {}` in {}{}",
                args.join(" "),
                workdir.display(),
                if background { " in the background" } else { "" }
            ),
        };
        if background && self.background.is_none() {
            return Err(tool_error(
                "shell: background tasks are not available in this context".to_string(),
            ));
        }
        check_call_permitted(self.approvals.as_ref(), &action, &raw)?;
        if let Some(refusal) = self.sandbox.as_ref().and_then(|plan| plan.refusal()) {
            return Err(tool_error(format!("shell: {refusal}")));
        }

        if background {
            let scope = self.background.clone().expect("checked above");
            return scope
                .start(
                    command,
                    args,
                    workdir,
                    background_timeout_secs,
                    self.sandbox.clone(),
                )
                .await;
        }
        let run = |sandbox: Option<Arc<SandboxPlan>>| {
            run_command_with_timeout(
                command.clone(),
                args.clone(),
                workdir.clone(),
                "shell",
                self.timeout,
                self.cancel.clone(),
                sandbox,
            )
        };
        let out = run(self.sandbox.clone()).await?;
        let result = sandbox_outcome(
            out,
            self.sandbox.as_deref(),
            self.escalate,
            self.approvals.as_ref(),
            &action,
            &workdir,
        );
        match result {
            SandboxOutcome::Done(value) => Ok(value),
            SandboxOutcome::RunOutside => {
                let out = run(None).await?;
                let mut value = command_json(&out);
                value["ran_outside_sandbox"] = json!(true);
                Ok(value)
            }
        }
    }
}

/// Approval identity for running a command outside the sandbox after the
/// sandbox blocked it. Its grants and rules are separate from `shell`: an
/// approval to run `cargo test` does not approve running it unsandboxed.
pub const UNSANDBOXED_TOOL: &str = "shell_unsandboxed";

/// What to do with a finished sandboxed command.
pub(super) enum SandboxOutcome {
    Done(Value),
    /// The person approved running it again outside the sandbox.
    RunOutside,
}

pub(super) fn command_json(out: &CommandOutput) -> Value {
    json!({
        "success": out.success,
        "exit_code": out.exit_code,
        "output": out.output,
        "truncated": out.truncated,
    })
}

/// Inspects a finished command run under `sandbox`: when the sandbox blocked
/// it, either asks (`escalate`, Custom mode) whether to run it outside the
/// sandbox, or explains the block to the agent (YOLO). The escalation request
/// is the original action renamed to [`UNSANDBOXED_TOOL`], so its grant key
/// follows the usual shell rules and destructive commands stay non-grantable.
pub(super) fn sandbox_outcome(
    mut out: CommandOutput,
    sandbox: Option<&SandboxPlan>,
    escalate: bool,
    approvals: &dyn ApprovalHook,
    action: &ToolAction,
    workdir: &Path,
) -> SandboxOutcome {
    let Some(plan) = sandbox else {
        return SandboxOutcome::Done(command_json(&out));
    };
    let Some(reason) = plan.denial(out.success, &out.output, workdir) else {
        if let Some(reason) = plan.blocked_in_success(out.success, &out.output, workdir) {
            out.output.push('\n');
            out.output.push_str(&crate::prompts::fill(
                crate::prompts::TOOL_SANDBOX_BLOCKED_PARTLY,
                &[("reason", &reason)],
            ));
            let mut value = command_json(&out);
            value["sandbox_blocked"] = json!(reason);
            return SandboxOutcome::Done(value);
        }
        if plan.may_have_blocked(out.success, &out.output) {
            out.output.push('\n');
            out.output.push_str(crate::prompts::TOOL_SANDBOX_NOTE);
        }
        return SandboxOutcome::Done(command_json(&out));
    };
    if escalate {
        let mut display = action.display.clone().unwrap_or(CommandDisplay {
            argv: action.command.clone(),
            cwd: Some(workdir.display().to_string()),
            background: false,
            sandbox_blocked: None,
        });
        display.sandbox_blocked = Some(reason.clone());
        let request = ToolAction {
            tool: UNSANDBOXED_TOOL.to_owned(),
            summary: format!(
                "Run outside the sandbox? `{}` was blocked: {reason}",
                action.command.join(" ")
            ),
            display: Some(display),
            // Leaving the sandbox is at least an execution, whatever the
            // original call was (a read-only `git status` must not escalate
            // as a read that policies and dialogs pass silently).
            risk: match action.risk {
                RiskLevel::Destructive => RiskLevel::Destructive,
                _ => RiskLevel::Execute,
            },
            ..action.clone()
        };
        if check_approval(approvals, &request).is_ok() {
            return SandboxOutcome::RunOutside;
        }
    }
    out.output.push_str(&sandbox_note(&reason, escalate));
    let mut value = command_json(&out);
    value["sandbox_blocked"] = json!(reason);
    SandboxOutcome::Done(value)
}
