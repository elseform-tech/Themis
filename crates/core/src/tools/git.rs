use super::*;

#[derive(Debug, Clone, Serialize, Deserialize, ToolInput)]
pub struct GitArgs {
    /// Git subcommand and flags, e.g. `["status", "--short"]`.
    #[input(description = "Git subcommand and flags, e.g. [\"status\", \"--short\"]")]
    pub args: Vec<String>,
}

/// Runs allowlisted `git` subcommands with `-C <root>`.
///
/// Allowlist: `status`, `diff`, `log`, `show` are auto-allowed
/// ([`RiskLevel::Read`]); `add`, `commit`, `checkout`, `branch` require
/// approval ([`RiskLevel::Write`]); everything else is denied outright.
/// The child runs with a scrubbed environment and a [`SHELL_TIMEOUT_SECS`]
/// kill deadline (shared runner with [`ShellTool`]).
#[tool(
    name = "git",
    description = "Run allowlisted git subcommands (status/diff/log/show auto-allowed; add/commit/checkout/branch need approval; all else denied).",
    input = GitArgs,
)]
pub struct GitTool {
    root: PathBuf,
    approvals: Arc<dyn ApprovalHook>,
    unrestricted: bool,
    sandbox: Option<Arc<SandboxPlan>>,
    escalate: bool,
}

impl GitTool {
    pub fn with_unrestricted_commands(mut self, unrestricted: bool) -> Self {
        self.unrestricted = unrestricted;
        self
    }
    /// Create a git tool rooted at `root`.
    pub fn new(root: PathBuf, approvals: Arc<dyn ApprovalHook>) -> Self {
        Self {
            root,
            approvals,
            unrestricted: false,
            sandbox: None,
            escalate: false,
        }
    }

    /// Runs git inside `plan`; see [`ShellTool::with_sandbox`].
    #[must_use]
    pub fn with_sandbox(mut self, plan: Arc<SandboxPlan>, escalate: bool) -> Self {
        self.sandbox = Some(plan);
        self.escalate = escalate;
        self
    }

    /// The project root this tool is scoped to.
    pub fn root(&self) -> &Path {
        &self.root
    }
}

#[async_trait]
impl ToolRuntime for GitTool {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let raw = args.clone();
        let GitArgs { args } = parse_tool_args("git", args)?;
        let Some(subcommand) = args.first().map(String::as_str) else {
            return Err(tool_error("git: no subcommand given".to_string()));
        };
        let root =
            resolve_within_root(&self.root, ".").map_err(|e| tool_error(format!("git: {e}")))?;
        let mut command = vec!["git".to_string()];
        command.extend(args.iter().cloned());
        let destructive = is_destructive_variant(&command);
        let risk = match subcommand {
            _ if self.unrestricted => RiskLevel::Execute,
            "checkout" | "branch" | "commit" if destructive => RiskLevel::Destructive,
            "status" | "diff" | "log" | "show" => RiskLevel::Read,
            "add" | "commit" | "checkout" | "branch" => RiskLevel::Write,
            other => return Err(tool_error(git_refusal(other))),
        };
        let action = ToolAction {
            display: Some(git_display(&args, &root)),
            command,
            tool: "git".into(),
            summary: format!("run `git {}`", args.join(" ")),
            risk,
        };
        check_call_permitted(self.approvals.as_ref(), &action, &raw)?;
        if let Some(refusal) = self.sandbox.as_ref().and_then(|plan| plan.refusal()) {
            return Err(tool_error(format!("git: {refusal}")));
        }

        let mut full_args = vec!["-C".to_string(), root.to_string_lossy().into_owned()];
        full_args.extend(args);
        let out = run_command(
            "git".to_string(),
            full_args.clone(),
            root.clone(),
            "git",
            None,
            self.sandbox.clone(),
        )
        .await?;
        match sandbox_outcome(
            out,
            self.sandbox.as_deref(),
            self.escalate,
            self.approvals.as_ref(),
            &action,
            &root,
        ) {
            SandboxOutcome::Done(value) => Ok(value),
            SandboxOutcome::RunOutside => {
                let out =
                    run_command("git".to_string(), full_args, root, "git", None, None).await?;
                let mut value = command_json(&out);
                value["ran_outside_sandbox"] = json!(true);
                Ok(value)
            }
        }
    }
}
