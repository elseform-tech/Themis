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
/// denying (~300s per the bridge contract).
pub const APPROVAL_TIMEOUT: Duration = Duration::from_secs(300);

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
}

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
        }
    }

    /// Logs each dialog's answer (`approval/answered`) to the run's diagnostics.
    #[must_use]
    pub fn with_audit(mut self, context: DiagnosticContext) -> Self {
        self.audit = Some(context);
        self
    }

    /// What the person answered, before any downgrade: `once`, `chat` ("Allow in
    /// this chat"), `deny`, or `none` when the dialog expired or was dropped.
    fn log_answer(&self, action: &ToolAction, decision: Option<Approval>, waited: Duration) {
        let Some(context) = &self.audit else {
            return;
        };
        let answer = match decision {
            Some(Approval::AllowOnce) => "once",
            Some(Approval::AllowAlways) => "chat",
            Some(Approval::Deny) => "deny",
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
    fn approve(&self, action: &ToolAction) -> Approval {
        let escape = action.tool == themis_core::tools::UNSANDBOXED_TOOL;
        if action.risk == CoreRisk::Read && !self.confirm_reads && !escape {
            return Approval::AllowOnce;
        }
        // Leaving the sandbox needs a person watching: automations never ask
        // for it (an existing chat grant was already checked by the caller).
        if escape && self.unattended {
            return Approval::Deny;
        }
        let integration = action.tool.starts_with("mcp_")
            || action.tool.starts_with("hook_")
            || action.tool.starts_with("manage_integrations_")
            || action.tool.starts_with("manage_automations_")
            || matches!(action.tool.as_str(), "list_plugins" | "save_skill");
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
            grant_site: action
                .site_grant()
                .map(|(tools, site)| crate::types::GrantSite { tools, site }),
            grantable,
            command: action.display.clone(),
            subagent: self.subagent.clone(),
        };
        let (sender, receiver) = std::sync::mpsc::channel();
        {
            let mut pending = self
                .pending
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
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
        let decision = wait_for_decision(&receiver, self.timeout);
        self.log_answer(action, decision, asked.elapsed());
        // `approve_action` removes its entry on success; this covers timeouts.
        self.pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&approval_id);
        match decision {
            // Only grantable actions may be remembered for the chat.
            Some(Approval::AllowAlways) if !grantable => Approval::AllowOnce,
            Some(decision) => decision,
            None => {
                self.sink.emit_approval_expired(&ApprovalExpired {
                    thread_id: self.thread_id.clone(),
                    approval_id,
                });
                Approval::Deny
            }
        }
    }
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
            hook.approve(&action("manage_integrations_save", CoreRisk::Write)),
            Approval::AllowOnce
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
        assert_eq!(hook.approve(&git), Approval::Deny);
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
        assert_eq!(hook.approve(&escape(CoreRisk::Read)), Approval::Deny);
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
            Approval::Deny
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
            Approval::Deny
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
        assert_eq!(handle.join().unwrap(), Approval::Deny);
        assert!(rx.try_recv().is_err(), "no expiry for an answered dialog");
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
            Approval::Deny
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
            Approval::Deny
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
