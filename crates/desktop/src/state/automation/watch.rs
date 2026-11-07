//! Watch checks: running the check command, its policy, failures and backstops.

use super::*;

impl AppState {
    /// Policy is evaluated again before every execution, exactly like a
    /// `shell` action: a later deny rule, or a chat switched from YOLO to
    /// Custom, stops the check. The chat's saved grants count like an allow
    /// rule for non-destructive commands. The scheduler never prompts.
    pub(super) async fn run_watch_check(
        &self,
        automation: &Automation,
        argv: &[String],
    ) -> Result<CheckReport, CheckError> {
        let root = canonical_project_dir(&automation.project_root).map_err(CheckError::Failed)?;
        match self
            .check_command_policy(&root, automation.target_thread_id.as_deref(), argv)
            .await
            .map_err(CheckError::Failed)?
        {
            CheckPolicy::Deny => {
                return Err(CheckError::Blocked(
                    "check command denied by approval rules".to_owned(),
                ))
            }
            CheckPolicy::Ask if automation.watch_state.approved_check.as_deref() != Some(argv) => {
                return Err(CheckError::Blocked(
                    "check command needs approval".to_owned(),
                ))
            }
            _ => {}
        }
        // Checks run in the sandbox the current configuration describes; a
        // blocked check is never escalated (the scheduler asks nobody).
        let sandbox = self
            .runtime_configuration(Some(&root), None)
            .map(|resolved| self.sandbox_plan(&root, &resolved.config.sandbox, None))
            .map_err(CheckError::Failed)?;
        let out = themis_core::tools::run_check_command(
            argv.to_vec(),
            root,
            std::time::Duration::from_secs(WATCH_CHECK_TIMEOUT_SECS),
            Some(sandbox),
        )
        .await
        .map_err(|error| CheckError::Failed(error.to_string()))?;
        Ok(CheckReport {
            argv: argv.to_vec(),
            fingerprint: check_fingerprint(out.exit_code, &out.output),
            exit_code: out.exit_code,
            output: out.output,
        })
    }

    /// The approval policy's verdict on running `argv` as a `shell` action in
    /// this chat: YOLO allows, otherwise the resolved rules decide.
    pub async fn check_command_policy(
        &self,
        root: &Path,
        thread_id: Option<&str>,
        argv: &[String],
    ) -> Result<CheckPolicy, String> {
        if let Some(thread_id) = thread_id {
            let yolo = self
                .inner
                .threads
                .read()
                .await
                .get(thread_id)
                .is_some_and(|thread| thread.approval_mode == ApprovalMode::Yolo);
            if yolo {
                return Ok(CheckPolicy::Allow);
            }
        }
        let resolved = self.runtime_configuration(Some(root), None)?;
        let verdict = shell_policy_for(ApprovalMode::Custom, &resolved.config.approval, argv);
        // A rule that asks is answered by the chat's "Allow in this chat"
        // grants (never for destructive variants). Deny rules were final above.
        if verdict == CheckPolicy::Ask {
            if let Some(thread_id) = thread_id {
                if self.chat_grants_for(thread_id).covers(&check_action(argv)) {
                    return Ok(CheckPolicy::Allow);
                }
            }
        }
        Ok(verdict)
    }

    /// Pauses an automation whose check may no longer run, with the reason
    /// shown in Tasks. Re-enabling it re-evaluates the policy.
    pub(super) async fn pause_blocked_check(&self, automation_id: &str, reason: &str) {
        {
            let mut automations = self.inner.automations.write().await;
            let Some(automation) = automations.get_mut(automation_id) else {
                return;
            };
            automation.enabled = false;
            automation.watch_state.stopped_reason = Some(reason.to_owned());
        }
        self.persist_automations().await;
        let _ = self.inner.diagnostics.append(
            "automation",
            "automation_paused",
            "info",
            json!({"automation_id": automation_id, "reason": reason}),
        );
    }

    /// Records a check that started no model run: either unchanged output or a
    /// failure (could not run, timed out, or denied).
    pub(super) async fn record_watch_check(&self, automation_id: &str, failed: bool) {
        let now = Utc::now();
        {
            let mut automations = self.inner.automations.write().await;
            if let Some(automation) = automations.get_mut(automation_id) {
                automation.last_run_at = Some(now.to_rfc3339_opts(SecondsFormat::Secs, true));
                automation.run_count += 1;
                if failed {
                    automation.watch_state.consecutive_failures += 1;
                } else {
                    automation.watch_state.consecutive_failures = 0;
                }
                advance_automation_schedule(automation, now);
            }
        }
        self.persist_automations().await;
    }

    /// Removes or pauses automations whose limits ended: chat deleted,
    /// expired, run limit reached, or too many failures in a row. Each is
    /// logged with its reason.
    pub async fn enforce_watch_backstops(&self, now: DateTime<Utc>) {
        let snapshot: Vec<Automation> = self
            .inner
            .automations
            .read()
            .await
            .values()
            .cloned()
            .collect();
        if snapshot.is_empty() {
            return;
        }
        let threads: HashSet<String> = self.inner.threads.read().await.keys().cloned().collect();
        for automation in snapshot {
            let chat_gone = automation
                .target_thread_id
                .as_ref()
                .is_some_and(|id| !threads.contains(id));
            if chat_gone && automation.origin == AutomationOrigin::User {
                // A user's own automation is paused, never silently deleted.
                self.pause_with_reason(&automation.id, "chat deleted").await;
                continue;
            }
            let reason = if chat_gone {
                Some("chat deleted")
            } else if automation
                .watch
                .expires_at
                .as_deref()
                .and_then(|text| DateTime::parse_from_rfc3339(text).ok())
                .is_some_and(|at| at <= now)
            {
                Some("expired")
            } else if automation
                .watch
                .max_runs
                .is_some_and(|max| automation.run_count >= max)
            {
                Some("run limit")
            } else if automation.watch_state.consecutive_failures >= WATCH_FAILURE_LIMIT {
                if automation.origin == AutomationOrigin::User {
                    self.pause_after_failures(&automation.id).await;
                    continue;
                }
                Some("failed 3 times")
            } else {
                None
            };
            if let Some(reason) = reason {
                let _ = self.remove_automation(&automation.id, reason).await;
            }
        }
    }

    pub(super) async fn pause_after_failures(&self, automation_id: &str) {
        self.pause_with_reason(automation_id, "failed 3 times")
            .await;
    }

    /// Pauses a still-enabled automation and records why (shown in Tasks).
    pub(super) async fn pause_with_reason(&self, automation_id: &str, reason: &str) {
        {
            let mut automations = self.inner.automations.write().await;
            let Some(automation) = automations.get_mut(automation_id) else {
                return;
            };
            if !automation.enabled {
                return;
            }
            automation.enabled = false;
            automation.watch_state.stopped_reason = Some(reason.to_owned());
        }
        self.persist_automations().await;
        let _ = self.inner.diagnostics.append(
            "automation",
            "automation_paused",
            "info",
            json!({"automation_id": automation_id, "reason": reason}),
        );
    }
}
