use super::*;

/// Appended to every refusal the agent sees: a refused action must not be reached
/// another way (a live run restored a file with `git show ... >` after `git reset
/// --hard` was refused, losing the same uncommitted change).
pub const REFUSAL_NOTE: &str = crate::prompts::TOOL_REFUSAL;

/// Decides whether a proposed [`ToolAction`] may run.
///
/// The desktop app implements this with a dialog (allow-once /
/// allow-always-for-thread / deny); headless callers use [`AllowAllHook`],
/// [`DenyAllHook`], or the [`FnHook`] closure adapter.
/// Content characters one `read_file` call returns; follow `next_offset` for more.
pub const READ_PAGE_CHARS: usize = 8000;

pub trait ApprovalHook: Send + Sync {
    /// Return the approval decision for `action`.
    fn approve(&self, action: &ToolAction) -> Approval;

    /// A deny rule the person configured refuses `action` without asking (Custom mode
    /// only). Used where Settings already allow an action, so that no prompt is shown.
    fn denied_by_rule(&self, _action: &ToolAction) -> bool {
        false
    }

    /// The sensitive project files this tool call touches, when the hook knows the list.
    fn sensitive_access(&self, _tool: &str, _args: &Value) -> Option<sensitive::SensitiveAccess> {
        None
    }

    /// [`Self::approve`] for a call that touches sensitive files.
    fn approve_sensitive(
        &self,
        action: &ToolAction,
        _access: &sensitive::SensitiveAccess,
    ) -> Approval {
        self.approve(action)
    }

    /// Whether [`approve`](Self::approve) would put the question to a person now, rather than
    /// answer from a rule, a grant or the mode. It asks nobody.
    fn would_ask(&self, _action: &ToolAction) -> bool {
        false
    }
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
    /// Refuse to run this action for now: the person steered a new message in
    /// while it waited (or before it asked), and the agent should read that first.
    DenyForNewMessage,
    /// Refused by Auto mode's reviewer; the reason and a safer way go back to the agent
    /// (see [`take_review_note`]).
    DenyReviewed,
}

impl Approval {
    /// Whether the action must not run. Check this rather than comparing with
    /// [`Approval::Deny`], so that every refusal counts.
    #[must_use]
    pub const fn is_denied(self) -> bool {
        matches!(
            self,
            Self::Deny | Self::DenyForNewMessage | Self::DenyReviewed
        )
    }

    /// The guidance a refusal carries in the tool result.
    #[must_use]
    pub const fn refusal_note(self) -> &'static str {
        match self {
            Self::DenyForNewMessage => crate::prompts::TOOL_DENIED_NEW_MESSAGE,
            _ => REFUSAL_NOTE,
        }
    }
}

/// What Auto mode's reviewer says about an action Custom would have asked about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AutoVerdict {
    Allow,
    /// Risky but plausibly wanted: refused, with why and a safer way; the agent changes course.
    Soft {
        reason: String,
        safer: String,
    },
    /// Destructive and unrequested, or driven by content rather than the person: refused, and
    /// the person is asked in a question box while the run goes on.
    Hard {
        reason: String,
        safer: String,
    },
    /// No verdict (the reviewer model failed or was too slow): treated as a hard block.
    Unavailable,
}

/// Auto mode's reviewer, supplied by the app: it judges an action against the person's own
/// words in the chat, and tells the person about a hard block.
pub trait AutoReviewer: Send + Sync {
    fn review(&self, action: &ToolAction) -> AutoVerdict;
    /// Whether the person allowed this exact action once (their "Allow once" after a block),
    /// using it up. Gates only a person may open (running outside the sandbox, sensitive
    /// files) honour only this, never the model's verdict.
    fn allowed_once(&self, action: &ToolAction) -> bool;
    /// A hard block: ask the person (Allow once, the safer way, or skip) without stopping the run.
    fn blocked(&self, action: &ToolAction, reason: &str, safer: &str);
}

