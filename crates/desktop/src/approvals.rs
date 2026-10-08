//! Interactive approval hook: read actions auto-pass, everything else asks the
//! UI through an `approval-request` event and blocks for the decision.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use themis_core::tools::{Approval, ApprovalHook, RiskLevel as CoreRisk, ToolAction};

use crate::sink::EventSink;
use crate::types::{ApprovalRequest, RiskLevel};

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
///   to deliver the decision. Timeout, expiry, and channel loss all deny.
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
    test_decisions: Option<Arc<Mutex<VecDeque<Approval>>>>,
}

impl DesktopApprovalHook {
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
            test_decisions: None,
        }
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
        if action.risk == CoreRisk::Read && !self.confirm_reads {
            return Approval::AllowOnce;
        }
        let integration = action.tool.starts_with("mcp_")
            || action.tool.starts_with("hook_")
            || action.tool.starts_with("manage_integrations_")
            || action.tool.starts_with("manage_automations_")
            || matches!(action.tool.as_str(), "list_plugins" | "save_skill");
        if !self.is_git && action.risk != CoreRisk::Read && !integration {
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
        self.sink.emit_approval_request(&request);
        let decision = wait_for_decision(&receiver, self.timeout);
        // `approve_action` removes its entry on success; this covers timeouts.
        self.pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&approval_id);
        decision.unwrap_or(Approval::Deny)
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
        // The dialog was raised, then the stale entry was removed.
        let event = rx.try_recv().expect("approval request emitted");
        assert!(matches!(event, TestEvent::Approval(_)), "{event:?}");
        assert!(pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_empty());
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
                TestEvent::Thread(_) | TestEvent::Review(_) => continue,
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
                TestEvent::Thread(_) | TestEvent::Review(_) => continue,
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
