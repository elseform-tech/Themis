//! Interactive approval hook: read actions auto-pass, everything else asks the
//! UI through an `approval-request` event and blocks for the decision.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::json;
use themis_core::diagnostics::DiagnosticContext;
use themis_core::tools::{Approval, ApprovalHook, RiskLevel as CoreRisk, ToolAction};

use crate::sink::EventSink;
use crate::types::{ApprovalExpired, ApprovalRequest, RiskLevel};

/// How long [`DesktopApprovalHook::approve`] waits for a UI decision before
/// denying, unless Settings › Permissions › Approval timeout says otherwise.
pub const APPROVAL_TIMEOUT: Duration =
    Duration::from_secs(crate::types::default_approval_timeout_seconds() as u64);

/// One outstanding approval dialog: the owning thread plus the rendezvous the
/// decision arrives on.
pub struct PendingApproval {
    pub request: ApprovalRequest,
    /// Thread that raised the approval.
    pub thread_id: String,
    /// Receives the UI decision; the hook blocks on the other end.
    pub sender: std::sync::mpsc::Sender<Approval>,
}

/// Approval id -> outstanding dialog, shared between hooks and `approve_action`.
pub type PendingMap = Arc<Mutex<HashMap<String, PendingApproval>>>;

/// [`ApprovalHook`] that routes non-read actions to the desktop UI.
///
/// Policy:
/// - `Read` risk is auto-allowed without a dialog.
/// - On non-git projects everything else is denied outright (read-only mode).
/// - Otherwise an [`ApprovalRequest`] is emitted on the sink and `approve`
///   blocks (with [`APPROVAL_TIMEOUT`]) for [`crate::state::AppState::approve_action`]
///   to deliver the decision. Timeout, expiry, and channel loss all deny; a
///   timeout also emits an `approval-expired` event so the UI drops the dialog.
/// - Destructive actions cannot be granted for the chat: a "chat" answer is
///   downgraded to allow-once, and unattended runs deny them without asking.
///
/// The block runs inside [`tokio::task::block_in_place`] when called on a
/// Tokio worker so the wait never stalls the executor; Tauri's async runtime
/// is multi-threaded (`TokioRuntime::new`), so this holds in production. Off a
/// runtime (plain sync callers) it blocks the calling thread directly.
pub struct DesktopApprovalHook {
    thread_id: String,
    is_git: bool,
    sink: Arc<dyn EventSink>,
    pending: PendingMap,
    timeout: Duration,
    confirm_reads: bool,
    unattended: bool,
    test_decisions: Option<Arc<Mutex<VecDeque<Approval>>>>,
    audit: Option<DiagnosticContext>,
    subagent: Option<crate::types::SubagentLabel>,
    steer_waiting: Option<SteerWaiting>,
    asked: Option<Asked>,
}

/// Whether a message the person steered in is waiting for the run to read it.
pub type SteerWaiting = Arc<dyn Fn() -> bool + Send + Sync>;

/// Told the id of each approval box this hook shows (a Vision's chat wakes its Vision).
pub type Asked = Arc<dyn Fn(&str) + Send + Sync>;

impl DesktopApprovalHook {
    /// Requests from this hook name the subagent that asks.
    pub fn with_subagent(mut self, id: &str, name: &str) -> Self {
        self.subagent = Some(crate::types::SubagentLabel {
            id: id.to_owned(),
            name: name.to_owned(),
        });
        self
    }
    /// Creates a hook for `thread_id` emitting on `sink`.
    pub fn new(
        thread_id: String,
        is_git: bool,
        sink: Arc<dyn EventSink>,
        pending: PendingMap,
    ) -> Self {
        Self {
            thread_id,
            is_git,
            sink,
            pending,
            timeout: APPROVAL_TIMEOUT,
            confirm_reads: false,
            unattended: false,
            test_decisions: None,
            audit: None,
            subagent: None,
            steer_waiting: None,
            asked: None,
        }
    }

    /// Calls `asked` with the id of each approval box shown.
    #[must_use]
    pub fn with_asked(mut self, asked: Asked) -> Self {
        self.asked = Some(asked);
        self
    }