/// Notes of the reviewer's refusals, by action, waiting for their tool result. The approval and
/// the result may run on different threads, so the note is keyed by the action itself.
static REVIEW_NOTES: std::sync::Mutex<Vec<(String, String)>> = std::sync::Mutex::new(Vec::new());

fn review_key(action: &ToolAction) -> String {
    format!("{}\0{}", action.tool, action.summary)
}

/// Sets the note a [`Approval::DenyReviewed`] refusal of `action` carries back to the agent.
pub fn set_review_note(action: &ToolAction, note: String) {
    let mut notes = REVIEW_NOTES.lock().unwrap_or_else(|e| e.into_inner());
    let key = review_key(action);
    notes.retain(|(known, _)| *known != key);
    notes.push((key, note));
    let over = notes.len().saturating_sub(64);
    notes.drain(..over);
}

/// The note of the reviewer's refusal of `action`, once.
pub fn take_review_note(action: &ToolAction) -> Option<String> {
    let mut notes = REVIEW_NOTES.lock().unwrap_or_else(|e| e.into_inner());
    let key = review_key(action);
    let at = notes.iter().position(|(known, _)| *known == key)?;
    Some(notes.remove(at).1)
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

pub(super) fn check_approval(
    hook: &dyn ApprovalHook,
    action: &ToolAction,
) -> Result<(), ToolCallError> {
    check_decision(hook.approve(action), action)
}

fn check_decision(decision: Approval, action: &ToolAction) -> Result<(), ToolCallError> {
    match decision {
        Approval::AllowOnce | Approval::AllowAlways => Ok(()),
        Approval::DenyReviewed => Err(tool_error(format!(
            "approval denied for '{}': {} {}",
            action.tool,
            action.summary,
            take_review_note(action).unwrap_or_else(|| REFUSAL_NOTE.to_owned())
        ))),
        denied @ (Approval::Deny | Approval::DenyForNewMessage) => Err(tool_error(format!(
            "approval denied for '{}': {} {}",
            action.tool,
            action.summary,
            denied.refusal_note()
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

/// [`check_approval_permitted`] for a file or command tool: a call that touches a
/// sensitive file asks as one (dispatch already did, when the permit is there).
pub(super) fn check_call_permitted(
    hook: &dyn ApprovalHook,
    action: &ToolAction,
    args: &Value,
) -> Result<(), ToolCallError> {
    if take_dispatch_permit() {
        return Ok(());
    }
    match hook.sensitive_access(&action.tool, args) {
        Some(access) => check_decision(hook.approve_sensitive(action, &access), action),
        None => check_approval(hook, action),
    }
}

pub(crate) fn tool_error(message: String) -> ToolCallError {
    ToolCallError::RuntimeError(message.into())
}

/// Parse a tool's arguments. A failure names the offending argument so the model can fix
/// that one field: serde alone reports only "invalid type: map, expected a string".
pub(crate) fn parse_tool_args<T: serde::de::DeserializeOwned>(
    tool: &str,
    args: Value,
) -> Result<T, ToolCallError> {
    let error = match T::deserialize(&args) {
        Ok(parsed) => return Ok(parsed),
        Err(error) => error,
    };
    // The culprit is the field whose removal makes the rest parse, or leaves only that
    // field missing.
    let culprit = args.as_object().and_then(|object| {
        object.keys().find(|key| {
            let mut rest = object.clone();
            rest.remove(*key);
            match T::deserialize(&Value::Object(rest)) {
                Ok(_) => true,
                Err(other) => other
                    .to_string()
                    .starts_with(&format!("missing field `{key}`")),
            }
        })
    });
    Err(tool_error(match culprit {
        Some(key) => {
            format!("{tool}: argument '{key}' is invalid: {error}. Fix '{key}' and call again.")
        }
        None => format!("{tool}: invalid arguments: {error}"),
    }))
}
