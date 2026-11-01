//! A chat's session, as plugin hooks see it. A Themis run is one turn, so a session is the chat's
//! continuous use: it opens with the chat's first run after the app started (or after it last
//! ended) and ends once, when the chat is deleted, its project is, the app quits, or the chat has
//! sat idle for `hooks.session_idle_minutes`. `session_id` is the chat's id throughout, so a session
//! that ended and a later one read as the same conversation, as a resume does in Claude.
//!
//! A crash or a forced quit ends nothing: the next run opens a new session, as in Claude.
//!
//! What runs here has no run around it and nobody to ask, so it runs only what was allowed while a
//! run could ask, and only if it is still switched on with the same definition.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use themis_core::plugins::hooks::{self, Hook, HookRuntime, OpenSession, SessionEnding};
use themis_core::plugins::{Plugin, PluginStore};
use themis_core::tools::background::TaskState;

use super::AppState;

/// The whole of what a session's end may take when the app is quitting. A hook that needs longer
/// hands its work to a detached process, as the short budget in Claude already makes them do.
const QUIT_BUDGET: Duration = Duration::from_secs(5);
/// The same when deleting a project, whose folder is about to go.
const PROJECT_DELETE_BUDGET: Duration = Duration::from_secs(5);
/// The same when nothing is waiting: a chat deleted, or left idle.
const END_BUDGET: Duration = Duration::from_secs(60);
/// How long a `Notification` hook may take; the notification has already gone.
const NOTIFICATION_BUDGET: Duration = Duration::from_secs(10);
/// How often idle chats are looked for.
pub(crate) const IDLE_CHECK_EVERY: Duration = Duration::from_secs(60);

/// What's kept about a chat's open session between its runs.
pub(super) struct OpenHookSession {
    root: PathBuf,
    permission_mode: String,
    last_active: Instant,
    idle_after: Option<Duration>,
    started: Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
    unattended: Arc<std::sync::Mutex<std::collections::HashMap<String, bool>>>,
    /// Claude's `source`, the path and its last fingerprint.
    config: Vec<(String, String, Option<u64>)>,
}

impl OpenHookSession {
    fn scope(&self) -> SessionScope {
        SessionScope {
            root: self.root.clone(),
            permission_mode: self.permission_mode.clone(),
            allowed: self
                .unattended
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone(),
        }
    }
}

/// What a hook with no run around it runs under.
struct SessionScope {
    root: PathBuf,
    permission_mode: String,
    allowed: std::collections::HashMap<String, bool>,
}

fn fingerprint(path: &Path) -> Option<u64> {
    use std::hash::{Hash, Hasher};
    let bytes = std::fs::read(path).ok()?;
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hash);
    Some(hash.finish())
}

impl AppState {
    /// A run's part in the chat's session, opening one if none is open.
    pub(super) fn open_hook_session(
        &self,
        thread_id: &str,
        root: &Path,
        permission_mode: &str,
        idle_minutes: u32,
        instructions: Vec<String>,
        has_history: bool,
    ) -> OpenSession {
        let idle_after =
            (idle_minutes > 0).then(|| Duration::from_secs(u64::from(idle_minutes) * 60));
        let user = super::settings_app_dir(self.inner.settings.path()).join("runtime.jsonc");
        let project = root.join(".themis/config.jsonc");
        let context_path = self
            .hook_session_dir(thread_id)
            .join("session-context.json");
        let mut sessions = self
            .inner
            .hook_sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(session) = sessions.get_mut(thread_id) {
            session.root = root.to_owned();
            session.permission_mode = permission_mode.to_owned();
            session.last_active = Instant::now();
            session.idle_after = idle_after;
            let mut config_changes = Vec::new();
            for (source, path, seen) in &mut session.config {
                let now = fingerprint(Path::new(path));
                if now != *seen {
                    *seen = now;
                    config_changes.push((source.clone(), path.clone()));
                }
            }
            return OpenSession {
                opening: false,
                resuming: false,
                context_path,
                started: Arc::clone(&session.started),
                unattended: Arc::clone(&session.unattended),
                config_changes,
                instructions,
            };
        }
        let session = OpenHookSession {
            root: root.to_owned(),
            permission_mode: permission_mode.to_owned(),
            last_active: Instant::now(),
            idle_after,
            started: Arc::default(),
            unattended: Arc::default(),
            config: [("user_settings", user), ("project_settings", project)]
                .into_iter()
                .map(|(source, path)| {
                    let seen = fingerprint(&path);
                    (source.to_owned(), path.to_string_lossy().into_owned(), seen)
                })
                .collect(),
        };
        let open = OpenSession {
            opening: true,
            resuming: has_history,
            context_path,
            started: Arc::clone(&session.started),
            unattended: Arc::clone(&session.unattended),
            config_changes: Vec::new(),
            instructions,
        };
        sessions.insert(thread_id.to_owned(), session);
        drop(sessions);
        let _ = self.inner.diagnostics.append(
            "hook",
            "session_started",
            "info",
            json!({"thread_id": thread_id}),
        );
        open
    }