    /// While a steered message waits to be read, actions are denied with no
    /// dialog (the next call of a batch would otherwise block it all over again).
    #[must_use]
    pub fn with_steer_waiting(mut self, steer_waiting: SteerWaiting) -> Self {
        self.steer_waiting = Some(steer_waiting);
        self
    }

    /// Logs each dialog's answer (`approval/answered`) to the run's diagnostics.
    #[must_use]
    pub fn with_audit(mut self, context: DiagnosticContext) -> Self {
        self.audit = Some(context);
        self
    }

    /// What the person answered, before any downgrade: `once`, `chat` ("Allow in
    /// this chat"), `deny`, `new_message` when they steered a message past the
    /// dialog, or `none` when the dialog expired or was dropped.
    fn log_answer(&self, action: &ToolAction, decision: Option<Approval>, waited: Duration) {
        let Some(context) = &self.audit else {
            return;
        };
        let answer = match decision {
            Some(Approval::AllowOnce) => "once",
            Some(Approval::AllowAlways) => "chat",
            Some(Approval::Deny | Approval::DenyByPerson) => "deny",
            Some(Approval::DenyForNewMessage) => "new_message",
            Some(Approval::DenyReviewed) => "reviewed",
            Some(Approval::DenyTimedOut) => "timed_out",
            None => "none",
        };
        let _ = context.log.append_with_level(
            "approval",
            "answered",
            "info",
            json!({"thread_id": context.thread_id, "run_id": context.run_id, "tool": action.tool,
                   "risk": format!("{:?}", action.risk), "answer": answer, "waited_ms": waited.as_millis()}),
            Some(&context.level),
        );
    }

    /// Overrides the decision wait (tests use a short timeout).
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    #[must_use]
    pub fn with_read_approval(mut self, confirm_reads: bool) -> Self {
        self.confirm_reads = confirm_reads;
        self
    }

    /// Marks a run nobody is watching (an automation): actions that always
    /// need a fresh answer (destructive ones) are denied instead of asked.
    #[must_use]
    pub fn with_unattended(mut self, unattended: bool) -> Self {
        self.unattended = unattended;
        self
    }

    /// Serves decisions from `decisions` in order instead of emitting dialogs
    /// (headless-test seam; an exhausted queue denies).
    #[must_use]
    pub fn with_test_decisions(mut self, decisions: Vec<Approval>) -> Self {
        self.test_decisions = Some(Arc::new(Mutex::new(decisions.into())));
        self
    }
}

impl ApprovalHook for DesktopApprovalHook {
    fn would_ask(&self, action: &ToolAction) -> bool {
        let escape = action.tool == themis_core::tools::UNSANDBOXED_TOOL;
        if action.risk == CoreRisk::Read && !self.confirm_reads && !escape {
            return false;
        }
        let integration = integration_tool(action);
        (self.is_git || (!escape && (action.risk == CoreRisk::Read || integration)))
            && !(self.unattended && (escape || !action.grantable()))
    }

    fn approve(&self, action: &ToolAction) -> Approval {
        self.ask(action, Vec::new(), false)
    }

    fn approve_first(&self, action: &ToolAction) -> Approval {
        self.ask(action, Vec::new(), true)
    }

    fn approve_sensitive(
        &self,
        action: &ToolAction,
        access: &themis_core::tools::sensitive::SensitiveAccess,
    ) -> Approval {
        self.ask(action, access.names(), false)
    }
}

