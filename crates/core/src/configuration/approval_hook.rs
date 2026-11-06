use super::*;

struct ConfiguredHook {
    mode: ApprovalMode,
    policy: ApprovalPolicy,
    interactive: Arc<dyn ApprovalHook>,
    grants: Arc<ChatGrants>,
    sensitive: SensitiveMatcher,
    /// Auto mode's reviewer; without one, Auto blocks what it would ask about.
    reviewer: Option<Arc<dyn crate::tools::AutoReviewer>>,
    /// Soft blocks per target this run: the third becomes a hard block.
    soft_blocks: std::sync::Mutex<std::collections::HashMap<String, u8>>,
}
impl ApprovalHook for ConfiguredHook {
    fn denied_by_rule(&self, action: &ToolAction) -> bool {
        self.rule_denies(action)
    }

    fn sensitive_access(&self, tool: &str, args: &Value) -> Option<SensitiveAccess> {
        self.sensitive.classify(tool, args)
    }

    /// Both must pass: the call as any other (a deny rule, or for a command its own
    /// approval), and each file. Rules naming `sensitive_file` decide a file before a chat
    /// grant does; a wildcard allow never covers one, and a deny of any shape refuses it.
    /// YOLO keeps only the denies. "Allow in this chat" remembers each file and kind of
    /// access; for a command also the command, so neither stands in for the other.
    fn approve_sensitive(&self, action: &ToolAction, access: &SensitiveAccess) -> Approval {
        let files = access.file_actions();
        let decisions: Vec<PolicyAction> = files
            .iter()
            .map(|file| self.policy.sensitive_decision(file))
            .collect();
        if decisions.contains(&PolicyAction::Deny) {
            return Approval::Deny;
        }
        if self.mode == ApprovalMode::Yolo {
            return self.approve(action);
        }
        if self.policy.decision(action) == PolicyAction::Deny {
            return Approval::Deny;
        }
        if decisions
            .iter()
            .all(|decision| *decision == PolicyAction::Allow)
        {
            return self.approve(action);
        }
        let command = crate::tools::is_command_tool(&action.tool);
        let granted = files
            .iter()
            .zip(&decisions)
            .all(|(file, decision)| *decision == PolicyAction::Allow || self.grants.covers(file));
        if granted {
            return if command {
                self.approve(action)
            } else {
                Approval::AllowOnce
            };
        }
        crate::diagnostics::emit(
            "approval",
            "sensitive_file_asked",
            "info",
            serde_json::json!({
                "tool": action.tool,
                "kind": access.kinds().join(","),
                "files": access.files.len(),
            }),
        );
        let asked = ToolAction {
            summary: format!(
                "{} (sensitive: {})",
                action.summary,
                crate::tools::sensitive::name_list(&access.names())
            ),
            ..action.clone()
        };
        if self.mode == ApprovalMode::Auto {
            return self.person_only(&asked, "a sensitive file (secrets or keys) needs the user");
        }
        match self.interactive.approve_sensitive(&asked, access) {
            Approval::AllowAlways => {
                if action.grantable() {
                    for file in &files {
                        self.grants.remember(file);
                    }
                    if command {
                        self.grants.remember(action);
                    }
                }
                Approval::AllowOnce
            }
            decision => decision,
        }
    }

    fn would_ask(&self, action: &ToolAction) -> bool {
        if action.tool == crate::tools::UNSANDBOXED_TOOL || self.mode != ApprovalMode::Custom {
            return false;
        }
        self.policy.decision(action) == PolicyAction::Ask
            && (action.tool.starts_with("browser_") || !self.grants.covers(action))
            && self.interactive.would_ask(action)
    }

    fn approve(&self, action: &ToolAction) -> Approval {
        if action.tool == crate::tools::UNSANDBOXED_TOOL {
            return self.approve_escape(action);
        }
        if self.mode == ApprovalMode::Yolo {
            // No prompts, but a deny rule you wrote still holds.
            return match self.policy.yolo_decision(action) {
                PolicyAction::Deny => Approval::Deny,
                _ => Approval::AllowOnce,
            };
        }
        // Browser permissions are kept in Settings › Browser, not chat grants: an "always"
        // answer goes back to the browser tool, which saves it as a site exception.
        let browser = action.tool.starts_with("browser_");
        // Rules decide first, so a deny rule always beats a chat grant.
        match self.policy.decision(action) {
            PolicyAction::Allow => Approval::AllowOnce,
            PolicyAction::Deny => Approval::Deny,
            PolicyAction::Ask => {
                if !browser && self.grants.covers(action) {
                    return Approval::AllowOnce;
                }
                if self.mode == ApprovalMode::Auto {
                    return self.auto(action);
                }
                match self.interactive.approve(action) {
                    Approval::AllowAlways if browser => Approval::AllowAlways,
                    Approval::AllowAlways => {
                        // Destructive actions are allowed this once only.
                        self.grants.remember(action);
                        Approval::AllowOnce
                    }
                    decision => decision,
                }
            }
        }
    }
}
impl ConfiguredHook {
    fn rule_denies(&self, action: &ToolAction) -> bool {
        match self.mode {
            ApprovalMode::Custom | ApprovalMode::Auto => {
                self.policy.decision(action) == PolicyAction::Deny
            }
            ApprovalMode::Yolo => self.policy.yolo_decision(action) == PolicyAction::Deny,
        }
    }