    /// The chat's run ended: its idle time starts now.
    pub(super) fn touch_hook_session(&self, thread_id: &str) {
        if let Some(session) = self
            .inner
            .hook_sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_mut(thread_id)
        {
            session.last_active = Instant::now();
        }
    }

    /// Takes the chat's session, so that it ends once whatever else tries.
    fn take_hook_session(&self, thread_id: &str) -> Option<OpenHookSession> {
        self.inner
            .hook_sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(thread_id)
    }

    /// Every hook of the enabled plugins as a run would have it. What decides whether one may run
    /// is its identity, which covers its whole definition.
    fn hooks_now(
        &self,
        thread_id: &str,
        root: &Path,
    ) -> Result<(PluginStore, Vec<Plugin>, Vec<Hook>), String> {
        let store = self.plugin_store(Some(root.to_owned()));
        let plugins: Vec<Plugin> = store
            .list()
            .map_err(|e| e.to_string())?
            .into_iter()
            .filter(|plugin| plugin.enabled)
            .collect();
        let session_dir = self.hook_session_dir(thread_id);
        let hooks = plugins
            .iter()
            .flat_map(|plugin| {
                plugin
                    .spec
                    .hooks
                    .iter()
                    .map(|hook| prepared_hook(hook, plugin, &store, root, &session_dir))
                    .collect::<Vec<_>>()
            })
            .collect();
        Ok((store, plugins, hooks))
    }

    /// Runs `event`'s hooks for a chat with no run around them. The plugin's files are checked and
    /// put back first, as for any run: a hook runs outside the sandbox, from a folder others can
    /// write.
    async fn run_session_hooks(
        &self,
        thread_id: &str,
        session: &SessionScope,
        event: &str,
        payload: Value,
        budget: Duration,
    ) -> Result<hooks::UnattendedOutcome, String> {
        let allowed = &session.allowed;
        let (store, plugins, every) = self.hooks_now(thread_id, &session.root)?;
        let wanted: Vec<Hook> = every
            .into_iter()
            .filter(|hook| hook.event == event)
            .collect();
        if wanted.is_empty() {
            return Ok(hooks::UnattendedOutcome::default());
        }
        let needed: Vec<Plugin> = plugins
            .into_iter()
            .filter(|plugin| {
                wanted.iter().any(|hook| {
                    hook.enabled
                        && allowed.get(&hooks::hook_identity(hook)) == Some(&true)
                        && plugin.spec.hooks.iter().any(|own| {
                            hook.runtime_identity.as_deref()
                                == Some(plugin.skill_id(&own.name).as_str())
                        })
                })
            })
            .collect();
        let root = session.root.clone();
        tokio::task::spawn_blocking(move || store.materialize(&needed, &root))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
        let runtime = HookRuntime {
            hooks: wanted,
            run_id: format!("session-{thread_id}"),
            thread_id: thread_id.to_owned(),
            root: session.root.clone(),
            approvals: Arc::new(themis_core::tools::DenyAllHook),
            agent: None,
            mcp_names: Arc::default(),
            session: hooks::HookSession {
                transcript_path: Some(self.hook_session_dir(thread_id).join("transcript.jsonl")),
                permission_mode: session.permission_mode.clone(),
                ..Default::default()
            },
        };
        Ok(runtime
            .run_unattended(event, payload, allowed, budget)
            .await)
    }

