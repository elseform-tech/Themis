//! Coding toolset: root-scoped filesystem, shell, patch, and git tools.
//!
//! Every tool in this module is scoped to a project root: any path that
//! escapes the root — via `..`, an absolute path, or a symlink — is rejected.
//! Every tool call goes through an [`ApprovalHook`] first: the custom
//! `shell`/`apply_patch`/`git` tools gate internally, and the third-party
//! filesystem tools are wrapped in [`GatedTool`] (their own `execute`
//! never consults our hook, so ungated use would bypass approvals).
//!
//! Start here: [`boxed_tools`] builds the full set as
//! `Vec<Box<dyn ToolT>>`, which plugs directly into a ReAct run via
//! `Context::with_tools(...)` without the `#[agent(...)]` macro (see
//! [`boxed_tools`] docs for both wirings).

mod patch;

use patch::*;

use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub use autoagents::async_trait;
use autoagents_derive::{tool, ToolInput};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub use autoagents::core::tool::ToolCallError;
pub use autoagents::core::tool::{ToolRuntime, ToolT};
pub use autoagents_toolkit::tools::filesystem::{
    CopyFile, CreateDir, DeleteFile, ListDir, MoveFile, ReadFile, SearchFile, WriteFile,
};

// ---------------------------------------------------------------------------
// Approval model
// ---------------------------------------------------------------------------

/// Risk classification for a proposed tool action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RiskLevel {
    /// Reads state without modifying anything.
    Read,
    /// Modifies files or VCS state in expected ways.
    Write,
    /// Executes an external program.
    Execute,
    /// Touches the network.
    Network,
    /// Irreversible or hard-to-reverse (recursive delete, force push, ...).
    Destructive,
}

/// Management tools carry distinct approvals for reads, changes and executable tests.
pub fn integration_risk(name: &str, args: &Value) -> Option<RiskLevel> {
    if !matches!(
        name,
        "list_plugins" | "save_skill" | "manage_integrations" | "manage_automations"
    ) {
        return None;
    }
    let action = match name {
        "save_skill" => "save_skill",
        "list_plugins" => "list",
        _ => args["action"].as_str().unwrap_or("list"),
    };
    Some(match action {
        "list" | "marketplaces" => RiskLevel::Read,
        "catalog" | "preview" | "install" | "update" if name == "manage_integrations" => {
            RiskLevel::Network
        }
        "import_repository" | "preview_repository" => RiskLevel::Network,
        "test_hook" => RiskLevel::Execute,
        "test_mcp" if args["server"]["command"].is_string() => RiskLevel::Execute,
        "test_mcp" => RiskLevel::Network,
        "delete" | "remove_marketplace" | "remove_component" => RiskLevel::Destructive,
        _ => RiskLevel::Write,
    })
}

pub fn integration_approval_identity(name: &str, args: &Value) -> String {
    if matches!(name, "manage_integrations" | "manage_automations") {
        use std::hash::{Hash, Hasher};
        let action = args["action"].as_str().unwrap_or("list");
        let mut definition = std::collections::hash_map::DefaultHasher::new();
        if matches!(action, "test_hook" | "test_mcp") {
            args.to_string().hash(&mut definition);
        }
        format!(
            "{}_{action}_{:?}_{:016x}",
            name,
            integration_risk(name, args),
            definition.finish()
        )
    } else {
        name.into()
    }
}

/// A single proposed action awaiting an approval decision.
#[derive(Debug, Clone)]
pub struct ToolAction {
    /// Tool name, e.g. `"shell"`.
    pub tool: String,
    /// One-line human-readable summary shown in approval UI.
    pub summary: String,
    /// Risk classification.
    pub risk: RiskLevel,
}

/// Decides whether a proposed [`ToolAction`] may run.
///
/// The desktop app implements this with a dialog (allow-once /
/// allow-always-for-thread / deny); headless callers use [`AllowAllHook`],
/// [`DenyAllHook`], or the [`FnHook`] closure adapter.
pub trait ApprovalHook: Send + Sync {
    /// Return the approval decision for `action`.
    fn approve(&self, action: &ToolAction) -> Approval;
}

/// Decision returned by an [`ApprovalHook`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Approval {
    /// Run this action once; ask again next time.
    AllowOnce,
    /// Run this action and remember the choice for equivalent actions.
    AllowAlways,
    /// Refuse to run this action.
    Deny,
}

/// [`ApprovalHook`] adapter over a closure.
pub struct FnHook<F>
where
    F: Fn(&ToolAction) -> Approval + Send + Sync,
{
    decide: F,
}

impl<F> FnHook<F>
where
    F: Fn(&ToolAction) -> Approval + Send + Sync,
{
    /// Wrap a closure as an approval hook.
    pub fn new(decide: F) -> Self {
        Self { decide }
    }
}

impl<F> ApprovalHook for FnHook<F>
where
    F: Fn(&ToolAction) -> Approval + Send + Sync,
{
    fn approve(&self, action: &ToolAction) -> Approval {
        (self.decide)(action)
    }
}

/// Approves every action. For tests and explicitly trusted headless runs.
#[derive(Debug, Clone, Copy, Default)]
pub struct AllowAllHook;

impl ApprovalHook for AllowAllHook {
    fn approve(&self, _action: &ToolAction) -> Approval {
        Approval::AllowAlways
    }
}

/// Denies every action. For tests.
#[derive(Debug, Clone, Copy, Default)]
pub struct DenyAllHook;

impl ApprovalHook for DenyAllHook {
    fn approve(&self, _action: &ToolAction) -> Approval {
        Approval::Deny
    }
}

fn check_approval(hook: &dyn ApprovalHook, action: &ToolAction) -> Result<(), ToolCallError> {
    match hook.approve(action) {
        Approval::AllowOnce | Approval::AllowAlways => Ok(()),
        Approval::Deny => Err(tool_error(format!(
            "approval denied for '{}': {}",
            action.tool, action.summary
        ))),
    }
}

tokio::task_local! {
    static DISPATCH_PERMIT: std::sync::Arc<std::sync::atomic::AtomicBool>;
}

/// Runs `fut` with a single-use dispatch permit: the next approval check in
/// this task scope passes without consulting the hook.
///
/// The runtime consults the hook once at dispatch (emitting the decided
/// event); tools consult again at execution (defense in depth for direct
/// `execute` calls). Without the permit, one agent-driven call would prompt
/// twice. Direct calls outside a permit scope always consult the hook.
pub async fn with_dispatch_permit<F: std::future::Future>(fut: F) -> F::Output {
    DISPATCH_PERMIT
        .scope(
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
            fut,
        )
        .await
}

/// Consumes the dispatch permit when present. `true` means this check was
/// pre-approved at dispatch and must skip the hook.
fn take_dispatch_permit() -> bool {
    DISPATCH_PERMIT
        .try_with(|permit| permit.swap(false, std::sync::atomic::Ordering::SeqCst))
        .unwrap_or(false)
}

