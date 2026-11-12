//! The `themis` CLI in an agent's shell.
//!
//! Each run's commands get a short-lived credential naming the run and its chat, the port of the
//! live app server and a folder holding only the app's own `themis` at the front of `PATH`. The
//! app server issues the credential (see `crates/desktop/src/server/agent.rs`), treats every call
//! that carries it as the agent's and refuses the methods an agent may not use. The person's own
//! credential stays in the server's discovery file, which sandboxed commands cannot read (see
//! [`DISCOVERY_FILE`] and `docs/sandbox.md`).

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

/// The credential of the run, in the environment of its commands.
pub const TOKEN_ENV: &str = "THEMIS_AGENT_TOKEN";
/// The local port of the app server, in the environment of a run's commands.
pub const PORT_ENV: &str = "THEMIS_AGENT_PORT";
/// The app server's discovery file in the app-data folder. It holds the person's credential, so
/// sandboxed commands can neither read nor write it.
pub const DISCOVERY_FILE: &str = "server.json";

/// What one run's commands get: its credential, the server's port and the folder that holds the
/// app's `themis` (none when the app ships no console CLI here).
#[derive(Clone, PartialEq, Eq)]
pub struct Access {
    token: String,
    port: u16,
    bin_dir: Option<PathBuf>,
}

impl std::fmt::Debug for Access {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never the credential.
        formatter
            .debug_struct("Access")
            .field("port", &self.port)
            .field("bin_dir", &self.bin_dir)
            .finish_non_exhaustive()
    }
}

impl Access {
    pub fn new(token: String, port: u16, bin_dir: Option<PathBuf>) -> Self {
        Self {
            token,
            port,
            bin_dir,
        }
    }

    /// The variables a command of the run gets.
    pub fn env(&self) -> [(&'static str, String); 2] {
        [
            (TOKEN_ENV, self.token.clone()),
            (PORT_ENV, self.port.to_string()),
        ]
    }

    /// The folder put first on the command's `PATH`.
    pub fn bin_dir(&self) -> Option<&Path> {
        self.bin_dir.as_deref()
    }

    /// `text` with the credential masked, for output that goes back to the model.
    pub fn redact(&self, text: &str) -> String {
        if self.token.is_empty() || !text.contains(&self.token) {
            return text.to_owned();
        }
        text.replace(&self.token, "[redacted]")
    }
}

/// A tool result with every string in it masked by `access` (unchanged without one).
pub fn redact_value(value: serde_json::Value, access: Option<&Access>) -> serde_json::Value {
    use serde_json::Value;
    let Some(access) = access else {
        return value;
    };
    match value {
        Value::String(text) => Value::String(access.redact(&text)),
        Value::Array(items) => Value::Array(
            items
                .into_iter()
                .map(|item| redact_value(item, Some(access)))
                .collect(),
        ),
        Value::Object(fields) => Value::Object(
            fields
                .into_iter()
                .map(|(key, item)| (key, redact_value(item, Some(access))))
                .collect(),
        ),
        other => other,
    }
}

/// What a run's credential is bound to, captured with the run: the project it works in (the
/// server refuses a `plugin_action` naming another one) and whether its sandbox blocks the
/// network (the server then refuses the `plugin_action` actions that fetch).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RunScope {
    /// The run's project root (its worktree when it has one).
    pub project_root: Option<PathBuf>,
    /// The run's sandbox plan blocks the network (`sandbox.network: "block"`; in YOLO only when
    /// written).
    pub network_blocked: bool,
    /// The read-only built-in tools of [`CHAT_READ_TOOLS`] the run has (`recall` unless a rule
    /// denies it, `search_memory` while memory is on and no rule denies it). The server allows
    /// the method of the same name only for these, so `themis recall` and `themis memory` give
    /// a run exactly what its tools give; a subagent's run has none.
    pub reads: Vec<String>,
    /// Set for a Vision's run: the credential may also drive the chats that Vision owns (see
    /// [`VisionScope`]); `None` for every other run.
    pub vision: Option<VisionScope>,
}

