//! A Vision run's fixed envelope, whatever its channel's approval mode: it never asks the person
//! anything in its hidden chat, and nothing runs but the bundled `themis`. Plain `themis` calls
//! (the program itself, run with its arguments and no shell, the only network it has) run
//! without asking, because the server's Vision grant is the real gate. Of the person's rules
//! only the deny rules apply (they still refuse `themis`); their allow rules and chat grants do
//! not, so every other command, and anything else that would be put to the person, is refused
//! with `visions-step-refused.md` going back to the Vision as the tool result. It reads, lists
//! and searches the project with the in-process read tools (`read_file`, `list_dir`,
//! `search_file`), which a Vision run has on every OS (`runs::run_tools`); its shell runs only
//! `themis`. The sandbox side is `SandboxPlan::vision_envelope`.

use std::path::Path;
use std::sync::Arc;

use themis_core::chat_grants::ChatGrants;
use themis_core::configuration::{ApprovalMode, ApprovalPolicy, ApprovalRule, PolicyAction};
use themis_core::tools::{Approval, ApprovalHook, ToolAction};

/// The in-process read tools a Vision run keeps; no program runs but `themis`.
const READ_TOOLS: [&str; 3] = ["read_file", "list_dir", "search_file"];

/// A Vision run's policy: the person's deny rules, then reads by the in-process read tools;
/// everything else asks, which here means refused.
fn envelope_policy(policy: &ApprovalPolicy) -> ApprovalPolicy {
    let denies = policy
        .rules
        .iter()
        .filter(|rule| rule.action == PolicyAction::Deny)
        .cloned();
    let reads = READ_TOOLS.map(|tool| ApprovalRule {
        tool: tool.to_owned(),
        action: PolicyAction::Allow,
        risk: Some("read".to_owned()),
        command: None,
    });
    ApprovalPolicy {
        default: PolicyAction::Ask,
        rules: denies.chain(reads).collect(),
        sensitive_files: policy.sensitive_files.clone(),
        auto_model: None,
    }
}

/// A Vision run's approvals: plain `themis` first (unless a deny rule refuses it), then the
/// envelope's policy with every question refused.
pub(super) fn envelope_approvals(policy: &ApprovalPolicy, root: &Path) -> Arc<dyn ApprovalHook> {
    let custom = themis_core::configuration::configured_hook_with_reviewer(
        ApprovalMode::Custom,
        envelope_policy(policy),
        Arc::new(Refuse),
        ChatGrants::in_memory(),
        Some(root),
        None,
    );
    Arc::new(VisionApprovals { inner: custom })
}

/// Whether `action` is the bundled `themis` program run directly: a bare `themis` (found first
/// on the run's `PATH`) with its arguments, never a path, wrapper or shell.
fn plain_themis(action: &ToolAction) -> bool {
    action.tool == "shell"
        && action
            .command
            .first()
            .is_some_and(|program| program == "themis")
}

struct VisionApprovals {
    inner: Arc<dyn ApprovalHook>,
}

impl ApprovalHook for VisionApprovals {
    fn approve(&self, action: &ToolAction) -> Approval {
        if plain_themis(action) && !self.inner.denied_by_rule(action) {
            return Approval::AllowOnce;
        }
        self.inner.approve(action)
    }

    fn denied_by_rule(&self, action: &ToolAction) -> bool {
        self.inner.denied_by_rule(action)
    }

    fn sensitive_access(
        &self,
        tool: &str,
        args: &serde_json::Value,
    ) -> Option<themis_core::tools::sensitive::SensitiveAccess> {
        self.inner.sensitive_access(tool, args)
    }

    fn approve_first(&self, action: &ToolAction) -> Approval {
        self.inner.approve_first(action)
    }

    fn approve_sensitive(
        &self,
        action: &ToolAction,
        access: &themis_core::tools::sensitive::SensitiveAccess,
    ) -> Approval {
        let decision = self.inner.approve_sensitive(action, access);
        // The question was put under the action's sensitive name; the result looks it up by
        // the action's own.
        if decision == Approval::DenyReviewed {
            Refuse::refuse(action);
        }
        decision
    }
}

/// Where Custom would ask the person: refused, with the reason for the Vision.
struct Refuse;

impl Refuse {
    fn refuse(action: &ToolAction) -> Approval {
        themis_core::tools::set_review_note(
            action,
            themis_core::prompts::VISIONS_STEP_REFUSED.to_owned(),
        );
        Approval::DenyReviewed
    }
}

