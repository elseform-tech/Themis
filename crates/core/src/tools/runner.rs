use super::*;

// ---------------------------------------------------------------------------
// Child-process runner (argv-only, no shell)
// ---------------------------------------------------------------------------

/// Maximum captured child output (stdout + stderr) per invocation.
pub const MAX_COMMAND_OUTPUT: usize = 32 * 1024;

/// Default wall-clock budget for [`ShellTool`] (and [`GitTool`]) child
/// processes. A child still running past this deadline is killed and the
/// call fails with an error naming the timeout.
pub const SHELL_TIMEOUT_SECS: u64 = 120;

pub(super) struct CommandOutput {
    pub(super) success: bool,
    pub(super) exit_code: Option<i32>,
    pub(super) output: String,
    pub(super) truncated: bool,
    /// The command left programs running after it exited and they were stopped.
    pub(super) leftovers_stopped: bool,
}

/// Run `program` with `args` in `workdir`, capturing combined output.
///
/// Enforcement: argv-only (no shell string), a scrubbed environment (see
/// [`scrubbed_env`]), no inherited stdin, and a [`SHELL_TIMEOUT_SECS`]
/// kill deadline.
///
/// `tokio::process` is intentionally not used (its cargo feature is off);
/// the blocking `std::process` call runs on `spawn_blocking` instead.
pub(super) async fn run_command(
    program: String,
    args: Vec<String>,
    workdir: PathBuf,
    tool: &'static str,
    cancel: Option<Arc<AtomicBool>>,
    sandbox: Option<Arc<SandboxPlan>>,
) -> Result<CommandOutput, ToolCallError> {
    run_command_with_timeout(
        program,
        args,
        workdir,
        tool,
        Duration::from_secs(SHELL_TIMEOUT_SECS),
        cancel,
        sandbox,
    )
    .await
}

/// Result of [`run_check_command`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckOutput {
    pub exit_code: Option<i32>,
    /// Combined stdout/stderr, bounded to [`MAX_COMMAND_OUTPUT`] bytes.
    pub output: String,
    pub truncated: bool,
}

/// Runs a watch check through the shell runner (argv only, scrubbed
/// environment, own process group, kill deadline, bounded output), inside
/// `sandbox` when given. Approval is decided by the caller; this performs no
/// policy check, and a sandbox denial is never escalated (nobody is asked).
pub async fn run_check_command(
    argv: Vec<String>,
    workdir: PathBuf,
    timeout: Duration,
    sandbox: Option<Arc<SandboxPlan>>,
) -> Result<CheckOutput, ToolCallError> {
    let mut parts = argv.into_iter();
    let program = parts
        .next()
        .filter(|program| !program.trim().is_empty())
        .ok_or_else(|| tool_error("check: command must not be empty".to_string()))?;
    // Scheduled checks belong to no run, so there is no Stop flag; the timeout still kills them.
    if let Some(refusal) = sandbox.as_ref().and_then(|plan| plan.refusal()) {
        return Err(tool_error(format!("check: {refusal}")));
    }
    let mut out = run_command_with_timeout(
        program,
        parts.collect(),
        workdir.clone(),
        "check",
        timeout,
        None,
        sandbox.clone(),
    )
    .await?;
    if let Some(plan) = &sandbox {
        if let Some(reason) = plan.denial(out.success, &out.output, &workdir) {
            out.output.push_str(&sandbox_note(&reason, false));
        }
    }
    Ok(CheckOutput {
        exit_code: out.exit_code,
        output: out.output,
        truncated: out.truncated,
    })
}

/// [`run_command`] with an explicit kill deadline (tests use a short budget).
/// `cancel` kills the whole process tree as soon as it is set (Stop).
pub(super) async fn run_command_with_timeout(
    program: String,
    args: Vec<String>,
    workdir: PathBuf,
    tool: &'static str,
    timeout: Duration,
    cancel: Option<Arc<AtomicBool>>,
    sandbox: Option<Arc<SandboxPlan>>,
) -> Result<CommandOutput, ToolCallError> {
    // Read here: the worker thread is outside the run's scope.
    let extras = ChildExtras::current();
    let agent = extras.agent_cli.clone();
    let mut out = tokio::task::spawn_blocking(move || {
        run_command_blocking(
            &program,
            &args,
            &workdir,
            tool,
            timeout,
            cancel.as_deref(),
            sandbox.as_deref(),
            &extras,
        )
    })
    .await
    .map_err(|e| tool_error(format!("{tool}: execution task failed: {e}")))??;
    if let Some(agent) = agent {
        out.output = agent.redact(&out.output);
    }
    if out.leftovers_stopped {
        // Metadata only: never the command, its arguments or its output.
        crate::diagnostics::emit(
            "tools",
            "shell_leftovers_stopped",
            "info",
            serde_json::json!({"tool": tool}),
        );
    }
    Ok(out)
}