impl DesktopApprovalHook {
    fn ask(&self, action: &ToolAction, sensitive_files: Vec<String>, ask_first: bool) -> Approval {
        let escape = action.tool == themis_core::tools::UNSANDBOXED_TOOL;
        if action.risk == CoreRisk::Read
            && !self.confirm_reads
            && !escape
            && sensitive_files.is_empty()
        {
            return Approval::AllowOnce;
        }
        // Leaving the sandbox needs a person watching: automations never ask
        // for it (an existing chat grant was already checked by the caller).
        if escape && self.unattended {
            return Approval::Deny;
        }
        let integration = integration_tool(action);
        if !self.is_git && (escape || (action.risk != CoreRisk::Read && !integration)) {
            return Approval::Deny;
        }
        let grantable = action.grantable();
        if self.unattended && !grantable {
            return Approval::Deny;
        }
        if let Some(queue) = &self.test_decisions {
            return queue
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .pop_front()
                .unwrap_or(Approval::Deny);
        }
        let approval_id = uuid::Uuid::new_v4().to_string();
        let request = ApprovalRequest {
            thread_id: self.thread_id.clone(),
            approval_id: approval_id.clone(),
            tool: action.tool.clone(),
            summary: action.summary.clone(),
            risk: RiskLevel::from(action.risk),
            grant_scope: action.grant_scope(),
            grant_site: grant_site(action),
            grantable,
            command: action.display.clone(),
            subagent: self.subagent.clone(),
            sensitive_files,
            ask_first,
        };
        let (sender, receiver) = std::sync::mpsc::channel();
        {
            let mut pending = self
                .pending
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            // Under the lock steering takes to deny what waits, after it fills the
            // inbox: an action either sees the message here or is there to be denied.
            if self.steer_waiting.as_ref().is_some_and(|waiting| waiting()) {
                return Approval::DenyForNewMessage;
            }
            pending.insert(
                approval_id.clone(),
                PendingApproval {
                    request: request.clone(),
                    thread_id: self.thread_id.clone(),
                    sender,
                },
            );
        }
        let asked = Instant::now();
        self.sink.emit_approval_request(&request);
        if let Some(asked) = &self.asked {
            asked(&approval_id);
        }
        let decision = wait_for_decision(&receiver, self.timeout);
        self.log_answer(action, decision, asked.elapsed());
        // `approve_action` removes its entry on success; this covers timeouts.
        self.pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&approval_id);
        self.settle(decision, approval_id, grantable, ask_first)
    }

    /// What the box's answer means for the step. The person's own Deny on an approval box tells
    /// the agent to ask how to go on; a subagent (which cannot stop to wait) and a destructive
    /// step's question box ("Skip it") keep the plain refusal.
    fn settle(
        &self,
        decision: Option<Approval>,
        approval_id: String,
        grantable: bool,
        ask_first: bool,
    ) -> Approval {
        match decision {
            // Only grantable actions may be remembered for the chat.
            Some(Approval::AllowAlways) if !grantable => Approval::AllowOnce,
            Some(Approval::Deny) if !ask_first && self.subagent.is_none() => Approval::DenyByPerson,
            // Sent from here, after the request above, so no client can see them
            // out of order and be left with a dead box.
            Some(Approval::DenyForNewMessage) => {
                self.sink.emit_approval_expired(&ApprovalExpired {
                    thread_id: self.thread_id.clone(),
                    approval_id,
                    steered: true,
                });
                Approval::DenyForNewMessage
            }
            Some(decision) => decision,
            None => {
                self.sink.emit_approval_expired(&ApprovalExpired {
                    thread_id: self.thread_id.clone(),
                    approval_id,
                    steered: false,
                });
                Approval::DenyTimedOut
            }
        }
    }
}

/// Integration and management tools, which a non-Git project may still be asked about.
/// The site an "allow" answer covers and, for the browser, the Settings › Browser permission
/// "Always allow on this site" saves for the action asked about.
fn grant_site(action: &ToolAction) -> Option<crate::types::GrantSite> {
    action
        .site_grant()
        .map(|(tools, site)| crate::types::GrantSite {
            tools,
            site,
            permission: action
                .browser_action()
                .and_then(themis_core::tools::site_permissions::browser_permission)
                .map(|permission| permission.label().to_owned()),
        })
}

/// Calls that change integrations, not the project: an MCP tool, a hook, an automation, or a
/// bare `themis plugin|skill|mcp|hook …` in the shell. A non-Git project still lets the person
/// allow them.
fn integration_tool(action: &ToolAction) -> bool {
    ["mcp_", "hook_", "manage_automations_"]
        .iter()
        .any(|prefix| action.tool.starts_with(prefix))
        || (action.tool == "shell"
            && themis_core::tools::themis_cli::manages_integrations(&action.command))
}