    /// Ends one chat's session, if it has one open.
    async fn end_hook_session(&self, thread_id: &str, ending: SessionEnding, budget: Duration) {
        let Some(session) = self.take_hook_session(thread_id) else {
            return;
        };
        let payload = json!({"reason": ending.reason(), "themis_reason": ending.detail()});
        let outcome = self
            .run_session_hooks(thread_id, &session.scope(), "SessionEnd", payload, budget)
            .await;
        let (level, details) = match outcome {
            Ok(outcome) => (
                if outcome.cut_short || outcome.failed > 0 {
                    "warn"
                } else {
                    "info"
                },
                json!({"thread_id": thread_id, "reason": ending.detail(), "ran": outcome.ran,
                    "failed": outcome.failed, "not_allowed": outcome.not_allowed,
                    "cut_short": outcome.cut_short}),
            ),
            // Metadata only: an error here can name a project's files.
            Err(_) => (
                "warn",
                json!({"thread_id": thread_id, "reason": ending.detail(), "unavailable": true}),
            ),
        };
        let _ = self
            .inner
            .diagnostics
            .append("hook", "session_ended", level, details);
    }

    /// Ends the sessions of chats left idle for as long as their configuration says. `now` is an
    /// argument so a check can say how much later it is.
    pub async fn end_idle_hook_sessions_at(&self, now: Instant) {
        let idle: Vec<String> = self
            .inner
            .hook_sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter(|(_, session)| {
                session.idle_after.is_some_and(|after| {
                    now.saturating_duration_since(session.last_active) >= after
                })
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in idle {
            // A chat at work isn't idle, and nor is one whose programs still run.
            let running = self
                .inner
                .threads
                .read()
                .await
                .get(&id)
                .is_some_and(|record| record.running);
            let programs = self
                .inner
                .background
                .list(&id, 0)
                .iter()
                .any(|task| task.record.state == TaskState::Running);
            if !running && !programs {
                self.end_hook_session(&id, SessionEnding::Idle, END_BUDGET)
                    .await;
            }
        }
    }

    /// Ends every open session at once, within the short budget a quitting app has.
    pub async fn end_all_hook_sessions(&self) {
        let ids: Vec<String> = self
            .inner
            .hook_sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .keys()
            .cloned()
            .collect();
        futures_util::future::join_all(
            ids.iter()
                .map(|id| self.end_hook_session(id, SessionEnding::AppQuit, QUIT_BUDGET)),
        )
        .await;
    }

    /// A chat was deleted: its session ends, then what hooks read about it goes. Nothing waits for
    /// that; the hook may read the transcript until it has finished.
    pub(super) fn end_deleted_chat_session(&self, thread_id: &str) {
        let state = self.clone();
        let thread_id = thread_id.to_owned();
        tokio::spawn(async move {
            state
                .end_hook_session(&thread_id, SessionEnding::ChatDeleted, END_BUDGET)
                .await;
            state.forget_hook_sessions(std::slice::from_ref(&thread_id));
        });
    }

    /// A project is about to go to the Trash: its chats' sessions end while their folder is there.
    pub(super) async fn end_project_sessions(&self, ids: &[String]) {
        futures_util::future::join_all(ids.iter().map(|id| {
            self.end_hook_session(id, SessionEnding::ProjectDeleted, PROJECT_DELETE_BUDGET)
        }))
        .await;
    }

    /// Themis has just notified the person about this chat: this is `Notification`, for those who
    /// want it. A chat with no open session has no hooks to tell.
    pub(super) fn notify_session_hooks(&self, thread_id: &str, kind: &'static str, message: &str) {
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let state = self.clone();
        let thread_id = thread_id.to_owned();
        let payload = json!({"message": message, "title": "Themis", "notification_type": kind});
        let scope = self
            .inner
            .hook_sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&thread_id)
            .map(OpenHookSession::scope);
        let Some(scope) = scope else {
            return;
        };
        tokio::spawn(async move {
            let _ = state
                .run_session_hooks(
                    &thread_id,
                    &scope,
                    "Notification",
                    payload,
                    NOTIFICATION_BUDGET,
                )
                .await;
        });
    }
}

/// A plugin's hook, ready to run in a chat.
pub(super) fn prepared_hook(
    hook: &Hook,
    plugin: &Plugin,
    store: &PluginStore,
    work_root: &Path,
    session_dir: &Path,
) -> Hook {
    let mut hook = hook.clone();
    hook.runtime_identity = Some(plugin.skill_id(&hook.name));
    hook.plugin_root = Some(
        work_root
            .join(".themis/plugin-files")
            .join(plugin.skill_id("resources")),
    );
    hook.plugin_data = Some(store.plugin_data_dir(&plugin.scope, &plugin.spec.name));
    hook.env_file = Some(hooks::PluginEnvFile {
        path: session_dir.join(format!("env-{}", plugin.skill_id("hooks"))),
        prefix: hooks::env_prefix(&plugin.spec.name),
    });
    hook
}