/// What a Vision's run credential is bound to beyond its own chat: the Vision, the channel the
/// run works for, and that channel's project, the only one it may start chats in. The server
/// checks every call on another chat against its owner record, never against command text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VisionScope {
    pub vision_id: String,
    pub channel_id: String,
    /// The channel's project root (canonical).
    pub project_root: PathBuf,
}

/// Read-only built-in tools the agent's `themis` mirrors, by the app-server method of the same
/// name (`themis recall`, `themis memory search|show`). The server binds each call to the run's
/// own chat.
pub const CHAT_READ_TOOLS: [&str; 2] = ["recall", "search_memory"];

impl RunScope {
    /// The scope of a run working in `project_root` under `sandbox` (none: the network is not
    /// blocked, as nothing would block it).
    pub fn new(project_root: &Path, sandbox: Option<&crate::sandbox::SandboxPlan>) -> Self {
        Self {
            project_root: Some(project_root.to_path_buf()),
            network_blocked: sandbox
                .is_some_and(|plan| plan.network() == crate::sandbox::NetworkPolicy::Block),
            reads: Vec::new(),
            vision: None,
        }
    }

    /// The scope with the run's mirrored read tools: the names of [`CHAT_READ_TOOLS`] among
    /// `tools` (the run's tools), so a tool the run lacks stays refused through `themis` too.
    #[must_use]
    pub fn with_reads<'a>(mut self, tools: impl IntoIterator<Item = &'a str>) -> Self {
        self.reads = tools
            .into_iter()
            .filter(|tool| CHAT_READ_TOOLS.contains(tool))
            .map(str::to_owned)
            .collect();
        self.reads.sort_unstable();
        self.reads.dedup();
        self
    }
}

tokio::task_local! {
    static RUN_SCOPE: RunScope;
}

/// Runs `future` as a run bound to `scope`: the credential its commands get carries it.
pub async fn with_run_scope<F: std::future::Future>(scope: RunScope, future: F) -> F::Output {
    RUN_SCOPE.scope(scope, future).await
}

/// Issues and revokes run credentials (the app server).
pub trait Issuer: Send + Sync {
    /// The access for one run of a chat, issued once per run and reused by its later commands.
    /// The first issue binds the credential to `scope`.
    fn issue(&self, thread_id: &str, run_id: &str, scope: &RunScope) -> Option<Access>;
    /// Ends every credential the chat's runs hold.
    fn revoke_thread(&self, thread_id: &str);
}

/// The issuer of this process. One app server per process installs it.
static ISSUER: RwLock<Option<Arc<dyn Issuer>>> = RwLock::new(None);

/// Makes `issuer` the one commands of runs ask.
pub fn install(issuer: Arc<dyn Issuer>) {
    *ISSUER.write().unwrap_or_else(|error| error.into_inner()) = Some(issuer);
}

/// Removes `issuer` if it is still the installed one (a server that stops).
pub fn uninstall(issuer: &Arc<dyn Issuer>) {
    let mut slot = ISSUER.write().unwrap_or_else(|error| error.into_inner());
    if slot
        .as_ref()
        .is_some_and(|current| Arc::ptr_eq(current, issuer))
    {
        *slot = None;
    }
}

fn installed() -> Option<Arc<dyn Issuer>> {
    ISSUER
        .read()
        .unwrap_or_else(|error| error.into_inner())
        .clone()
}

/// The access for a run of `thread_id` bound to `scope`, when an app server runs in this process.
pub fn issue(thread_id: &str, run_id: &str, scope: &RunScope) -> Option<Access> {
    if thread_id.is_empty() || run_id.is_empty() {
        return None;
    }
    installed()?.issue(thread_id, run_id, scope)
}

/// The access for the run whose task this is (its diagnostics context and run scope), if any.
/// A task without a run scope gets none: a credential is never issued unbound.
pub fn current() -> Option<Access> {
    let (thread_id, run_id) = crate::diagnostics::ACTIVE
        .try_with(|context| (context.thread_id.clone(), context.run_id.clone()))
        .ok()?;
    let scope = RUN_SCOPE.try_with(Clone::clone).ok()?;
    issue(&thread_id, &run_id, &scope)
}