impl ApprovalHook for Refuse {
    fn approve(&self, action: &ToolAction) -> Approval {
        Self::refuse(action)
    }

    fn approve_first(&self, action: &ToolAction) -> Approval {
        Self::refuse(action)
    }

    fn approve_sensitive(
        &self,
        action: &ToolAction,
        _access: &themis_core::tools::sensitive::SensitiveAccess,
    ) -> Approval {
        Self::refuse(action)
    }
}

#[cfg(test)]
mod tests {
    use themis_core::configuration::RuntimeConfig;
    use themis_core::tools::RiskLevel;

    use super::*;

    fn shell(words: &[&str], risk: RiskLevel) -> ToolAction {
        ToolAction {
            display: None,
            tool: "shell".into(),
            summary: words.join(" "),
            risk,
            command: words.iter().map(|word| (*word).to_owned()).collect(),
        }
    }

    /// Plain `themis` runs without asking; anything Custom would ask about is refused with the
    /// reason (never a box); reads in the project run; a deny rule still refuses `themis`.
    #[test]
    fn a_vision_run_runs_plain_themis_and_refuses_what_would_ask() {
        let dir = tempfile::tempdir().unwrap();
        let policy = RuntimeConfig::default().approval;
        let hook = envelope_approvals(&policy, dir.path());
        let call = shell(
            &["themis", "call", "create_thread", "{\"provider\":\"go\"}"],
            RiskLevel::Execute,
        );
        assert_eq!(hook.approve(&call), Approval::AllowOnce);
        assert!(!hook.would_ask(&call));
        for refused in [
            shell(
                &["sh", "-c", "themis call create_thread {}"],
                RiskLevel::Execute,
            ),
            shell(&["/usr/local/bin/themis", "status"], RiskLevel::Execute),
            shell(&["curl", "https://example.com"], RiskLevel::Network),
            shell(&["sh", "-c", "echo hi > note.txt"], RiskLevel::Write),
        ] {
            assert_eq!(
                hook.approve(&refused),
                Approval::DenyReviewed,
                "{refused:?}"
            );
            let note = themis_core::tools::take_review_note(&refused).unwrap_or_default();
            assert!(note.contains("a Vision's run never asks"), "{note}");
            assert!(!hook.would_ask(&refused));
        }
        let read = ToolAction {
            display: None,
            tool: "read_file".into(),
            summary: "README.md".into(),
            risk: RiskLevel::Read,
            command: Vec::new(),
        };
        assert_eq!(hook.approve(&read), Approval::AllowOnce);

        let mut denying = policy.clone();
        denying.rules.insert(
            0,
            serde_json::from_value(serde_json::json!(
                {"tool": "shell", "command": ["themis"], "action": "deny"}
            ))
            .unwrap(),
        );
        let hook = envelope_approvals(&denying, dir.path());
        assert!(hook.approve(&call).is_denied());
    }

    /// Inside a Vision run only deny rules count: the person's allow rules let nothing else run,
    /// not `curl`, not a read-only `ls`; the in-process read tools still read the project.
    #[test]
    fn allow_rules_do_not_apply_inside_a_vision_run() {
        let dir = tempfile::tempdir().unwrap();
        let mut policy = RuntimeConfig::default().approval;
        for rule in [
            serde_json::json!({"tool": "shell", "command": ["curl"], "action": "allow"}),
            serde_json::json!({"tool": "shell", "action": "allow"}),
            serde_json::json!({"tool": "*", "action": "allow"}),
        ] {
            policy
                .rules
                .insert(0, serde_json::from_value(rule).unwrap());
        }
        let hook = envelope_approvals(&policy, dir.path());
        for refused in [
            shell(&["curl", "https://example.com"], RiskLevel::Network),
            shell(&["ls", "-la"], RiskLevel::Read),
            shell(&["python3", "-c", "print(1)"], RiskLevel::Execute),
        ] {
            assert_eq!(
                hook.approve(&refused),
                Approval::DenyReviewed,
                "{refused:?}"
            );
        }
        for tool in ["read_file", "list_dir", "search_file"] {
            let read = ToolAction {
                display: None,
                tool: tool.into(),
                summary: "README.md".into(),
                risk: RiskLevel::Read,
                command: Vec::new(),
            };
            assert_eq!(hook.approve(&read), Approval::AllowOnce, "{tool}");
        }
    }
}