/// [`check_approval`], honoring a dispatch permit first.
///
/// All in-`execute` approval checks must use this (never `check_approval`
/// directly) so agent-driven calls prompt exactly once while direct
/// `execute` calls stay fully gated.
pub fn check_approval_permitted(
    hook: &dyn ApprovalHook,
    action: &ToolAction,
) -> Result<(), ToolCallError> {
    if take_dispatch_permit() {
        return Ok(());
    }
    check_approval(hook, action)
}

fn tool_error(message: String) -> ToolCallError {
    ToolCallError::RuntimeError(message.into())
}

// ---------------------------------------------------------------------------
// Approval-gating wrapper for third-party tools
// ---------------------------------------------------------------------------

/// Builds a one-line summary for a gated tool call.
///
/// Receives the inner tool's name and the raw JSON args; the result becomes
/// the [`ToolAction::summary`] shown to the approval hook.
pub type SummaryFn = fn(&str, &Value) -> String;

/// Wraps a third-party [`ToolT`] whose `execute` never consults our
/// [`ApprovalHook`] so every call is approval-gated first.
///
/// Metadata (`name`, `description`, `args_schema`, `output_schema`) delegates
/// to the inner tool, so wrapping is transparent to tool listings, skill
/// filters, and LLM bindings. `execute` builds a [`ToolAction`] from the
/// inner name, `risk`, and `summarize`, consults the hook (`AllowOnce` /
/// `AllowAlways` proceed; `Deny` fails with an approval-mentioning error),
/// then delegates to the inner tool.
///
/// [`boxed_tools`] wraps every `autoagents-toolkit` filesystem tool in this;
/// the custom [`ShellTool`], [`ApplyPatchTool`], and [`GitTool`] gate
/// internally and are intentionally *not* double-wrapped.
pub struct GatedTool {
    inner: Box<dyn ToolT>,
    approvals: Arc<dyn ApprovalHook>,
    risk: RiskLevel,
    summarize: SummaryFn,
}

impl GatedTool {
    /// Wrap `inner` with `risk` and the default filesystem summary builder.
    pub fn new(inner: Box<dyn ToolT>, approvals: Arc<dyn ApprovalHook>, risk: RiskLevel) -> Self {
        Self {
            inner,
            approvals,
            risk,
            summarize: summarize_fs_call,
        }
    }

    /// Wrap `inner` with `risk` and a custom summary builder.
    pub fn with_summary(
        inner: Box<dyn ToolT>,
        approvals: Arc<dyn ApprovalHook>,
        risk: RiskLevel,
        summarize: SummaryFn,
    ) -> Self {
        Self {
            inner,
            approvals,
            risk,
            summarize,
        }
    }

    /// The risk level this wrapper gates under.
    pub fn risk(&self) -> RiskLevel {
        self.risk
    }
}

impl std::fmt::Debug for GatedTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GatedTool")
            .field("tool", &self.inner.name())
            .field("risk", &self.risk)
            .finish_non_exhaustive()
    }
}

impl ToolT for GatedTool {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn description(&self) -> &str {
        self.inner.description()
    }

    fn args_schema(&self) -> Value {
        self.inner.args_schema()
    }

    fn output_schema(&self) -> Option<Value> {
        self.inner.output_schema()
    }
}

#[async_trait]
impl ToolRuntime for GatedTool {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let name = self.inner.name();
        let action = ToolAction {
            tool: name.to_owned(),
            summary: (self.summarize)(name, &args),
            risk: self.risk,
        };
        check_approval_permitted(self.approvals.as_ref(), &action)?;
        self.inner.execute(args).await
    }
}

/// One-line summary for filesystem toolkit calls.
///
/// Unknown tool names and malformed args fall back to a capped
/// ``call `<name>` with <args>`` rendering so the approval UI always shows
/// something meaningful.
fn summarize_fs_call(name: &str, args: &Value) -> String {
    fn field<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
        args.get(key).and_then(Value::as_str)
    }
    fn flag(args: &Value, key: &str) -> bool {
        args.get(key).and_then(Value::as_bool).unwrap_or(false)
    }
    let summary = match name {
        "write_file" => field(args, "file_path").map(|path| {
            if flag(args, "append") {
                format!("append to `{path}`")
            } else {
                format!("write `{path}`")
            }
        }),
        "read_file" => field(args, "file_path").map(|path| format!("read `{path}`")),
        "list_dir" => field(args, "directory_path").map(|path| format!("list `{path}`")),
        "search_file" => match (field(args, "directory"), field(args, "pattern")) {
            (Some(dir), Some(pattern)) => Some(format!("search `{pattern}` in `{dir}`")),
            (None, Some(pattern)) => Some(format!("search `{pattern}`")),
            _ => None,
        },
        "copy_file" => match (field(args, "source_path"), field(args, "destination_path")) {
            (Some(from), Some(to)) => Some(format!("copy `{from}` -> `{to}`")),
            _ => None,
        },
        "move_file" => match (field(args, "source_path"), field(args, "destination_path")) {
            (Some(from), Some(to)) => Some(format!("move `{from}` -> `{to}`")),
            _ => None,
        },
        "create_dir" => field(args, "directory_path").map(|path| format!("create dir `{path}`")),
        "delete_file" => field(args, "path").map(|path| {
            if flag(args, "recursive") {
                format!("delete `{path}` (recursive)")
            } else {
                format!("delete `{path}`")
            }
        }),
        _ => None,
    };
    if let Some(summary) = summary {
        return summary;
    }
    const MAX_CHARS: usize = 200;
    let single_line = args.to_string();
    let single_line = single_line.split_whitespace().collect::<Vec<_>>().join(" ");
    let full = format!("call `{name}` with {single_line}");
    if full.chars().count() <= MAX_CHARS {
        return full;
    }
    let mut truncated: String = full.chars().take(MAX_CHARS).collect();
    truncated.push('…');
    truncated
}

// ---------------------------------------------------------------------------
// Root-scoped path resolution
// ---------------------------------------------------------------------------