/// Ends the credentials of a chat's runs (its run finished).
pub fn revoke_thread(thread_id: &str) {
    if let Some(issuer) = installed() {
        issuer.revoke_thread(thread_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_credential_never_shows_in_debug_output_and_is_masked_in_text() {
        let access = Access::new("secret-run-token".into(), 4711, None);
        assert!(!format!("{access:?}").contains("secret-run-token"));
        assert_eq!(
            access.redact("token=secret-run-token done"),
            "token=[redacted] done"
        );
        assert_eq!(access.redact("nothing here"), "nothing here");
        let value = serde_json::json!({"text": "a secret-run-token b", "n": 1,
            "list": ["secret-run-token"]});
        assert_eq!(
            redact_value(value.clone(), Some(&access)),
            serde_json::json!({"text": "a [redacted] b", "n": 1, "list": ["[redacted]"]})
        );
        assert_eq!(redact_value(value.clone(), None), value);
        let env = access.env();
        assert_eq!(env[0], (TOKEN_ENV, "secret-run-token".to_owned()));
        assert_eq!(env[1], (PORT_ENV, "4711".to_owned()));
    }

    /// Only the mirrored read tools are kept, once each, whatever else the run has.
    #[test]
    fn a_scope_keeps_only_the_mirrored_read_tools_the_run_has() {
        let scope = RunScope::new(Path::new("/p"), None);
        assert!(scope.reads.is_empty(), "nothing unless given");
        let scope = scope.with_reads(["shell", "search_memory", "recall", "recall", "write"]);
        assert_eq!(scope.reads, vec!["recall", "search_memory"]);
        let scope = RunScope::new(Path::new("/p"), None).with_reads(["shell"]);
        assert!(scope.reads.is_empty());
    }

    #[test]
    fn nothing_is_issued_outside_a_run() {
        assert!(current().is_none());
        let scope = RunScope::default();
        assert!(issue("", "run", &scope).is_none());
        assert!(issue("thread", "", &scope).is_none());
    }

    struct Recorder(std::sync::Mutex<Vec<RunScope>>);

    impl Issuer for Recorder {
        fn issue(&self, _thread_id: &str, _run_id: &str, scope: &RunScope) -> Option<Access> {
            self.0
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push(scope.clone());
            Some(Access::new("t".into(), 1, None))
        }
        fn revoke_thread(&self, _thread_id: &str) {}
    }

    /// A run's commands get a credential bound to the run's scope; a task with a diagnostics
    /// context but no run scope gets none.
    #[tokio::test]
    async fn a_credential_is_issued_only_with_the_runs_scope() {
        let recorder = Arc::new(Recorder(std::sync::Mutex::new(Vec::new())));
        let issuer: Arc<dyn Issuer> = recorder.clone();
        install(Arc::clone(&issuer));
        let logs = tempfile::tempdir().unwrap();
        let context = || crate::diagnostics::DiagnosticContext {
            log: Arc::new(crate::diagnostics::DiagnosticLog::new(logs.path().into())),
            thread_id: "chat".into(),
            run_id: "run".into(),
            level: "info".into(),
        };
        let unbound = crate::diagnostics::ACTIVE.scope(context(), async { current() });
        assert!(unbound.await.is_none(), "no run scope, no credential");
        let scope = RunScope {
            project_root: Some(PathBuf::from("/projects/a")),
            network_blocked: true,
            reads: vec!["recall".into()],
            vision: None,
        };
        let bound = crate::diagnostics::ACTIVE.scope(
            context(),
            with_run_scope(scope.clone(), async { current() }),
        );
        assert!(bound.await.is_some());
        uninstall(&issuer);
        assert_eq!(
            *recorder.0.lock().unwrap_or_else(|error| error.into_inner()),
            vec![scope]
        );
    }
}