/// Blocks for the UI decision, yielding the Tokio worker while waiting.
fn wait_for_decision(
    receiver: &std::sync::mpsc::Receiver<Approval>,
    timeout: Duration,
) -> Option<Approval> {
    let wait = || receiver.recv_timeout(timeout).ok();
    if tokio::runtime::Handle::try_current().is_ok() {
        tokio::task::block_in_place(wait)
    } else {
        wait()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sink::{ChannelSink, TestEvent};
    use themis_core::tools::RiskLevel as CoreRisk;

    fn action(tool: &str, risk: CoreRisk) -> ToolAction {
        ToolAction {
            display: None,
            command: Vec::new(),
            tool: tool.to_owned(),
            summary: format!("test {tool}"),
            risk,
        }
    }

    fn hook_with_sink(
        is_git: bool,
    ) -> (
        DesktopApprovalHook,
        tokio::sync::mpsc::UnboundedReceiver<TestEvent>,
    ) {
        let (sink, rx) = ChannelSink::channel();
        let hook = DesktopApprovalHook::new(
            "thread-1".to_owned(),
            is_git,
            Arc::new(sink),
            PendingMap::default(),
        );
        (hook, rx)
    }

    #[test]
    fn non_git_integrations_still_require_and_accept_explicit_approval() {
        let (hook, _) = hook_with_sink(false);
        let hook = hook.with_test_decisions(vec![Approval::AllowOnce]);
        assert_eq!(
            hook.approve(&action("manage_automations_create", CoreRisk::Write)),
            Approval::AllowOnce
        );
        // A bare `themis` integration change, as the old integration tools were; any other
        // shell write stays refused.
        let shell = |argv: &[&str], risk| ToolAction {
            command: argv.iter().map(|word| (*word).to_owned()).collect(),
            ..action("shell", risk)
        };
        let hook = hook.with_test_decisions(vec![Approval::AllowOnce, Approval::AllowOnce]);
        assert_eq!(
            hook.approve(&shell(
                &["themis", "plugin", "install", "x@y"],
                CoreRisk::Network
            )),
            Approval::AllowOnce
        );
        assert_eq!(
            hook.approve(&shell(
                &["themis", "thread", "send", "t", "hi"],
                CoreRisk::Execute
            )),
            Approval::Deny
        );
        assert_eq!(
            hook.approve(&shell(
                &["./themis", "mcp", "add", "p", "-"],
                CoreRisk::Execute
            )),
            Approval::Deny
        );
        assert_eq!(
            hook.approve(&action("write_file", CoreRisk::Write)),
            Approval::Deny
        );
    }

    #[test]
    fn destructive_git_dialog_carries_the_full_command() {
        let (hook, mut rx) = hook_with_sink(true);
        let hook = hook.with_timeout(Duration::from_millis(10));
        let argv: Vec<String> = ["git", "push", "--force", "origin", "main"]
            .iter()
            .map(|t| t.to_string())
            .collect();
        let git = ToolAction {
            display: Some(themis_core::tools::CommandDisplay {
                argv: argv.clone(),
                cwd: Some("/repo".into()),
                background: false,
                sandbox_blocked: None,
            }),
            command: argv.clone(),
            tool: "shell".into(),
            summary: "run git".into(),
            risk: CoreRisk::Execute,
        };
        assert_eq!(hook.approve(&git), Approval::DenyTimedOut);
        let Ok(TestEvent::Approval(request)) = rx.try_recv() else {
            panic!("expected an approval request");
        };
        assert_eq!(request.command.unwrap().argv, argv);
        let scope = request.grant_scope.unwrap();
        assert_eq!(scope.argv, argv);
        assert!(!scope.covers_further_args);
    }

    fn escape(risk: CoreRisk) -> ToolAction {
        let mut action = action(themis_core::tools::UNSANDBOXED_TOOL, risk);
        action.command = vec!["git".into(), "status".into()];
        action
    }

    #[test]
    fn leaving_the_sandbox_is_never_approved_by_a_shortcut() {
        // The read shortcut does not apply: a dialog is raised (and expires).
        let (hook, mut rx) = hook_with_sink(true);
        let hook = hook.with_timeout(Duration::from_millis(10));
        assert_eq!(
            hook.approve(&escape(CoreRisk::Read)),
            Approval::DenyTimedOut
        );
        assert!(matches!(rx.try_recv(), Ok(TestEvent::Approval(_))));
        // Unattended runs never ask for it.
        let (hook, mut rx) = hook_with_sink(true);
        let hook = hook
            .with_unattended(true)
            .with_test_decisions(vec![Approval::AllowOnce]);
        assert_eq!(hook.approve(&escape(CoreRisk::Execute)), Approval::Deny);
        assert!(rx.try_recv().is_err());
        // Non-git projects refuse it outright.
        let (hook, _) = hook_with_sink(false);
        let hook = hook.with_test_decisions(vec![Approval::AllowOnce]);
        assert_eq!(hook.approve(&escape(CoreRisk::Execute)), Approval::Deny);
        // A person's answer is what allows it.
        let (hook, _) = hook_with_sink(true);
        let hook = hook.with_test_decisions(vec![Approval::AllowOnce]);
        assert_eq!(
            hook.approve(&escape(CoreRisk::Execute)),
            Approval::AllowOnce
        );
    }

    #[test]
    fn read_actions_auto_allow_without_an_event() {
        let (hook, mut rx) = hook_with_sink(true);
        assert_eq!(
            hook.approve(&action("read_file", CoreRisk::Read)),
            Approval::AllowOnce
        );
        assert!(rx.try_recv().is_err(), "no dialog for reads");
    }

    #[test]
    fn read_confirmation_can_require_an_approval() {
        let (hook, mut rx) = hook_with_sink(true);
        let hook = hook
            .with_read_approval(true)
            .with_timeout(Duration::from_millis(10));
        assert_eq!(
            hook.approve(&action("read_file", CoreRisk::Read)),
            Approval::DenyTimedOut
        );
        assert!(matches!(rx.try_recv(), Ok(TestEvent::Approval(_))));
    }

    #[test]
    fn non_git_projects_deny_writes_without_an_event() {
        let (hook, mut rx) = hook_with_sink(false);
        assert_eq!(
            hook.approve(&action("write_file", CoreRisk::Write)),
            Approval::Deny
        );
        assert!(rx.try_recv().is_err(), "no dialog when read-only");
        // Reads still pass on non-git projects.
        assert_eq!(
            hook.approve(&action("read_file", CoreRisk::Read)),
            Approval::AllowOnce
        );
    }

    #[test]
    fn injected_decisions_serve_in_order_then_deny() {
        let (sink, _) = ChannelSink::channel();
        let hook = DesktopApprovalHook::new(
            "thread-1".to_owned(),
            true,
            Arc::new(sink),
            PendingMap::default(),
        )
        .with_test_decisions(vec![Approval::AllowOnce, Approval::AllowAlways]);
        assert_eq!(
            hook.approve(&action("shell", CoreRisk::Execute)),
            Approval::AllowOnce
        );
        assert_eq!(
            hook.approve(&action("shell", CoreRisk::Execute)),
            Approval::AllowAlways
        );
        assert_eq!(
            hook.approve(&action("shell", CoreRisk::Execute)),
            Approval::Deny
        );
    }

    #[test]
    fn approval_times_out_to_deny_and_cleans_up() {
        let (sink, mut rx) = ChannelSink::channel();
        let pending: PendingMap = PendingMap::default();
        let hook = DesktopApprovalHook::new(
            "thread-1".to_owned(),
            true,
            Arc::new(sink),
            Arc::clone(&pending),
        )
        .with_timeout(Duration::from_millis(50));
        assert_eq!(
            hook.approve(&action("shell", CoreRisk::Execute)),
            Approval::DenyTimedOut
        );
        // The dialog was raised, the stale entry removed and the UI told so.
        let event = rx.try_recv().expect("approval request emitted");
        let TestEvent::Approval(request) = event else {
            panic!("{event:?}");
        };
        let event = rx.try_recv().expect("expiry emitted");
        let TestEvent::Expired(expired) = event else {
            panic!("{event:?}");
        };
        assert_eq!(expired.approval_id, request.approval_id);
        assert_eq!(expired.thread_id, "thread-1");
        assert!(pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_empty());
    }

    #[test]
    fn answered_approvals_do_not_emit_an_expiry() {
        let (hook, mut rx) = hook_with_sink(true);
        let hook = hook.with_timeout(Duration::from_secs(30));
        let pending = Arc::clone(&hook.pending);
        let handle = std::thread::spawn(move || hook.approve(&action("shell", CoreRisk::Execute)));
        let TestEvent::Approval(request) = rx.blocking_recv().unwrap() else {
            panic!("expected a request");
        };
        pending
            .lock()
            .unwrap()
            .remove(&request.approval_id)
            .unwrap()
            .sender
            .send(Approval::Deny)
            .unwrap();
        // The person's own Deny: the agent is told to ask how to go on.
        assert_eq!(handle.join().unwrap(), Approval::DenyByPerson);
        assert!(rx.try_recv().is_err(), "no expiry for an answered dialog");
    }

    /// Answers Deny to the one box `hook` shows (a destructive step's question box when
    /// `first`), returning what the step was told.
    fn deny_once(
        hook: DesktopApprovalHook,
        mut rx: tokio::sync::mpsc::UnboundedReceiver<TestEvent>,
        first: bool,
    ) -> Approval {
        let pending = Arc::clone(&hook.pending);
        let hook = hook.with_timeout(Duration::from_secs(30));
        let handle = std::thread::spawn(move || {
            let shell = action("shell", CoreRisk::Execute);
            if first {
                hook.approve_first(&shell)
            } else {
                hook.approve(&shell)
            }
        });
        let TestEvent::Approval(request) = rx.blocking_recv().unwrap() else {
            panic!("expected a request");
        };
        let sender = pending
            .lock()
            .unwrap()
            .remove(&request.approval_id)
            .unwrap()
            .sender;
        sender.send(Approval::Deny).unwrap();
        handle.join().unwrap()
    }

    /// Deny pressed on a subagent's box, or "Skip it" on a destructive step's question box,
    /// stays a plain refusal: neither asks how to go on.
    #[test]
    fn only_the_main_agents_approval_box_deny_asks_how_to_go_on() {
        let (hook, rx) = hook_with_sink(true);
        assert_eq!(deny_once(hook, rx, false), Approval::DenyByPerson);
        let (hook, rx) = hook_with_sink(true);
        assert_eq!(deny_once(hook, rx, true), Approval::Deny);
        let (hook, rx) = hook_with_sink(true);
        let hook = hook.with_subagent("sub-1", "Researcher");
        assert_eq!(deny_once(hook, rx, false), Approval::Deny);
    }

    fn destructive() -> ToolAction {
        let argv: Vec<String> = ["git", "push", "--force"].map(str::to_owned).to_vec();
        ToolAction {
            display: None,
            command: argv,
            tool: "shell".into(),
            summary: "force push".into(),
            risk: CoreRisk::Execute,
        }
    }

    #[test]
    fn destructive_requests_are_not_grantable_and_ignore_chat_answers() {
        let (hook, mut rx) = hook_with_sink(true);
        let hook = hook.with_timeout(Duration::from_secs(30));
        let pending = Arc::clone(&hook.pending);
        let handle = std::thread::spawn(move || hook.approve(&destructive()));
        let TestEvent::Approval(request) = rx.blocking_recv().unwrap() else {
            panic!("expected a request");
        };
        assert!(!request.grantable);
        pending
            .lock()
            .unwrap()
            .remove(&request.approval_id)
            .unwrap()
            .sender
            .send(Approval::AllowAlways)
            .unwrap();
        assert_eq!(handle.join().unwrap(), Approval::AllowOnce);
        let (hook, mut rx) = hook_with_sink(true);
        let hook = hook.with_timeout(Duration::from_millis(5));
        hook.approve(&action("shell", CoreRisk::Execute));
        let TestEvent::Approval(request) = rx.try_recv().unwrap() else {
            panic!("expected a request");
        };
        assert!(request.grantable);
    }

    #[test]
    fn a_steered_message_already_waiting_denies_without_a_box() {
        let (hook, mut rx) = hook_with_sink(true);
        let waiting = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let check = Arc::clone(&waiting);
        let hook = hook
            .with_timeout(Duration::from_millis(5))
            .with_steer_waiting(Arc::new(move || {
                check.load(std::sync::atomic::Ordering::SeqCst)
            }));
        assert_eq!(
            hook.approve(&action("write_file", CoreRisk::Write)),
            Approval::DenyForNewMessage
        );
        assert!(rx.try_recv().is_err(), "no box, and nothing to take away");
        // Once the run has read it, actions ask as usual.
        waiting.store(false, std::sync::atomic::Ordering::SeqCst);
        hook.approve(&action("write_file", CoreRisk::Write));
        assert!(matches!(rx.try_recv(), Ok(TestEvent::Approval(_))));
    }

    #[test]
    fn a_new_message_takes_the_box_away_without_an_expiry() {
        let (sink, mut rx) = ChannelSink::channel();
        let pending: PendingMap = PendingMap::default();
        let hook = DesktopApprovalHook::new(
            "thread-1".to_owned(),
            true,
            Arc::new(sink),
            Arc::clone(&pending),
        )
        .with_timeout(Duration::from_secs(30));
        let handle = std::thread::spawn(move || hook.approve(&action("shell", CoreRisk::Execute)));
        let Some(TestEvent::Approval(request)) = rx.blocking_recv() else {
            panic!("expected a request");
        };
        let waiting = pending.lock().unwrap().remove(&request.approval_id);
        waiting
            .unwrap()
            .sender
            .send(Approval::DenyForNewMessage)
            .unwrap();
        assert_eq!(handle.join().unwrap(), Approval::DenyForNewMessage);
        let Some(TestEvent::Expired(expired)) = rx.blocking_recv() else {
            panic!("expected the box to be taken away");
        };
        assert_eq!(expired.approval_id, request.approval_id);
        assert!(
            expired.steered,
            "no expiry warning for the person's own act"
        );
    }

    #[test]
    fn unattended_runs_deny_destructive_actions_without_asking() {
        let (hook, mut rx) = hook_with_sink(true);
        let hook = hook
            .with_unattended(true)
            .with_timeout(Duration::from_millis(5));
        assert_eq!(hook.approve(&destructive()), Approval::Deny);
        assert!(rx.try_recv().is_err(), "nobody to ask");
        // Ordinary actions still ask (and expire when nobody answers).
        assert_eq!(
            hook.approve(&action("shell", CoreRisk::Execute)),
            Approval::DenyTimedOut
        );
        assert!(matches!(rx.try_recv(), Ok(TestEvent::Approval(_))));
    }

    #[test]
    fn blocking_roundtrip_delivers_the_sent_decision() {
        let (sink, mut rx) = ChannelSink::channel();
        let pending: PendingMap = PendingMap::default();
        let hook = DesktopApprovalHook::new(
            "thread-1".to_owned(),
            true,
            Arc::new(sink),
            Arc::clone(&pending),
        )
        .with_timeout(Duration::from_secs(30));
        // The hook blocks on a worker thread while the test answers.
        let handle = std::thread::spawn(move || hook.approve(&action("shell", CoreRisk::Execute)));
        let request = loop {
            match rx.blocking_recv().expect("event stream open") {
                TestEvent::Approval(request) => break request,
                TestEvent::Thread(_) | TestEvent::Review(_) | TestEvent::Expired(_) => continue,
            }
        };
        assert_eq!(request.thread_id, "thread-1");
        assert_eq!(request.tool, "shell");
        assert_eq!(request.risk, RiskLevel::Execute);
        let sender = pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&request.approval_id)
            .expect("pending entry")
            .sender;
        sender.send(Approval::AllowAlways).expect("deliver");
        assert_eq!(handle.join().expect("hook thread"), Approval::AllowAlways);
    }

    #[test]
    fn dialog_answers_are_logged_as_answered_not_as_the_outcome() {
        let directory = tempfile::tempdir().expect("tempdir");
        let context = DiagnosticContext {
            log: Arc::new(themis_core::diagnostics::DiagnosticLog::new(
                directory.path().join("logs"),
            )),
            thread_id: "thread-1".to_owned(),
            run_id: "run-1".to_owned(),
            level: "info".to_owned(),
        };
        let (sink, mut rx) = ChannelSink::channel();
        let pending: PendingMap = PendingMap::default();
        let hook = Arc::new(
            DesktopApprovalHook::new(
                "thread-1".to_owned(),
                true,
                Arc::new(sink),
                Arc::clone(&pending),
            )
            .with_timeout(Duration::from_millis(300))
            .with_audit(context),
        );
        // A read passes without a dialog, so nothing is answered.
        assert_eq!(
            hook.approve(&action("read_file", CoreRisk::Read)),
            Approval::AllowOnce
        );
        // "Allow in this chat" is logged as `chat`, then an expired dialog as `none`.
        let asking = Arc::clone(&hook);
        let handle =
            std::thread::spawn(move || asking.approve(&action("write_file", CoreRisk::Write)));
        let request = loop {
            if let TestEvent::Approval(request) = rx.blocking_recv().expect("event stream open") {
                break request;
            }
        };
        let sender = pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&request.approval_id)
            .expect("pending entry")
            .sender;
        sender.send(Approval::AllowAlways).expect("deliver");
        assert_eq!(handle.join().expect("hook thread"), Approval::AllowAlways);
        assert_eq!(
            hook.approve(&action("shell", CoreRisk::Execute)),
            Approval::DenyTimedOut
        );

        let text = std::fs::read_to_string(directory.path().join("logs/diagnostics-0.jsonl"))
            .expect("log");
        let answers: Vec<serde_json::Value> = text
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("json"))
            .filter(|record| record["event"] == "answered")
            .map(|record| record["details"].clone())
            .collect();
        assert_eq!(answers.len(), 2, "{text}");
        assert_eq!(
            (answers[0]["tool"].as_str(), answers[0]["answer"].as_str()),
            (Some("write_file"), Some("chat"))
        );
        assert_eq!(
            (answers[1]["tool"].as_str(), answers[1]["answer"].as_str()),
            (Some("shell"), Some("none"))
        );
        assert_eq!(answers[0]["thread_id"], "thread-1");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn blocking_roundtrip_works_on_a_tokio_worker() {
        // Same rendezvous, but the hook blocks on a real Tokio worker via
        // `block_in_place` — the production shape under Tauri's runtime.
        let (sink, mut rx) = ChannelSink::channel();
        let pending: PendingMap = PendingMap::default();
        let hook = DesktopApprovalHook::new(
            "thread-1".to_owned(),
            true,
            Arc::new(sink),
            Arc::clone(&pending),
        )
        .with_timeout(Duration::from_secs(30));
        // Called synchronously from inside an async task, exactly like the
        // tool-execution path in production.
        let blocked =
            tokio::spawn(async move { hook.approve(&action("write_file", CoreRisk::Write)) });
        let request = loop {
            match rx.recv().await.expect("event stream open") {
                TestEvent::Approval(request) => break request,
                TestEvent::Thread(_) | TestEvent::Review(_) | TestEvent::Expired(_) => continue,
            }
        };
        pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&request.approval_id)
            .expect("pending entry")
            .sender
            .send(Approval::AllowOnce)
            .expect("deliver");
        assert_eq!(blocked.await.expect("join"), Approval::AllowOnce);
    }
}
