use super::*;

/// What hooks read about the session they run in.
#[derive(Clone, Default)]
pub struct HookSession {
    /// The chat's Claude-format transcript, kept while it has hooks. It holds prompts, so it goes
    /// to the hook and never to a log.
    pub transcript_path: Option<std::path::PathBuf>,
    /// Claude's words: `default`, or `bypassPermissions` in YOLO.
    pub permission_mode: String,
    /// Stops a hook that is still running (the person's Stop).
    pub cancel: Option<Arc<std::sync::atomic::AtomicBool>>,
    /// The session this chat's run belongs to. A subagent's run has none: it has no session of
    /// its own, and `SessionStart` is the chat's.
    pub open: Option<OpenSession>,
    /// The chat's model, as `SessionStart` reports it.
    pub model: String,
    /// What `prompt` hooks ask.
    pub prompt_model: Option<PromptModel>,
    /// The `bin` folders of this run's plugins, searched after the system's for its commands.
    pub plugin_bins: Vec<std::path::PathBuf>,
    /// The skills the person attached to this turn, in Claude's words: they enter it with no call.
    pub attached_skills: Vec<String>,
    /// The skills this run's hooks have been told about, so one read in pages counts once.
    pub skills_seen: Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
    /// Told when a background hook leaves a note, which may outlive this run.
    pub on_rewake: Option<RewakeNotify>,
}

/// A chat's session, as a run sees it. A session is the chat's continuous use: it opens with the
/// chat's first run after the app started (or after it last ended), and ends when the chat is
/// deleted, the app quits or the chat has sat idle; the app keeps the rest.
#[derive(Clone, Default)]
pub struct OpenSession {
    /// This run opens the session: its instructions count as loaded, and what the plugin's session
    /// hooks left for the last one goes.
    pub opening: bool,
    /// This run opens the session of a chat that already has history.
    pub resuming: bool,
    /// Where what `SessionStart` hooks said is kept, so that every turn of the chat has it.
    pub context_path: std::path::PathBuf,
    /// The `SessionStart` hooks that have run in this session.
    pub started: Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
    /// Whether each hook no run may be there for is allowed to run. It is asked once a session,
    /// while a run can ask.
    pub unattended: Arc<std::sync::Mutex<std::collections::HashMap<String, bool>>>,
    /// Configuration files that changed since the session's last run: Claude's `source`, the path.
    pub config_changes: Vec<(String, String)>,
    /// The instruction files this project's configuration loads, by full path.
    pub instructions: Vec<String>,
}

/// Why a session ended: Claude's word, then what happened here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionEnding {
    AppQuit,
    ChatDeleted,
    ProjectDeleted,
    Idle,
}

impl SessionEnding {
    /// Claude's documented `reason`: quitting is `prompt_input_exit`, the rest `other`.
    #[must_use]
    pub fn reason(self) -> &'static str {
        match self {
            Self::AppQuit => "prompt_input_exit",
            _ => "other",
        }
    }

    /// What happened here, for scripts that care and for the diagnostics.
    #[must_use]
    pub fn detail(self) -> &'static str {
        match self {
            Self::AppQuit => "app_quit",
            Self::ChatDeleted => "chat_deleted",
            Self::ProjectDeleted => "project_deleted",
            Self::Idle => "idle",
        }
    }
}

/// What came of running a session's unattended hooks.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UnattendedOutcome {
    pub ran: usize,
    pub failed: usize,
    /// Not allowed when the session opened.
    pub not_allowed: usize,
    /// Still running when the budget ran out, and stopped.
    pub cut_short: bool,
}

/// What `SessionStart` hooks have said in this chat, by hook. Anything unreadable counts as nothing
/// said: they run again.
pub(super) fn read_session_context(path: &Path) -> std::collections::BTreeMap<String, String> {
    let mut text = String::new();
    let read = std::fs::File::open(path).and_then(|file| {
        file.take(SESSION_CONTEXT_FILE_BYTES)
            .read_to_string(&mut text)
    });
    if read.is_err() {
        return Default::default();
    }
    serde_json::from_str(&text).unwrap_or_default()
}

/// Keeps what `SessionStart` hooks said, readable only by the person: it comes from their plugins
/// and may hold anything. A failure costs that context on later turns and never stops the work.
pub(super) fn write_session_context(
    path: &Path,
    said: &std::collections::BTreeMap<String, String>,
) {
    use std::io::Write as _;
    let saved = (|| -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut options = std::fs::OpenOptions::new();
        options.create(true).write(true).truncate(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options.open(path)?;
        file.write_all(serde_json::to_string(said)?.as_bytes())
    })();
    if saved.is_err() {
        crate::diagnostics::emit("hook", "session_context_unsaved", "warn", json!({}));
    }
}

/// Most of the file of what `SessionStart` hooks said that is read back.
const SESSION_CONTEXT_FILE_BYTES: u64 = 1024 * 1024;