    /// Auto at a gate only a person may open (running outside the sandbox, sensitive files):
    /// the reviewer model never decides. The person's "Allow once" for this exact action lets
    /// it run; otherwise it is blocked and the person is asked in a question box.
    fn person_only(&self, action: &ToolAction, reason: &str) -> Approval {
        if self
            .reviewer
            .as_ref()
            .is_some_and(|reviewer| reviewer.allowed_once(action))
        {
            return Approval::AllowOnce;
        }
        crate::diagnostics::emit(
            "approval",
            "auto_review",
            "info",
            serde_json::json!({"tool": action.tool, "verdict": "person_only"}),
        );
        let safer = "stay inside the sandbox and the project's ordinary files";
        if let Some(reviewer) = &self.reviewer {
            reviewer.blocked(action, reason, safer);
        }
        crate::tools::set_review_note(
            action,
            crate::prompts::fill(
                crate::prompts::AUTO_HARD_BLOCK,
                &[("reason", reason), ("safer", safer)],
            ),
        );
        Approval::DenyReviewed
    }

    /// Where Custom would ask the person: Auto asks its reviewer instead, never the person
    /// (the reviewer also honours an "Allow once" the person gave after a block). A soft block goes back to
    /// the agent with its reason and a safer way; the third on the same target, a hard block or
    /// no verdict also asks the person in a question box, while the run goes on.
    fn auto(&self, action: &ToolAction) -> Approval {
        let verdict = match &self.reviewer {
            Some(reviewer) => reviewer.review(action),
            None => crate::tools::AutoVerdict::Unavailable,
        };
        let target = format!("{}\0{}", action.tool, action.command.join(" "));
        let (kind, reason, safer) = match verdict {
            crate::tools::AutoVerdict::Allow => {
                crate::diagnostics::emit(
                    "approval",
                    "auto_review",
                    "info",
                    serde_json::json!({"tool": action.tool, "verdict": "allow"}),
                );
                return Approval::AllowOnce;
            }
            crate::tools::AutoVerdict::Soft { reason, safer } => {
                let mut counts = self.soft_blocks.lock().unwrap_or_else(|e| e.into_inner());
                let count = counts.entry(target).or_insert(0);
                *count = count.saturating_add(1);
                let kind = if *count >= 3 { "hard" } else { "soft" };
                (kind, reason, safer)
            }
            crate::tools::AutoVerdict::Hard { reason, safer } => ("hard", reason, safer),
            crate::tools::AutoVerdict::Unavailable => (
                "unavailable",
                "the reviewer could not judge this action".to_owned(),
                "use a reversible approach, or leave this step for later".to_owned(),
            ),
        };
        crate::diagnostics::emit(
            "approval",
            "auto_review",
            "info",
            serde_json::json!({"tool": action.tool, "verdict": kind}),
        );
        let note = if kind == "soft" {
            crate::prompts::fill(
                crate::prompts::AUTO_SOFT_BLOCK,
                &[("reason", &reason), ("safer", &safer)],
            )
        } else {
            if let Some(reviewer) = &self.reviewer {
                reviewer.blocked(action, &reason, &safer);
            }
            crate::prompts::fill(
                crate::prompts::AUTO_HARD_BLOCK,
                &[("reason", &reason), ("safer", &safer)],
            )
        };
        crate::tools::set_review_note(action, note);
        Approval::DenyReviewed
    }

