use super::*;

/// Called with a chat's id when one of its hooks has left a note for it.
pub type RewakeNotify = Arc<dyn Fn(&str) + Send + Sync>;

/// Most of a reporting hook's output the agent gets, in bytes.
pub const REWAKE_OUTPUT_LIMIT: usize = 16 * 1024;
/// Most of `rewakeMessage` or `rewakeSummary` that is kept, in bytes.
pub const REWAKE_LABEL_LIMIT: usize = 1024;
/// Notes waiting per chat; a further one pushes out the oldest, with a diagnostics event.
pub const REWAKE_NOTES_PER_CHAT: usize = 4;

/// What marks the message that starts a turn for a hook's report, whose text is the runtime's own
/// and never the hook's. The chat shows it as what it is; a saved transcript is read back with it.
pub const HOOK_WAKE_MARKER: &str = "[Plugin hook \u{2014} runtime, not from the user]";

/// What a finished background hook left for the agent (Claude's `asyncRewake`). It reaches the
/// chat's next run at its next step, labelled and fenced as the plugin's: not from the person, not
/// instructions, and it grants nothing.
#[derive(Debug, Clone)]
pub struct RewakeNote {
    pub hook: String,
    /// The plugin's line above the output.
    pub message: Option<String>,
    pub output: String,
    /// What the person sees of it, a line at most.
    pub summary: String,
}

impl RewakeNote {
    /// The note the model gets. `[[` is broken up too: in a chat message it would attach a skill.
    #[must_use]
    pub fn model_text(&self) -> String {
        let body = match &self.message {
            Some(message) => format!("{message}\n\n{}", self.output),
            None => self.output.clone(),
        };
        let fenced = body
            .replace("<<<", "< < <")
            .replace(">>>", "> > >")
            .replace("[[", "[ [");
        crate::prompts::fill(
            crate::prompts::HOOK_REWAKE_NOTE,
            &[("hook", &self.hook), ("output", &fenced)],
        )
    }
}

/// Notes waiting for each chat's next model request. They outlive the run that started the hook,
/// and not the app: the hook's process doesn't either.
static REWAKE_NOTES: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<String, std::collections::VecDeque<RewakeNote>>>,
> = std::sync::LazyLock::new(Default::default);

fn rewake_notes() -> std::sync::MutexGuard<
    'static,
    std::collections::HashMap<String, std::collections::VecDeque<RewakeNote>>,
> {
    REWAKE_NOTES.lock().unwrap_or_else(|e| e.into_inner())
}

pub(super) fn leave_rewake_note(thread_id: &str, note: RewakeNote) {
    let mut all = rewake_notes();
    let waiting = all.entry(thread_id.to_owned()).or_default();
    if waiting.len() >= REWAKE_NOTES_PER_CHAT {
        waiting.pop_front();
        crate::diagnostics::emit("hook", "rewake_dropped", "warn", json!({"hook": note.hook}));
    }
    waiting.push_back(note);
}

/// Takes the notes waiting for a chat.
pub fn take_rewake_notes(thread_id: &str) -> Vec<RewakeNote> {
    rewake_notes()
        .remove(thread_id)
        .map(Vec::from)
        .unwrap_or_default()
}

/// Whether a note is waiting for a chat.
#[must_use]
pub fn rewake_waiting(thread_id: &str) -> bool {
    rewake_notes().contains_key(thread_id)
}

/// The notes for the chat's run this is, at one of its steps: a subagent's steps leave them for it.
#[must_use]
pub fn rewake_notes_for_run() -> Vec<RewakeNote> {
    ACTIVE
        .try_with(|runtime| {
            if runtime.agent.is_some() {
                Vec::new()
            } else {
                take_rewake_notes(&runtime.thread_id)
            }
        })
        .unwrap_or_default()
}

/// The first line of a report, as short as a label: what the person sees when nothing better says.
pub(super) fn first_line(text: &str) -> String {
    capped(text.lines().next().unwrap_or_default().trim(), 200).to_owned()
}

/// Background hooks running at once; past this they are skipped, with a diagnostics event.
const BACKGROUND_HOOKS: usize = 8;
static BACKGROUND_SLOTS: tokio::sync::Semaphore =
    tokio::sync::Semaphore::const_new(BACKGROUND_HOOKS);

impl HookRuntime {
    /// Starts a hook nobody waits for. It is asked about here, in the run; its time limit bounds
    /// how long it outlives the run. What comes of it only reaches the diagnostics, unless it is a
    /// hook that reports back and exits 2: then the chat gets a note. Stop ends one of those, and
    /// its report with it.
    pub(super) fn start_in_background(&self, hook: &Hook, payload: &Value) {
        let log = |event: &str, level: &str| {
            crate::diagnostics::emit("hook", event, level, json!({"hook": hook.name}));
        };
        if authorize(hook, &self.approvals).is_err() {
            log("background_denied", "warn");
            return;
        }
        let Ok(permit) = BACKGROUND_SLOTS.try_acquire() else {
            log("background_skipped", "warn");
            return;
        };
        let (hook, root, payload) = (hook.clone(), self.root.clone(), payload.clone());
        let context = crate::diagnostics::ACTIVE.try_with(Clone::clone).ok();
        let thread_id = self.thread_id.clone();
        let on_rewake = self.session.on_rewake.clone();
        let cancel = self
            .session
            .cancel
            .clone()
            .filter(|_| hook.rewake.is_some());
        let work = async move {
            let _permit = permit;
            let outcome = tokio::select! {
                outcome = IN_HOOK.scope(true, execute(&hook, &root, &payload)) => outcome,
                () = stopped(cancel) => {
                    crate::diagnostics::emit(
                        "hook",
                        "rewake_stopped",
                        "info",
                        json!({"hook": hook.name}),
                    );
                    return;
                }
            };
            crate::diagnostics::emit(
                "hook",
                "background_finished",
                if outcome.is_ok() { "info" } else { "warn" },
                json!({"hook": hook.name, "ok": outcome.is_ok()}),
            );
            let report = outcome
                .err()
                .and_then(|error| error.downcast::<HookReport>().ok());
            if let (Some(report), Some(rewake)) = (report, &hook.rewake) {
                if report.output.is_empty() {
                    return;
                }
                let summary = report
                    .summary
                    .or_else(|| rewake.summary.clone())
                    .unwrap_or_else(|| first_line(&report.output));
                leave_rewake_note(
                    &thread_id,
                    RewakeNote {
                        hook: hook.name.clone(),
                        message: rewake.message.clone(),
                        output: report.output,
                        summary: capped(&summary, REWAKE_LABEL_LIMIT).to_owned(),
                    },
                );
                // Metadata only: never what it said.
                crate::diagnostics::emit("hook", "rewake_note", "info", json!({"hook": hook.name}));
                if let Some(notify) = on_rewake {
                    notify(&thread_id);
                }
            }
        };
        tokio::spawn(async move {
            match context {
                Some(context) => crate::diagnostics::ACTIVE.scope(context, work).await,
                None => work.await,
            }
        });
    }
}