/// How long to keep reading output after the direct child has exited. A
/// backgrounded grandchild (`cmd &`) inherits the pipes and would otherwise
/// hold them open for as long as it lives; after this it is stopped.
const PIPE_GRACE: Duration = Duration::from_millis(1500);
/// How long to wait for the pipes to close once leftovers were stopped.
const STOPPED_PIPE_GRACE: Duration = Duration::from_millis(500);
/// Per-stream capture bound while a command runs; excess is discarded.
const MAX_CAPTURE_BYTES: usize = 1024 * 1024;

/// One output pipe drained on its own thread into a shared, bounded buffer.
struct Drain {
    buffer: Arc<Mutex<Vec<u8>>>,
    thread: std::thread::JoinHandle<()>,
}

impl Drain {
    fn spawn<R: Read + Send + 'static>(mut pipe: R) -> Self {
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let shared = Arc::clone(&buffer);
        let thread = std::thread::spawn(move || {
            let mut chunk = [0u8; 8192];
            while let Ok(read) = pipe.read(&mut chunk) {
                if read == 0 {
                    break;
                }
                let mut buffer = shared.lock().unwrap_or_else(|e| e.into_inner());
                let room = MAX_CAPTURE_BYTES.saturating_sub(buffer.len());
                buffer.extend_from_slice(&chunk[..read.min(room)]);
            }
        });
        Self { buffer, thread }
    }

    fn take(&self) -> Vec<u8> {
        std::mem::take(&mut *self.buffer.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

/// Blocking child-process driver: spawn, poll with a kill deadline, combine
/// stdout/stderr, truncate. Both pipes are drained on helper threads so a
/// chatty child never wedges on a full pipe buffer while we poll `try_wait`.
/// After the child exits, see [`finish_after_exit`]: nothing it started
/// outlives the call.
#[allow(clippy::too_many_arguments)]
fn run_command_blocking(
    program: &str,
    args: &[String],
    workdir: &Path,
    tool: &str,
    timeout: Duration,
    cancel: Option<&AtomicBool>,
    sandbox: Option<&SandboxPlan>,
    extras: &ChildExtras,
) -> Result<CommandOutput, ToolCallError> {
    let prepared = prepare_command(program, args, sandbox, extras)
        .map_err(|e| tool_error(format!("{tool}: {}", spawn_failure(program, args, &e))))?;
    let mut command = prepared.command();
    command
        .current_dir(workdir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env_clear()
        .envs(scrubbed_env());
    extras.apply(&mut command);
    // Own process group so a timeout can take down grandchildren too.
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    let mut child = command
        .spawn()
        .map_err(|e| tool_error(format!("{tool}: {}", spawn_failure(program, args, &e))))?;
    let scope = ProcessScope::around(&child);

    let stdout_drain = child.stdout.take().map(Drain::spawn);
    let stderr_drain = child.stderr.take().map(Drain::spawn);

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child
            .try_wait()
            .map_err(|e| tool_error(format!("{tool}: failed to wait for '{program}': {e}")))?
        {
            Some(status) => break status,
            None => {
                let stopped = cancel.is_some_and(|flag| flag.load(Ordering::SeqCst));
                if stopped || Instant::now() >= deadline {
                    kill_process_tree(&mut child);
                    let _ = child.wait();
                    // Detach the drain threads instead of joining: a process
                    // that escaped the group (e.g. called setsid) could still
                    // hold the pipes open and wedge the join. The error must
                    // return promptly; the threads exit when the pipes close.
                    drop((stdout_drain, stderr_drain));
                    return Err(tool_error(if stopped {
                        format!("{tool}: command was stopped by you and killed")
                    } else {
                        format!(
                            "{tool}: command timed out after {}s and was killed",
                            timeout.as_secs()
                        )
                    }));
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    };

    Ok(finish_after_exit(
        &status,
        scope,
        [stdout_drain.as_ref(), stderr_drain.as_ref()],
    ))
}

/// Whether every pipe reached end of file within `grace`.
fn drained_within(drains: &[Option<&Drain>; 2], grace: Duration) -> bool {
    let finished = || {
        drains
            .iter()
            .all(|drain| drain.is_none_or(|drain| drain.thread.is_finished()))
    };
    let deadline = Instant::now() + grace;
    while Instant::now() < deadline && !finished() {
        std::thread::sleep(Duration::from_millis(5));
    }
    finished()
}

/// After the command's own process exited: keep reading for at most
/// [`PIPE_GRACE`] while something it started still writes, then stop whatever
/// it left running (`cmd &`, `nohup`) so nothing outlives the call, and say so
/// in the result. A process that detached from the group or job (`setsid`, a
/// daemon) cannot be reached; if it still holds the pipes the result says that.
fn finish_after_exit(
    status: &std::process::ExitStatus,
    scope: ProcessScope,
    drains: [Option<&Drain>; 2],
) -> CommandOutput {
    let mut drained = drained_within(&drains, PIPE_GRACE);
    let leftovers_stopped = scope.stop_leftovers();
    if leftovers_stopped && !drained {
        drained = drained_within(&drains, STOPPED_PIPE_GRACE);
    }
    let [stdout, stderr] = drains.map(|drain| drain.map(Drain::take).unwrap_or_default());
    let mut output = combine_output(status, &stdout, &stderr);
    output.leftovers_stopped = leftovers_stopped;
    let notes = [
        (
            leftovers_stopped,
            crate::prompts::TOOL_SHELL_LEFTOVERS_STOPPED,
        ),
        (!drained, crate::prompts::TOOL_SHELL_OUTPUT_HELD),
    ];
    for (_, note) in notes.iter().filter(|(applies, _)| *applies) {
        output.output.push('\n');
        output.output.push_str(note);
    }
    output
}

/// What a command starts stays reachable through this: on Unix its process
/// group (the command leads its own), on Windows a kill-on-close job.
struct ProcessScope {
    #[cfg(unix)]
    group: u32,
    #[cfg(windows)]
    job: Option<crate::kill_on_close::KillOnClose>,
}

impl ProcessScope {
    #[cfg_attr(not(any(unix, windows)), allow(unused_variables))]
    fn around(child: &std::process::Child) -> Self {
        Self {
            #[cfg(unix)]
            group: child.id(),
            // Without a job (assigning failed) leftovers are not tracked.
            #[cfg(windows)]
            job: crate::kill_on_close::KillOnClose::around(
                std::os::windows::io::AsRawHandle::as_raw_handle(child),
            )
            .ok(),
        }
    }

    /// Stops what is still running after the command exited; true when
    /// anything was.
    fn stop_leftovers(self) -> bool {
        #[cfg(unix)]
        {
            sweep_group(self.group)
        }
        // Dropping the job's only handle ends every process left in it. A
        // console host may outlive its client by a moment; that is no leftover.
        #[cfg(windows)]
        {
            self.job.is_some_and(|job| {
                let settle = Instant::now() + Duration::from_millis(300);
                while job.active_processes() > 0 && Instant::now() < settle {
                    std::thread::sleep(Duration::from_millis(10));
                }
                job.active_processes() > 0
            })
        }
        #[cfg(not(any(unix, windows)))]
        {
            false
        }
    }
}

/// Stops what is left of process group `pgid` after its leader exited:
/// SIGTERM, a short grace, then SIGKILL, only while the group still has
/// members. Returns whether it had any.
pub(super) fn sweep_group(pgid: u32) -> bool {
    #[cfg(unix)]
    {
        let pgid = pgid as libc::pid_t;
        // SAFETY: signal 0 only probes for existence.
        let exists = |pgid| unsafe { libc::killpg(pgid, 0) } == 0;
        if !exists(pgid) {
            return false;
        }
        // SAFETY: the group was created by the spawn that owns `pgid`.
        unsafe { libc::killpg(pgid, libc::SIGTERM) };
        let grace = Instant::now() + Duration::from_millis(300);
        while Instant::now() < grace && exists(pgid) {
            std::thread::sleep(Duration::from_millis(10));
        }
        if exists(pgid) {
            // SAFETY: as above.
            unsafe { libc::killpg(pgid, libc::SIGKILL) };
        }
        true
    }
    #[cfg(not(unix))]
    {
        let _ = pgid;
        false
    }
}

/// What a run adds to its commands' environment: its plugins' `bin` folders and the `themis` CLI
/// access (see [`crate::agent_cli`]). Read in the run's task, before a worker thread takes over.
#[derive(Debug, Clone, Default)]
pub struct ChildExtras {
    /// The `bin` folders of the run's plugins.
    pub plugin_bins: Vec<PathBuf>,
    /// The run's credential, the app server's port and the folder of the app's `themis`.
    pub agent_cli: Option<crate::agent_cli::Access>,
}

impl ChildExtras {
    /// The extras of the run whose task this is (none outside a run).
    pub fn current() -> Self {
        Self {
            plugin_bins: crate::plugins::hooks::plugin_bins(),
            agent_cli: crate::agent_cli::current(),
        }
    }

    /// Sets `PATH` and the run's `themis` variables on a command whose environment was scrubbed.
    pub(crate) fn apply(&self, command: &mut std::process::Command) {
        if let Some(path) = child_path(self) {
            command.env("PATH", path);
        }
        if let Some(access) = &self.agent_cli {
            command.envs(access.env());
        }
    }
}

/// The child's `PATH`: the app's own `themis` folder first (it holds nothing else), then the
/// scrubbed `PATH`, then the `bin` folders of the run's plugins, so a plugin's program is found but
/// never comes ahead of a system one of the same name.
pub(crate) fn child_path(extras: &ChildExtras) -> Option<std::ffi::OsString> {
    let system = scrubbed_env()
        .into_iter()
        .find(|(name, _)| name.to_string_lossy().eq_ignore_ascii_case("PATH"))
        .map(|(_, value)| value);
    let first = extras
        .agent_cli
        .as_ref()
        .and_then(|access| access.bin_dir())
        .map(Path::to_path_buf);
    if extras.plugin_bins.is_empty() && first.is_none() {
        return system;
    }
    let all = first
        .into_iter()
        .chain(system.iter().flat_map(std::env::split_paths))
        .chain(extras.plugin_bins.iter().cloned());
    std::env::join_paths(all).ok().or(system)
}

/// `program args` wrapped for `sandbox` (unchanged without one). The program is
/// resolved against the scrubbed child `PATH`, the one the child would use.
pub(crate) fn prepare_command(
    program: &str,
    args: &[String],
    sandbox: Option<&SandboxPlan>,
    extras: &ChildExtras,
) -> std::io::Result<crate::sandbox::Prepared> {
    let path = child_path(extras);
    match sandbox {
        Some(plan) => plan.prepare(program, args, path.as_deref()),
        None => crate::sandbox::SandboxPlan::unsandboxed(program, args),
    }
}

/// Text appended to a command's output when the sandbox blocked it.
pub(super) fn sandbox_note(reason: &str, escalation_denied: bool) -> String {
    let template = if escalation_denied {
        crate::prompts::TOOL_SANDBOX_BLOCKED_UNAPPROVED
    } else {
        crate::prompts::TOOL_SANDBOX_BLOCKED
    };
    format!(
        "\n{}",
        crate::prompts::fill(template, &[("reason", reason)])
    )
}

/// Message for a program that could not be started. When the "program" holds
/// whitespace and no executable of that exact name exists, the model almost
/// certainly put a whole command line in `command`; say exactly how to split
/// it. The call is never auto-split: approvals must see the argv as sent.
pub(crate) fn spawn_failure(program: &str, args: &[String], error: &std::io::Error) -> String {
    let base = format!("failed to start '{program}': {error}");
    if error.kind() != std::io::ErrorKind::NotFound || !program.contains(char::is_whitespace) {
        return base;
    }
    let mut parts = program.split_whitespace();
    let Some(head) = parts.next() else {
        return base;
    };
    // A path that merely contains spaces (`/Applications/My App/bin/x`) is not
    // a command line: only suggest a split when the first token can be a
    // program, i.e. it is not a path-like prefix that does not exist.
    if head.contains(['/', '\\']) && !Path::new(head).exists() {
        return format!(
            "{base}. If this path contains spaces, check that it exists and is spelled exactly; \
             'command' must be only the program (arguments go in 'args', one element each)"
        );
    }
    let argv: Vec<&str> = parts.chain(args.iter().map(String::as_str)).collect();
    let rendered = serde_json::to_string(&argv).unwrap_or_else(|_| "[]".into());
    format!(
        "{base}. 'command' must be only the program name; every argument is a separate element of 'args'. \
         Retry exactly as: command: {}, args: {rendered}",
        serde_json::to_string(head).unwrap_or_default()
    )
}

/// Kill `child` and every descendant that stayed in its process tree.
///
/// Unix: the child leads its own process group, so the group gets SIGTERM, a
/// short grace period, then SIGKILL. Windows: `taskkill /T /F` walks the
/// process tree (descendants that detached from the tree are not reached).
pub(super) fn kill_process_tree(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        let pgid = child.id() as libc::pid_t;
        // SAFETY: killpg only sends a signal to the group we created.
        unsafe { libc::killpg(pgid, libc::SIGTERM) };
        let grace = Instant::now() + Duration::from_millis(300);
        let mut reaped = false;
        while Instant::now() < grace {
            if matches!(child.try_wait(), Ok(Some(_))) {
                reaped = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        // Follow with SIGKILL: the leader may have exited on SIGTERM while a
        // grandchild ignored it. Skip it only when the leader is reaped and
        // the group is empty (ESRCH), so a recycled group id is never hit.
        // SAFETY: signal 0 only probes for existence.
        let group_gone = reaped
            && unsafe { libc::killpg(pgid, 0) } == -1
            && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH);
        if !group_gone {
            unsafe { libc::killpg(pgid, libc::SIGKILL) };
        }
    }
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("taskkill")
            .args(["/T", "/F", "/PID", &child.id().to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let _ = child.kill();
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = child.kill();
    }
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
        leftovers_stopped: false,
    }
}

/// Build the scrubbed environment inherited by spawned child processes.
///
/// The parent environment is *not* inherited wholesale (`env_clear` + this
/// allowlist). Names are compared case-insensitively so Windows' `Path`
/// matches `PATH`. Exact allowlist:
///
/// - All platforms: `PATH`, `TMPDIR`, `TEMP`, `TMP`.
/// - Locale and terminal: `LANG`, `LC_ALL` and any `LC_*`, `TERM`.
/// - Unix: `HOME`, `USER`, `LOGNAME`, `SSH_AUTH_SOCK` (agent socket path only;
///   keys stay in the agent).
/// - Windows: `SYSTEMROOT`, `SYSTEMDRIVE`, `USERNAME`, `USERPROFILE`,
///   `HOMEDRIVE`, `HOMEPATH`.
///
/// Any variable whose *name* contains `KEY`, `TOKEN`, or `SECRET`
/// (case-insensitive) is **never** inherited, even if it were allowlisted —
/// the blocklist is checked first (see [`child_env_allowed`]). Values are
/// never inspected. Locale names never contain those words, so they pass.
pub(super) fn scrubbed_env() -> Vec<(std::ffi::OsString, std::ffi::OsString)> {
    std::env::vars_os()
        .filter(|(name, _)| child_env_allowed(&name.to_string_lossy()))
        .collect()
}

/// Decide whether one environment variable `name` may pass to a child.
///
/// Blocklist first: names containing `KEY`, `TOKEN`, or `SECRET`
/// (case-insensitive) are always rejected. Otherwise the name must be on the
/// [`scrubbed_env`] allowlist (compared case-insensitively).
pub(super) fn child_env_allowed(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    if upper.contains("KEY") || upper.contains("TOKEN") || upper.contains("SECRET") {
        return false;
    }
    const COMMON: &[&str] = &["PATH", "TMPDIR", "TEMP", "TMP"];
    if COMMON.contains(&upper.as_str()) || upper == "LANG" || upper.starts_with("LC_") {
        return true;
    }
    if upper == "TERM" {
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
        const UNIX: &[&str] = &["HOME", "USER", "LOGNAME", "SSH_AUTH_SOCK"];
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