    /// Running a sandbox-blocked command outside the sandbox needs a person:
    /// a dialog answer, a chat grant they made for `shell_unsandboxed`, or a
    /// rule naming `shell_unsandboxed` exactly. Wildcard, prefix and risk-only
    /// allow rules never cover it (deny rules of any shape still deny), the
    /// default action is never an allow, and YOLO (which never asks) denies.
    fn approve_escape(&self, action: &ToolAction) -> Approval {
        if self.mode == ApprovalMode::Yolo {
            return Approval::Deny;
        }
        match self.policy.escape_decision(action) {
            PolicyAction::Deny => return Approval::Deny,
            PolicyAction::Allow if action.grantable() => return Approval::AllowOnce,
            _ => {}
        }
        if self.grants.covers(action) {
            return Approval::AllowOnce;
        }
        if self.mode == ApprovalMode::Auto {
            return self.person_only(action, "running outside the sandbox needs the user");
        }
        match self.interactive.approve(action) {
            Approval::AllowAlways => {
                self.grants.remember(action);
                Approval::AllowOnce
            }
            decision => decision,
        }
    }
}

impl ApprovalPolicy {
    /// The patterns of `sensitive_files`, for files under `root`.
    #[must_use]
    pub fn sensitive_matcher(&self, root: Option<&std::path::Path>) -> SensitiveMatcher {
        SensitiveMatcher::new(&self.sensitive_files, root)
    }

    /// Decision for one sensitive file (see `SensitiveAccess::file_actions`): the first deny
    /// that matches (normal matching, wildcards included) or the first allow/ask rule whose
    /// tool is exactly `sensitive_file`, in order; otherwise ask (a `deny` default denies, an
    /// `allow` default still asks: `default` was not written with secrets in mind).
    pub fn sensitive_decision(&self, file: &ToolAction) -> PolicyAction {
        for rule in &self.rules {
            let exact = rule.tool == SENSITIVE_TOOL;
            if (rule.action == PolicyAction::Deny || exact) && rule_matches(rule, file) {
                return rule.action;
            }
        }
        if self.default == PolicyAction::Deny {
            PolicyAction::Deny
        } else {
            PolicyAction::Ask
        }
    }

    /// Decision for a `shell_unsandboxed` escape: the first deny rule that
    /// matches (normal matching, wildcards included) or the first allow/ask
    /// rule whose tool is exactly `shell_unsandboxed`, in order; otherwise ask
    /// (a `deny` default denies, an `allow` default still asks).
    pub fn escape_decision(&self, action: &ToolAction) -> PolicyAction {
        for rule in &self.rules {
            let exact = rule.tool == crate::tools::UNSANDBOXED_TOOL;
            if (rule.action == PolicyAction::Deny || exact) && rule_matches(rule, action) {
                return rule.action;
            }
        }
        if self.default == PolicyAction::Deny {
            PolicyAction::Deny
        } else {
            PolicyAction::Ask
        }
    }
}

/// Wrap the existing UI approval hook; automatic decisions are never cached by tool name.
/// Grants made through it last as long as the hook (one run).
pub fn configured_hook(
    mode: ApprovalMode,
    policy: ApprovalPolicy,
    interactive: Arc<dyn ApprovalHook>,
) -> Arc<dyn ApprovalHook> {
    configured_hook_with_grants(mode, policy, interactive, ChatGrants::in_memory())
}

/// [`configured_hook_for_project`] with nothing to look at on disk: names alone decide.
pub fn configured_hook_with_grants(
    mode: ApprovalMode,
    policy: ApprovalPolicy,
    interactive: Arc<dyn ApprovalHook>,
    grants: Arc<ChatGrants>,
) -> Arc<dyn ApprovalHook> {
    configured_hook_for_project(mode, policy, interactive, grants, None)
}

/// Like [`configured_hook`], with the chat's grants: stored grants are honored
/// where the policy asks, and an "allow in this chat" answer is saved to them.
/// `root` is the run's project, which relative paths in tool calls start from.
pub fn configured_hook_for_project(
    mode: ApprovalMode,
    policy: ApprovalPolicy,
    interactive: Arc<dyn ApprovalHook>,
    grants: Arc<ChatGrants>,
    root: Option<&std::path::Path>,
) -> Arc<dyn ApprovalHook> {
    configured_hook_with_reviewer(mode, policy, interactive, grants, root, None)
}

/// Like [`configured_hook_for_project`], with Auto mode's reviewer.
pub fn configured_hook_with_reviewer(
    mode: ApprovalMode,
    policy: ApprovalPolicy,
    interactive: Arc<dyn ApprovalHook>,
    grants: Arc<ChatGrants>,
    root: Option<&std::path::Path>,
    reviewer: Option<Arc<dyn crate::tools::AutoReviewer>>,
) -> Arc<dyn ApprovalHook> {
    let sensitive = policy.sensitive_matcher(root);
    Arc::new(ConfiguredHook {
        mode,
        policy,
        interactive,
        grants,
        sensitive,
        reviewer,
        soft_blocks: std::sync::Mutex::default(),
    })
}
