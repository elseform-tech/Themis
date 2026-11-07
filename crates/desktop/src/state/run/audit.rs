//! Logging every gated action's final outcome during a run.

use super::*;

/// Logs every gated action's final outcome (`approval/decided`: allowed or denied, by a rule,
/// a chat grant, YOLO or a person). What a person answered in a dialog is the hook's own
/// `approval/answered` event.
pub(in crate::state) struct AuditedApprovals {
    pub(in crate::state) inner: Arc<dyn ApprovalHook>,
    pub(in crate::state) context: themis_core::diagnostics::DiagnosticContext,
}

impl ApprovalHook for AuditedApprovals {
    fn denied_by_rule(&self, action: &themis_core::tools::ToolAction) -> bool {
        self.inner.denied_by_rule(action)
    }

    fn sensitive_access(
        &self,
        tool: &str,
        args: &serde_json::Value,
    ) -> Option<themis_core::tools::sensitive::SensitiveAccess> {
        self.inner.sensitive_access(tool, args)
    }

    fn approve_sensitive(
        &self,
        action: &themis_core::tools::ToolAction,
        access: &themis_core::tools::sensitive::SensitiveAccess,
    ) -> themis_core::tools::Approval {
        self.audited(action, || self.inner.approve_sensitive(action, access))
    }

    fn would_ask(&self, action: &themis_core::tools::ToolAction) -> bool {
        self.inner.would_ask(action)
    }

    fn approve(&self, action: &themis_core::tools::ToolAction) -> themis_core::tools::Approval {
        self.audited(action, || self.inner.approve(action))
    }
}

impl AuditedApprovals {
    fn audited(
        &self,
        action: &themis_core::tools::ToolAction,
        decide: impl FnOnce() -> themis_core::tools::Approval,
    ) -> themis_core::tools::Approval {
        let started = std::time::Instant::now();
        let decision = decide();
        let _ = self.context.log.append_with_level("approval", "decided", "info", json!({
            "thread_id":self.context.thread_id,"run_id":self.context.run_id,"operation_id":self.context.run_id,"tool":action.tool,
            "risk":format!("{:?}",action.risk),"outcome":if decision.is_denied() { "denied" } else { "allowed" },"elapsed_ms":started.elapsed().as_millis()
        }), Some(&self.context.level));
        decision
    }
}