/// Resolve a tool-supplied path to an absolute path inside `root`.
///
/// `user_path` may be relative (resolved against `root`) or absolute.
/// Rejects escapes via `..`, absolute paths outside `root`, and symlinks
/// inside `root` that point outside it (symlinks are resolved with
/// `canonicalize`, then contained with `strip_prefix`).
///
/// Nonexistent paths are supported (needed for file creation): the nearest
/// existing ancestor is canonicalized and the remainder is lexically
/// appended, then the containment check applies to the result.
///
/// Note: this is check-then-use (TOCTOU). It stops confused or malicious
/// model output from wandering off-root, but a hostile actor racing the
/// filesystem could still swap a symlink between check and use.
pub fn resolve_within_root(root: &Path, user_path: &str) -> anyhow::Result<PathBuf> {
    let root_canon = root
        .canonicalize()
        .map_err(|e| anyhow::anyhow!("project root '{}' is not accessible: {e}", root.display()))?;
    let joined = {
        let candidate = Path::new(user_path);
        if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            root_canon.join(candidate)
        }
    };
    let candidate = lexical_normalize(&joined);

    // Walk up to the nearest existing ancestor, remembering the tail.
    let mut ancestor = candidate.as_path();
    let mut tail: Vec<&std::ffi::OsStr> = Vec::new();
    loop {
        match std::fs::symlink_metadata(ancestor) {
            Ok(_) => break,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => match ancestor.file_name() {
                Some(name) => {
                    tail.push(name);
                    ancestor = ancestor.parent().ok_or_else(|| {
                        anyhow::anyhow!("path '{user_path}' escapes the project root")
                    })?;
                }
                None => {
                    anyhow::bail!("path '{user_path}' escapes the project root");
                }
            },
            Err(e) => anyhow::bail!("cannot access path '{user_path}': {e}"),
        }
    }

    let mut resolved = ancestor
        .canonicalize()
        .map_err(|e| anyhow::anyhow!("cannot resolve path '{user_path}': {e}"))?;
    for part in tail.iter().rev() {
        resolved.push(part);
    }
    resolved
        .strip_prefix(&root_canon)
        .map_err(|_| anyhow::anyhow!("path '{user_path}' escapes the project root"))?;
    Ok(resolved)
}

/// Collapse `.` and `..` lexically (no filesystem access).
fn lexical_normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::RootDir | Component::Prefix(_) | Component::Normal(_) => {
                out.push(component.as_os_str());
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Child-process runner (argv-only, no shell)
// ---------------------------------------------------------------------------

/// Maximum captured child output (stdout + stderr) per invocation.
pub const MAX_COMMAND_OUTPUT: usize = 32 * 1024;

/// Default wall-clock budget for [`ShellTool`] (and [`GitTool`]) child
/// processes. A child still running past this deadline is killed and the
/// call fails with an error naming the timeout.
pub const SHELL_TIMEOUT_SECS: u64 = 120;

struct CommandOutput {
    success: bool,
    exit_code: Option<i32>,
    output: String,
    truncated: bool,
}

/// Run `program` with `args` in `workdir`, capturing combined output.
///
/// Enforcement: argv-only (no shell string), a scrubbed environment (see
/// [`scrubbed_env`]), no inherited stdin, and a [`SHELL_TIMEOUT_SECS`]
/// kill deadline.
///
/// `tokio::process` is intentionally not used (its cargo feature is off);
/// the blocking `std::process` call runs on `spawn_blocking` instead.
async fn run_command(
    program: String,
    args: Vec<String>,
    workdir: PathBuf,
    tool: &'static str,
) -> Result<CommandOutput, ToolCallError> {
    run_command_with_timeout(
        program,
        args,
        workdir,
        tool,
        Duration::from_secs(SHELL_TIMEOUT_SECS),
    )
    .await
}

/// [`run_command`] with an explicit kill deadline (tests use a short budget).
async fn run_command_with_timeout(
    program: String,
    args: Vec<String>,
    workdir: PathBuf,
    tool: &'static str,
    timeout: Duration,
) -> Result<CommandOutput, ToolCallError> {
    tokio::task::spawn_blocking(move || {
        run_command_blocking(&program, &args, &workdir, tool, timeout)
    })
    .await
    .map_err(|e| tool_error(format!("{tool}: execution task failed: {e}")))?
}

/// Blocking child-process driver: spawn, poll with a kill deadline, combine
/// stdout/stderr, truncate. Both pipes are drained on helper threads so a
/// chatty child never wedges on a full pipe buffer while we poll `try_wait`.
fn run_command_blocking(
    program: &str,
    args: &[String],
    workdir: &Path,
    tool: &str,
    timeout: Duration,
) -> Result<CommandOutput, ToolCallError> {
    let mut child = std::process::Command::new(program)
        .args(args)
        .current_dir(workdir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env_clear()
        .envs(scrubbed_env())
        .spawn()
        .map_err(|e| tool_error(format!("{tool}: failed to start '{program}': {e}")))?;

    let drain = |mut pipe: std::process::ChildStdout| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = pipe.read_to_end(&mut buf);
            buf
        })
    };
    // `ChildStdout` and `ChildStderr` are distinct types; read stderr through
    // the same shape via a tiny adapter thread.
    let stdout_thread = child.stdout.take().map(drain);
    let stderr_thread = child.stderr.take().map(|mut pipe| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = pipe.read_to_end(&mut buf);
            buf
        })
    });

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child
            .try_wait()
            .map_err(|e| tool_error(format!("{tool}: failed to wait for '{program}': {e}")))?
        {
            Some(status) => break status,
            None => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    // Detach the drain threads instead of joining: a killed
                    // child whose pipes are held open by orphaned
                    // grandchildren would wedge the join. The error must
                    // return promptly; the threads exit when the pipes close.
                    drop(stdout_thread);
                    drop(stderr_thread);
                    return Err(tool_error(format!(
                        "{tool}: command timed out after {}s and was killed",
                        timeout.as_secs()
                    )));
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    };

    let stdout = stdout_thread
        .and_then(|thread| thread.join().ok())
        .unwrap_or_default();
    let stderr = stderr_thread
        .and_then(|thread| thread.join().ok())
        .unwrap_or_default();
    Ok(combine_output(&status, &stdout, &stderr))
}

/// Merge captured stdout/stderr into one truncated [`CommandOutput`].
fn combine_output(
    status: &std::process::ExitStatus,
    stdout: &[u8],
    stderr: &[u8],
) -> CommandOutput {
    let mut combined = String::from_utf8_lossy(stdout).into_owned();
    let stderr = String::from_utf8_lossy(stderr);
    if !stderr.is_empty() {
        if !combined.is_empty() && !combined.ends_with('\n') {
            combined.push('\n');
        }
        combined.push_str(&stderr);
    }
    let (text, truncated) = truncate_output(&combined);
    CommandOutput {
        success: status.success(),
        exit_code: status.code(),
        output: text,
        truncated,
    }
}

/// Build the scrubbed environment inherited by spawned child processes.
///
/// The parent environment is *not* inherited wholesale (`env_clear` + this
/// allowlist). Names are compared case-insensitively so Windows' `Path`
/// matches `PATH`. Exact allowlist:
///
/// - All platforms: `PATH`, `TMPDIR`, `TEMP`, `TMP`.
/// - Unix: `HOME`, `USER`, `LOGNAME`.
/// - Windows: `SYSTEMROOT`, `SYSTEMDRIVE`, `USERNAME`, `USERPROFILE`,
///   `HOMEDRIVE`, `HOMEPATH`.
///
/// Any variable whose *name* contains `KEY`, `TOKEN`, or `SECRET`
/// (case-insensitive) is **never** inherited, even if it were allowlisted —
/// the blocklist is checked first (see [`child_env_allowed`]). Values are
/// never inspected.
fn scrubbed_env() -> Vec<(std::ffi::OsString, std::ffi::OsString)> {
    std::env::vars_os()
        .filter(|(name, _)| child_env_allowed(&name.to_string_lossy()))
        .collect()
}

/// Decide whether one environment variable `name` may pass to a child.
///
/// Blocklist first: names containing `KEY`, `TOKEN`, or `SECRET`
/// (case-insensitive) are always rejected. Otherwise the name must be on the
/// [`scrubbed_env`] allowlist (compared case-insensitively).
fn child_env_allowed(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    if upper.contains("KEY") || upper.contains("TOKEN") || upper.contains("SECRET") {
        return false;
    }
    const COMMON: &[&str] = &["PATH", "TMPDIR", "TEMP", "TMP"];
    if COMMON.contains(&upper.as_str()) {
        return true;
    }
    #[cfg(windows)]
    {
        const WINDOWS: &[&str] = &[
            "SYSTEMROOT",
            "SYSTEMDRIVE",
            "USERNAME",
            "USERPROFILE",
            "HOMEDRIVE",
            "HOMEPATH",
        ];
        WINDOWS.contains(&upper.as_str())
    }
    #[cfg(not(windows))]
    {
        const UNIX: &[&str] = &["HOME", "USER", "LOGNAME"];
        UNIX.contains(&upper.as_str())
    }
}

fn truncate_output(text: &str) -> (String, bool) {
    if text.len() <= MAX_COMMAND_OUTPUT {
        return (text.to_string(), false);
    }
    let mut end = MAX_COMMAND_OUTPUT;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (format!("{}...\n[output truncated]", &text[..end]), true)
}

// ---------------------------------------------------------------------------
// Shell tool
// ---------------------------------------------------------------------------

/// Arguments for [`ShellTool`].
#[derive(Debug, Clone, Serialize, Deserialize, ToolInput)]
pub struct ShellArgs {
    /// Program to execute (resolved via `PATH`; never interpreted by a shell).
    #[input(description = "Program to execute (resolved via PATH; no shell interpretation)")]
    pub command: String,
    /// Arguments passed verbatim to the program.
    #[serde(default)]
    #[input(description = "Arguments passed verbatim to the program")]
    pub args: Vec<String>,
    /// Working directory, relative to the project root; empty means the root.
    #[serde(default)]
    #[input(description = "Working directory relative to the project root; empty means the root")]
    pub cwd: String,
}

/// Runs a program with argv (no shell string) and `cwd` pinned inside root.
///
/// Always requires approval ([`RiskLevel::Execute`]). Combined stdout/stderr
/// is captured up to [`MAX_COMMAND_OUTPUT`] bytes. The child runs with a
/// scrubbed environment (see [`scrubbed_env`]) and is killed past the
/// execution timeout ([`SHELL_TIMEOUT_SECS`] by default).
#[tool(
    name = "shell",
    description = "Run a program with explicit argv and a project-root working directory (no shell). Always requires approval.",
    input = ShellArgs,
)]
pub struct ShellTool {
    root: PathBuf,
    approvals: Arc<dyn ApprovalHook>,
    timeout: Duration,
}

impl ShellTool {
    /// Create a shell tool rooted at `root` with the default execution timeout.
    pub fn new(root: PathBuf, approvals: Arc<dyn ApprovalHook>) -> Self {
        Self {
            root,
            approvals,
            timeout: Duration::from_secs(SHELL_TIMEOUT_SECS),
        }
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
        let ShellArgs { command, args, cwd } = serde_json::from_value(args)?;
        if command.trim().is_empty() {
            return Err(tool_error("shell: 'command' must not be empty".to_string()));
        }
        let dir = if cwd.trim().is_empty() {
            "."
        } else {
            cwd.as_str()
        };
        let workdir =
            resolve_within_root(&self.root, dir).map_err(|e| tool_error(format!("shell: {e}")))?;
        if !workdir.is_dir() {
            return Err(tool_error(format!(
                "shell: working directory '{}' is not a directory",
                workdir.display()
            )));
        }
        let action = ToolAction {
            tool: "shell".to_string(),
            summary: format!(
                "run `{command} {}` in {}",
                args.join(" "),
                workdir.display()
            ),
            risk: RiskLevel::Execute,
        };
        check_approval_permitted(self.approvals.as_ref(), &action)?;

        let out = run_command_with_timeout(command, args, workdir, "shell", self.timeout).await?;
        Ok(json!({
            "success": out.success,
            "exit_code": out.exit_code,
            "output": out.output,
            "truncated": out.truncated,
        }))
    }
}

// ---------------------------------------------------------------------------
// Apply-patch tool
// ---------------------------------------------------------------------------

/// Arguments for [`ApplyPatchTool`].
#[derive(Debug, Clone, Serialize, Deserialize, ToolInput)]
pub struct ApplyPatchArgs {
    /// Unified diff to apply; `a/`/`b/` prefixes accepted.
    #[input(description = "Unified diff to apply (git-style a/ b/ prefixes accepted)")]
    pub patch: String,
}

/// Applies a unified diff to files inside the project root.
///
/// Pure-Rust applier (no `patch` binary needed). Every hunk target is
/// resolved with [`resolve_within_root`] and out-of-root hunks abort the
/// whole patch before anything is written. Requires approval
/// ([`RiskLevel::Write`]).
///
/// Limitations: exact-match hunks only (no fuzz); UTF-8 text files;
/// `\r\n` files are rewritten with `\n` endings.
#[tool(
    name = "apply_patch",
    description = "Apply a unified diff to files inside the project root. Requires approval.",
    input = ApplyPatchArgs,
)]
pub struct ApplyPatchTool {
    root: PathBuf,
    approvals: Arc<dyn ApprovalHook>,
}

impl ApplyPatchTool {
    /// Create an apply-patch tool rooted at `root`.
    pub fn new(root: PathBuf, approvals: Arc<dyn ApprovalHook>) -> Self {
        Self { root, approvals }
    }

    /// The project root this tool is scoped to.
    pub fn root(&self) -> &Path {
        &self.root
    }
}

#[async_trait]
impl ToolRuntime for ApplyPatchTool {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let ApplyPatchArgs { patch } = serde_json::from_value(args)?;
        // Plan first (validates every path and hunk), approve, then commit.
        let plan =
            plan_patch(&self.root, &patch).map_err(|e| tool_error(format!("apply_patch: {e}")))?;
        let summary = if plan.is_empty() {
            "apply empty patch (no changes)".to_string()
        } else {
            let files: Vec<String> = plan
                .iter()
                .map(|f| format!("{} ({})", f.relative, f.op))
                .collect();
            format!("apply patch to {}: {}", plan.len(), files.join(", "))
        };
        check_approval_permitted(
            self.approvals.as_ref(),
            &ToolAction {
                tool: "apply_patch".to_string(),
                summary,
                risk: RiskLevel::Write,
            },
        )?;
        let mut changed = Vec::with_capacity(plan.len());
        for file in &plan {
            commit_planned_file(file).map_err(|e| tool_error(format!("apply_patch: {e}")))?;
            changed.push(file.relative.clone());
        }
        Ok(json!({ "success": true, "files": changed }))
    }
}

/// Apply `patch` to `root` without an approval check.
///
/// Used by [`ApplyPatchTool`] after approval; also handy for tests and for
/// callers that gate approval at dispatch time. Returns the affected
/// root-relative paths. Two-phase: the whole patch is parsed and validated
/// before any file is written.
pub fn apply_unified_diff(root: &Path, patch: &str) -> anyhow::Result<Vec<String>> {
    let plan = plan_patch(root, patch)?;
    let mut changed = Vec::with_capacity(plan.len());
    for file in &plan {
        commit_planned_file(file)?;
        changed.push(file.relative.clone());
    }
    Ok(changed)
}

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
}

impl GitTool {
    /// Create a git tool rooted at `root`.
    pub fn new(root: PathBuf, approvals: Arc<dyn ApprovalHook>) -> Self {
        Self { root, approvals }
    }

    /// The project root this tool is scoped to.
    pub fn root(&self) -> &Path {
        &self.root
    }
}

#[async_trait]
impl ToolRuntime for GitTool {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let GitArgs { args } = serde_json::from_value(args)?;
        let Some(subcommand) = args.first().map(String::as_str) else {
            return Err(tool_error("git: no subcommand given".to_string()));
        };
        let root =
            resolve_within_root(&self.root, ".").map_err(|e| tool_error(format!("git: {e}")))?;
        match subcommand {
            "status" | "diff" | "log" | "show" => {}
            "add" | "commit" | "checkout" | "branch" => {
                check_approval_permitted(
                    self.approvals.as_ref(),
                    &ToolAction {
                        tool: "git".to_string(),
                        summary: format!("run `git {}`", args.join(" ")),
                        risk: RiskLevel::Write,
                    },
                )?;
            }
            other => {
                return Err(tool_error(format!(
                    "git: subcommand '{other}' is denied by the allowlist \
                     (allowed: status, diff, log, show, add, commit, checkout, branch)"
                )));
            }
        }

        let mut full_args = vec!["-C".to_string(), root.to_string_lossy().into_owned()];
        full_args.extend(args);
        let out = run_command("git".to_string(), full_args, root, "git").await?;
        Ok(json!({
            "success": out.success,
            "exit_code": out.exit_code,
            "output": out.output,
            "truncated": out.truncated,
        }))
    }
}

// ---------------------------------------------------------------------------
// Composition entry points
// ---------------------------------------------------------------------------

/// Build the full coding toolset for `root` as boxed SDK tools.
///
/// Returns, in order: `list_dir`, `read_file`, `write_file`, `copy_file`,
/// `move_file`, `delete_file`, `create_dir`, `search_file` (all
/// `autoagents-toolkit` filesystem tools constructed with
/// `new_with_root_dir`), plus the custom `shell`, `apply_patch`, and `git`
/// tools wired to `approvals`.
///
/// Every toolkit tool is wrapped in [`GatedTool`] so its calls consult
/// `approvals` first — ungated, their `execute` would bypass approvals
/// entirely. Gate risks: `list_dir`/`read_file`/`search_file` gate as
/// [`RiskLevel::Read`]; `write_file`/`copy_file`/`move_file`/`create_dir` as
/// [`RiskLevel::Write`]; `delete_file` as [`RiskLevel::Destructive`]
/// (regardless of its `recursive` flag). The custom tools gate internally
/// and are not double-wrapped.
///
/// `root` must exist; it is canonicalized once here so every tool shares
/// one scope.
///
/// # Dynamic wiring (recommended)
///
/// The result plugs directly into a ReAct run — no `#[agent]` macro needed:
///
/// ```ignore
/// use autoagents::core::agent::Context;
/// use themis_core::tools::{AllowAllHook, boxed_tools};
/// use std::sync::Arc;
///
/// let tools = boxed_tools(project_root, Arc::new(AllowAllHook))?;
/// let context = Context::new(llm, None).with_tools(tools);
/// // ... run ReActAgent with `context`
/// ```
///
/// # Macro-style wiring
///
/// `#[agent(tools = [..])]` expands bare paths to struct literals (`Tool {}`),
/// so root/hook configuration must come from an expression evaluated inside
/// the generated `tools(&self)`. An agent struct that carries the root and
/// hook can expose builders, e.g. `tools = [self.make_shell(), ...]`, or —
/// simpler — skip the macro and use `Context::with_tools` with
/// [`boxed_tools`] as above.
pub fn boxed_tools(
    root: &Path,
    approvals: Arc<dyn ApprovalHook>,
) -> anyhow::Result<Vec<Box<dyn ToolT>>> {
    let canonical = root
        .canonicalize()
        .map_err(|e| anyhow::anyhow!("project root '{}' is not accessible: {e}", root.display()))?;
    let root_string = canonical.to_string_lossy().into_owned();
    let gate = |tool: Box<dyn ToolT>, risk: RiskLevel| -> Box<dyn ToolT> {
        Box::new(GatedTool::new(tool, Arc::clone(&approvals), risk))
    };
    let mut tools: Vec<Box<dyn ToolT>> = vec![
        gate(
            Box::new(ListDir::new_with_root_dir(root_string.clone())),
            RiskLevel::Read,
        ),
        gate(
            Box::new(ReadFile::new_with_root_dir(root_string.clone())),
            RiskLevel::Read,
        ),
        gate(
            Box::new(WriteFile::new_with_root_dir(root_string.clone())),
            RiskLevel::Write,
        ),
        gate(
            Box::new(CopyFile::new_with_root_dir(root_string.clone())),
            RiskLevel::Write,
        ),
        gate(
            Box::new(MoveFile::new_with_root_dir(root_string.clone())),
            RiskLevel::Write,
        ),
        gate(
            Box::new(DeleteFile::new_with_root_dir(root_string.clone())),
            RiskLevel::Destructive,
        ),
        gate(
            Box::new(CreateDir::new_with_root_dir(root_string.clone())),
            RiskLevel::Write,
        ),
        gate(
            Box::new(SearchFile::new_with_root_dir(root_string.clone())),
            RiskLevel::Read,
        ),
    ];
    tools.push(Box::new(ShellTool::new(
        canonical.clone(),
        Arc::clone(&approvals),
    )));
    tools.push(Box::new(ApplyPatchTool::new(
        canonical.clone(),
        Arc::clone(&approvals),
    )));
    tools.push(Box::new(GitTool::new(canonical, approvals)));
    Ok(tools)
}

/// Build the full toolset as shared (`Arc`) handles.
///
/// Convert to boxes with the SDK's `shared_tools_to_boxes` when an API wants
/// `Vec<Box<dyn ToolT>>` but the handles must stay shared across agents.
pub fn shared_tools(
    root: &Path,
    approvals: Arc<dyn ApprovalHook>,
) -> anyhow::Result<Vec<Arc<dyn ToolT>>> {
    Ok(boxed_tools(root, approvals)?
        .into_iter()
        .map(Arc::from)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn allow_all() -> Arc<dyn ApprovalHook> {
        Arc::new(AllowAllHook)
    }

    fn deny_all() -> Arc<dyn ApprovalHook> {
        Arc::new(DenyAllHook)
    }

    fn rooted_case() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempdir().unwrap();
        let root = tmp.path().join("root");
        fs::create_dir(&root).unwrap();
        (tmp, root)
    }

    /// Hook that records every consulted action and answers with `decision`.
    struct RecordingHook {
        decision: Approval,
        seen: std::sync::Mutex<Vec<ToolAction>>,
    }

    impl RecordingHook {
        fn new(decision: Approval) -> Self {
            Self {
                decision,
                seen: std::sync::Mutex::new(Vec::new()),
            }
        }

        fn seen(&self) -> Vec<ToolAction> {
            self.seen.lock().unwrap().clone()
        }
    }

    impl ApprovalHook for RecordingHook {
        fn approve(&self, action: &ToolAction) -> Approval {
            self.seen.lock().unwrap().push(action.clone());
            self.decision
        }
    }

    fn find_tool<'a>(tools: &'a [Box<dyn ToolT>], name: &str) -> &'a dyn ToolT {
        tools
            .iter()
            .find(|tool| tool.name() == name)
            .map(|tool| &**tool)
            .unwrap()
    }

    #[tokio::test]
    async fn gated_write_denied_under_deny_all() {
        let (_tmp, root) = rooted_case();
        let tools = boxed_tools(&root, deny_all()).unwrap();
        let err = find_tool(&tools, "write_file")
            .execute(json!({
                "file_path": "note.txt",
                "content": "hello",
                "append": false,
            }))
            .await
            .unwrap_err();
        let message = err.to_string().to_lowercase();
        assert!(message.contains("approv"), "{message}");
        assert!(!root.join("note.txt").exists());
    }

    #[tokio::test]
    async fn gated_write_works_under_allow_all() {
        let (_tmp, root) = rooted_case();
        let tools = boxed_tools(&root, allow_all()).unwrap();
        let result = find_tool(&tools, "write_file")
            .execute(json!({
                "file_path": "sub/note.txt",
                "content": "hello",
                "append": false,
            }))
            .await
            .unwrap();
        assert_eq!(result["success"].as_bool(), Some(true));
        assert_eq!(
            fs::read_to_string(root.join("sub/note.txt")).unwrap(),
            "hello"
        );
    }

    #[tokio::test]
    async fn gated_read_consults_hook_with_read_risk() {
        // Core asserts the wrapper *consulted* the hook with Read risk; the
        // auto-allow-reads policy itself lives in the desktop hook, so here a
        // recording hook answers AllowAlways and the read succeeds.
        let (_tmp, root) = rooted_case();
        fs::write(root.join("r.txt"), "data").unwrap();
        let hook = Arc::new(RecordingHook::new(Approval::AllowAlways));
        let tools = boxed_tools(&root, hook.clone()).unwrap();
        let result = find_tool(&tools, "read_file")
            .execute(json!({ "file_path": "r.txt" }))
            .await
            .unwrap();
        assert_eq!(result["success"].as_bool(), Some(true));
        assert_eq!(result["content"].as_str(), Some("data"));
        let seen = hook.seen();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].tool, "read_file");
        assert_eq!(seen[0].risk, RiskLevel::Read);
        assert!(!seen[0].summary.is_empty());
    }

    #[tokio::test]
    async fn gated_delete_denied_under_deny_all() {
        let (_tmp, root) = rooted_case();
        let hook = Arc::new(RecordingHook::new(Approval::Deny));
        let tools = boxed_tools(&root, hook.clone()).unwrap();
        let delete = find_tool(&tools, "delete_file");

        // Plain file delete.
        fs::write(root.join("victim.txt"), "x").unwrap();
        let err = delete
            .execute(json!({ "path": "victim.txt", "recursive": false }))
            .await
            .unwrap_err();
        assert!(err.to_string().to_lowercase().contains("approv"), "{err}");
        assert!(root.join("victim.txt").exists());

        // Recursive directory delete flows through the same Destructive gate.
        fs::create_dir(root.join("dir")).unwrap();
        fs::write(root.join("dir/inner.txt"), "x").unwrap();
        let err = delete
            .execute(json!({ "path": "dir", "recursive": true }))
            .await
            .unwrap_err();
        assert!(err.to_string().to_lowercase().contains("approv"), "{err}");
        assert!(root.join("dir/inner.txt").exists());

        let seen = hook.seen();
        assert_eq!(seen.len(), 2);
        for action in &seen {
            assert_eq!(action.tool, "delete_file");
            assert_eq!(action.risk, RiskLevel::Destructive);
        }
    }

    #[test]
    fn gated_tool_delegates_metadata() {
        let (_tmp, root) = rooted_case();
        let tools = boxed_tools(&root, allow_all()).unwrap();
        let gated = find_tool(&tools, "write_file");
        let inner = WriteFile::new_with_root_dir(root.to_string_lossy().into_owned());
        assert_eq!(gated.name(), inner.name());
        assert_eq!(gated.description(), inner.description());
        assert_eq!(gated.args_schema(), inner.args_schema());
        assert_eq!(gated.output_schema(), inner.output_schema());
        let debug = format!("{gated:?}");
        assert!(debug.contains("GatedTool"), "{debug}");
    }

    #[test]
    fn shell_default_timeout_is_120s() {
        let (_tmp, root) = rooted_case();
        assert_eq!(SHELL_TIMEOUT_SECS, 120);
        let tool = ShellTool::new(root, allow_all());
        assert_eq!(tool.timeout(), Duration::from_secs(120));
        let tool = tool.with_timeout(Duration::from_secs(1));
        assert_eq!(tool.timeout(), Duration::from_secs(1));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn shell_timeout_kills_long_running_command() {
        let (_tmp, root) = rooted_case();
        let tool = ShellTool::new(root, allow_all()).with_timeout(Duration::from_secs(1));
        let start = Instant::now();
        let err = tool
            .execute(json!({ "command": "sleep", "args": ["300"] }))
            .await
            .unwrap_err();
        let message = err.to_string().to_lowercase();
        assert!(message.contains("timed out"), "{message}");
        assert!(message.contains("1s"), "{message}");
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "kill took too long: {:?}",
            start.elapsed()
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn shell_env_is_scrubbed() {
        let (_tmp, root) = rooted_case();
        std::env::set_var("THEMIS_SECRET_PROBE", "should-not-appear-98765");
        let result = ShellTool::new(root, allow_all())
            .execute(json!({ "command": "env", "args": [] }))
            .await
            .unwrap();
        std::env::remove_var("THEMIS_SECRET_PROBE");
        let output = result["output"].as_str().unwrap();
        assert!(
            !output.contains("THEMIS_SECRET_PROBE"),
            "sentinel leaked: {output}"
        );
        assert!(
            !output.contains("should-not-appear-98765"),
            "sentinel value leaked: {output}"
        );
        assert!(output.contains("PATH="), "PATH missing: {output}");
    }

    #[test]
    fn child_env_blocklist_wins_over_allowlist() {
        for secret in [
            "API_KEY",
            "api_key",
            "Github_Token",
            "MY_SECRET",
            "THEMIS_SECRET_PROBE",
            "SECRET_PATH",
        ] {
            assert!(!child_env_allowed(secret), "{secret} must be blocked");
        }
        for allowed in ["PATH", "Path", "TMPDIR", "TEMP", "TMP"] {
            assert!(child_env_allowed(allowed), "{allowed} must pass");
        }
        #[cfg(not(windows))]
        for allowed in ["HOME", "USER", "LOGNAME"] {
            assert!(child_env_allowed(allowed), "{allowed} must pass");
        }
        #[cfg(windows)]
        for allowed in [
            "SYSTEMROOT",
            "SYSTEMDRIVE",
            "USERNAME",
            "USERPROFILE",
            "HOMEDRIVE",
            "HOMEPATH",
        ] {
            assert!(child_env_allowed(allowed), "{allowed} must pass");
        }
        assert!(!child_env_allowed("RANDOM_UNRELATED_VAR"));
        assert!(!child_env_allowed("SSH_AUTH_SOCK"));
    }

    #[test]
    fn management_approvals_separate_read_mutation_and_changed_execution() {
        assert_eq!(
            integration_risk("save_skill", &json!({"action":"list"})),
            Some(RiskLevel::Write)
        );
        assert_eq!(
            integration_risk("manage_integrations", &json!({"action":"list"})),
            Some(RiskLevel::Read)
        );
        // Cached catalog requests can still fetch when the source cache is absent.
        assert_eq!(
            integration_risk(
                "manage_integrations",
                &json!({"action":"catalog","refresh":false})
            ),
            Some(RiskLevel::Network)
        );
        // Preview does not install, but uncached sources can fetch repositories.
        assert_eq!(
            integration_risk(
                "manage_integrations",
                &json!({"action":"preview","refresh":false})
            ),
            Some(RiskLevel::Network)
        );
        assert_eq!(
            integration_risk(
                "manage_integrations",
                &json!({"action":"preview_repository"})
            ),
            Some(RiskLevel::Network)
        );
        assert_eq!(
            integration_risk("manage_integrations", &json!({"action":"delete"})),
            Some(RiskLevel::Destructive)
        );
        assert_eq!(
            integration_risk("manage_integrations", &json!({"action":"remove_component"})),
            Some(RiskLevel::Destructive)
        );
        assert_eq!(
            integration_risk("manage_integrations", &json!({"action":"test_hook"})),
            Some(RiskLevel::Execute)
        );
        assert_ne!(
            integration_approval_identity("manage_integrations", &json!({"action":"list"})),
            integration_approval_identity("manage_integrations", &json!({"action":"save"}))
        );
        assert_ne!(
            integration_approval_identity(
                "manage_integrations",
                &json!({"action":"test_hook","hook":{"command":"true"}})
            ),
            integration_approval_identity(
                "manage_integrations",
                &json!({"action":"test_hook","hook":{"command":":"}})
            )
        );
    }

    #[test]
    fn approval_hooks_behave() {
        let shell = ToolAction {
            tool: "shell".to_string(),
            summary: "run echo".to_string(),
            risk: RiskLevel::Execute,
        };
        assert_eq!(AllowAllHook.approve(&shell), Approval::AllowAlways);
        assert_eq!(DenyAllHook.approve(&shell), Approval::Deny);
        let hook = FnHook::new(|action: &ToolAction| {
            if action.tool == "git" {
                Approval::AllowOnce
            } else {
                Approval::Deny
            }
        });
        assert_eq!(hook.approve(&shell), Approval::Deny);
        let git = ToolAction {
            tool: "git".to_string(),
            summary: "status".to_string(),
            risk: RiskLevel::Read,
        };
        assert_eq!(hook.approve(&git), Approval::AllowOnce);
    }

    #[test]
    fn traversal_is_rejected() {
        let (tmp, root) = rooted_case();
        fs::write(tmp.path().join("outside.txt"), "secret").unwrap();
        let err = resolve_within_root(&root, "../outside.txt").unwrap_err();
        assert!(err.to_string().contains("escapes"), "{err}");
    }

    #[tokio::test]
    async fn toolkit_read_rejects_traversal() {
        let (tmp, root) = rooted_case();
        fs::write(tmp.path().join("outside.txt"), "secret").unwrap();
        let tool = ReadFile::new_with_root_dir(root.to_string_lossy().into_owned());
        let err = tool
            .execute(json!({ "file_path": "../outside.txt" }))
            .await
            .unwrap_err();
        assert!(!err.to_string().is_empty());
    }

    #[test]
    fn absolute_escape_is_rejected() {
        let (tmp, root) = rooted_case();
        let outside = tmp.path().join("outside.txt");
        fs::write(&outside, "secret").unwrap();
        let err = resolve_within_root(&root, outside.to_str().unwrap()).unwrap_err();
        assert!(err.to_string().contains("escapes"), "{err}");
    }

    #[test]
    fn absolute_path_inside_root_is_allowed() {
        let (_tmp, root) = rooted_case();
        let inside = root.join("a.txt");
        fs::write(&inside, "x").unwrap();
        let resolved = resolve_within_root(&root, inside.to_str().unwrap()).unwrap();
        assert!(resolved.ends_with("a.txt"), "{}", resolved.display());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_escape_is_rejected() {
        let (tmp, root) = rooted_case();
        let outside = tmp.path().join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("secret.txt"), "secret").unwrap();
        std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();
        let err = resolve_within_root(&root, "link/secret.txt").unwrap_err();
        assert!(err.to_string().contains("escapes"), "{err}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn toolkit_read_rejects_symlink_escape() {
        let (tmp, root) = rooted_case();
        let outside = tmp.path().join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("secret.txt"), "secret").unwrap();
        std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();
        let tool = ReadFile::new_with_root_dir(root.to_string_lossy().into_owned());
        let err = tool
            .execute(json!({ "file_path": "link/secret.txt" }))
            .await
            .unwrap_err();
        assert!(!err.to_string().is_empty());
    }

    #[tokio::test]
    async fn write_file_inside_root_works() {
        let (_tmp, root) = rooted_case();
        let tool = WriteFile::new_with_root_dir(root.to_string_lossy().into_owned());
        let result = tool
            .execute(json!({
                "file_path": "sub/note.txt",
                "content": "hello",
                "append": false,
            }))
            .await
            .unwrap();
        assert_eq!(result["success"].as_bool(), Some(true));
        assert_eq!(
            fs::read_to_string(root.join("sub/note.txt")).unwrap(),
            "hello"
        );
    }

    #[tokio::test]
    async fn shell_denied_under_deny_all() {
        let (_tmp, root) = rooted_case();
        let tool = ShellTool::new(root, deny_all());
        let err = tool
            .execute(json!({ "command": "echo", "args": ["hi"] }))
            .await
            .unwrap_err();
        let message = err.to_string().to_lowercase();
        assert!(message.contains("approv"), "{message}");
    }

    #[tokio::test]
    async fn shell_echo_works_under_allow_all() {
        let (_tmp, root) = rooted_case();
        let tool = ShellTool::new(root, allow_all());
        let result = tool
            .execute(json!({ "command": "echo", "args": ["hello-tools"] }))
            .await
            .unwrap();
        assert_eq!(result["success"].as_bool(), Some(true));
        assert!(result["output"].as_str().unwrap().contains("hello-tools"));
    }

    #[tokio::test]
    async fn shell_rejects_cwd_outside_root() {
        let (tmp, root) = rooted_case();
        let tool = ShellTool::new(root.clone(), allow_all());
        let err = tool
            .execute(json!({ "command": "echo", "args": ["hi"], "cwd": ".." }))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("escapes"), "{err}");
        let err = tool
            .execute(json!({
                "command": "echo",
                "args": ["hi"],
                "cwd": tmp.path().to_str().unwrap(),
            }))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("escapes"), "{err}");

        // Positive control: a cwd inside root works.
        fs::create_dir(root.join("sub")).unwrap();
        let result = tool
            .execute(json!({ "command": "echo", "args": ["hi"], "cwd": "sub" }))
            .await
            .unwrap();
        assert_eq!(result["success"].as_bool(), Some(true));
    }

    #[tokio::test]
    async fn apply_patch_happy_path() {
        let (_tmp, root) = rooted_case();
        fs::write(root.join("hello.txt"), "line1\nline2\n").unwrap();
        let tool = ApplyPatchTool::new(root.clone(), allow_all());

        let modify = "--- a/hello.txt\n+++ b/hello.txt\n@@ -1,2 +1,3 @@\n line1\n line2\n+line3\n";
        let result = tool.execute(json!({ "patch": modify })).await.unwrap();
        assert_eq!(result["success"].as_bool(), Some(true));
        assert_eq!(
            fs::read_to_string(root.join("hello.txt")).unwrap(),
            "line1\nline2\nline3\n"
        );

        let create = "--- /dev/null\n+++ b/new.txt\n@@ -0,0 +1,2 @@\n+one\n+two\n";
        tool.execute(json!({ "patch": create })).await.unwrap();
        assert_eq!(
            fs::read_to_string(root.join("new.txt")).unwrap(),
            "one\ntwo\n"
        );
    }

    #[test]
    fn apply_patch_rejects_unprefixed_empty_hunk_lines() {
        let error = match parse_hunk(&["@@ -1 +1 @@", ""], 0) {
            Ok(_) => panic!("unprefixed empty hunk line was accepted"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("unexpected hunk line"));
    }

    #[tokio::test]
    async fn apply_patch_rejects_out_of_root_hunk() {
        let (tmp, root) = rooted_case();
        let tool = ApplyPatchTool::new(root, allow_all());
        let evil = "--- /dev/null\n+++ b/../../evil.txt\n@@ -0,0 +1 @@\n+pwned\n";
        let err = tool.execute(json!({ "patch": evil })).await.unwrap_err();
        assert!(err.to_string().contains("escapes"), "{err}");
        assert!(!tmp.path().join("evil.txt").exists());
        assert!(!tmp.path().join("root/evil.txt").exists());
    }

    #[tokio::test]
    async fn apply_patch_requires_approval() {
        let (_tmp, root) = rooted_case();
        fs::write(root.join("f.txt"), "a\n").unwrap();
        let tool = ApplyPatchTool::new(root, deny_all());
        let patch = "--- a/f.txt\n+++ b/f.txt\n@@ -1 +1 @@\n-a\n+b\n";
        let err = tool.execute(json!({ "patch": patch })).await.unwrap_err();
        let message = err.to_string().to_lowercase();
        assert!(message.contains("approv"), "{message}");
    }

    fn git_available() -> bool {
        std::process::Command::new("git")
            .arg("--version")
            .output()
            .map(|out| out.status.success())
            .unwrap_or(false)
    }

    #[tokio::test]
    async fn git_status_works_in_temp_repo() {
        if !git_available() {
            eprintln!("skipping git_status_works_in_temp_repo: git binary not found");
            return;
        }
        let (_tmp, root) = rooted_case();
        let inited = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(&root)
            .status()
            .unwrap();
        assert!(inited.success());
        let tool = GitTool::new(root, allow_all());
        let result = tool
            .execute(json!({ "args": ["status", "--short"] }))
            .await
            .unwrap();
        assert_eq!(result["success"].as_bool(), Some(true));
    }

    #[tokio::test]
    async fn git_allowlist_denies_unknown_subcommands() {
        let (_tmp, root) = rooted_case();
        let tool = GitTool::new(root, allow_all());
        let err = tool
            .execute(json!({ "args": ["push", "origin", "main"] }))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("denied"), "{err}");
    }

    #[tokio::test]
    async fn git_write_requires_approval() {
        // No git binary needed: the denial happens before any spawn.
        let (_tmp, root) = rooted_case();
        let tool = GitTool::new(root, deny_all());
        let err = tool
            .execute(json!({ "args": ["add", "."] }))
            .await
            .unwrap_err();
        let message = err.to_string().to_lowercase();
        assert!(message.contains("approv"), "{message}");
    }

    #[test]
    fn boxed_tools_returns_full_set() {
        let (_tmp, root) = rooted_case();
        let tools = boxed_tools(&root, allow_all()).unwrap();
        let names: Vec<&str> = tools.iter().map(|tool| tool.name()).collect();
        assert_eq!(
            names,
            vec![
                "list_dir",
                "read_file",
                "write_file",
                "copy_file",
                "move_file",
                "delete_file",
                "create_dir",
                "search_file",
                "shell",
                "apply_patch",
                "git",
            ]
        );
        for tool in &tools {
            assert!(!tool.description().is_empty());
            assert!(tool.args_schema().is_object());
        }
    }

    #[test]
    fn boxed_tools_rejects_missing_root() {
        let err =
            boxed_tools(Path::new("/definitely/not/here/themis-test"), allow_all()).unwrap_err();
        assert!(err.to_string().contains("not accessible"), "{err}");
    }

    #[test]
    fn shared_tools_convert_to_boxes() {
        let (_tmp, root) = rooted_case();
        let shared = shared_tools(&root, allow_all()).unwrap();
        assert_eq!(shared.len(), 11);
        let boxes = autoagents::core::tool::shared_tools_to_boxes(&shared);
        assert_eq!(boxes.len(), 11);
        assert_eq!(boxes[8].name(), "shell");
    }
}
