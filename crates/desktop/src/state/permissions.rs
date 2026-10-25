//! Sandbox plans for runs and the read-only effective-permissions view that
//! the desktop Settings → Permissions tab and `themis permissions` share.

use super::*;
use serde_json::{json, Value};
use themis_core::configuration::{self, ApprovalMode};
use themis_core::sandbox::{PlanInputs, SandboxConfig, SandboxPlan};

impl AppState {
    /// Test seam: the home folder sandbox plans use for `~/` paths and the
    /// secret list (`None` uses `$HOME`).
    pub fn set_sandbox_home_override(&self, home: Option<PathBuf>) {
        *self
            .inner
            .sandbox_home
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = home;
    }

    /// The app-data folder (beside `settings.json`).
    pub(crate) fn app_data_dir(&self) -> PathBuf {
        settings_app_dir(self.inner.settings.path())
    }

    /// The sandbox commands of a run (or a watch check) in `root` use under
    /// `config`, protecting this app's data folder. `open` (YOLO) leaves only the
    /// protected folders read-only.
    pub(crate) fn sandbox_plan(
        &self,
        root: &Path,
        config: &SandboxConfig,
        open: bool,
    ) -> Arc<SandboxPlan> {
        let home = self
            .inner
            .sandbox_home
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        let app_data = self.app_data_dir();
        let inputs = PlanInputs {
            config,
            project_root: root,
            app_data: Some(&app_data),
            home: home.as_deref(),
            open,
        };
        let unavailable = self
            .inner
            .sandbox_backends_unavailable
            .load(std::sync::atomic::Ordering::SeqCst);
        Arc::new(if unavailable {
            SandboxPlan::with_availability(inputs, |_| {
                themis_core::sandbox::Availability::no("simulated as missing for this test")
            })
        } else {
            SandboxPlan::new(inputs)
        })
    }

    /// Test seam: treat every sandbox backend as missing on this machine.
    pub fn set_sandbox_backends_unavailable(&self, unavailable: bool) {
        self.inner
            .sandbox_backends_unavailable
            .store(unavailable, std::sync::atomic::Ordering::SeqCst);
    }

    /// Read-only view of the permissions that apply to runs in `project_root`
    /// (and, with `thread_id`, that chat): approval mode, ordered rules with
    /// their source, sandbox status, configuration files with problems, and
    /// the chat's saved grants. Re-read on every call.
    pub async fn get_effective_permissions(
        &self,
        project_root: Option<String>,
        thread_id: Option<String>,
    ) -> Result<Value, String> {
        let thread = match &thread_id {
            Some(id) => self.inner.threads.read().await.get(id).map(|record| {
                (
                    record.approval_mode,
                    record.project_root.to_string_lossy().into_owned(),
                )
            }),
            None => None,
        };
        let project_root = project_root.or_else(|| thread.as_ref().map(|(_, root)| root.clone()));
        let project = project_root
            .as_deref()
            .map(canonical_project_dir)
            .transpose()?;
        let user_path = self.app_data_dir().join("runtime.jsonc");
        let project_path = project
            .as_ref()
            .map(|root| root.join(".themis/config.jsonc"));
        let user_text = std::fs::read_to_string(&user_path).ok();
        let project_text = project_path
            .as_ref()
            .and_then(|path| std::fs::read_to_string(path).ok());
        let user_alone = user_text
            .as_deref()
            .map(|text| configuration::resolve(Some(text), None, None).map(|_| ()));
        let mut files = vec![file_entry(
            "user",
            &user_path,
            user_text.as_deref(),
            user_alone.as_ref().and_then(|result| result.as_ref().err()),
        )];
        let resolved = self.runtime_configuration(project.as_deref(), None);
        if let Some(path) = &project_path {
            let project_error = match (&resolved, &user_alone) {
                (Err(_), Some(Err(_))) => None,
                (Err(error), _) => Some(anyhow::anyhow!(error.clone())),
                _ => None,
            };
            files.push(file_entry(
                "project",
                path,
                project_text.as_deref(),
                project_error.as_ref(),
            ));
        }
        let (approval, sandbox) = match &resolved {
            Ok(resolved) => {
                let rules: Vec<Value> = resolved
                    .config
                    .approval
                    .rules
                    .iter()
                    .zip(
                        resolved
                            .rule_sources
                            .iter()
                            .chain(std::iter::repeat(&"default".to_owned())),
                    )
                    .map(|(rule, source)| {
                        json!({
                            "tool": rule.tool,
                            "action": rule.action,
                            "risk": rule.risk,
                            "command": rule.command,
                            "source": source,
                        })
                    })
                    .collect();
                let project_rules = resolved
                    .rule_sources
                    .iter()
                    .filter(|source| *source == "project")
                    .count();
                let default_source = resolved
                    .provenance
                    .get("approval.default")
                    .cloned()
                    .unwrap_or_else(|| "default".into());
                let placeholder = PathBuf::from("<project>");
                let plan_root = project.as_deref().unwrap_or(&placeholder);
                let mut status = serde_json::to_value(
                    // A YOLO chat's commands use the open plan.
                    self.sandbox_plan(
                        plan_root,
                        &resolved.config.sandbox,
                        thread
                            .as_ref()
                            .is_some_and(|(mode, _)| *mode == ApprovalMode::Yolo),
                    )
                    .status(),
                )
                .map_err(|e| e.to_string())?;
                status["sources"] = json!({
                    "enabled": resolved.provenance.get("sandbox.enabled"),
                    "backend": resolved.provenance.get("sandbox.backend"),
                    "network": resolved.provenance.get("sandbox.network"),
                    "writable_paths": resolved.provenance.get("sandbox.writable_paths"),
                });
                (
                    json!({
                        "default": resolved.config.approval.default,
                        "default_source": default_source,
                        "rules": rules,
                        "project_rule_count": project_rules,
                    }),
                    status,
                )
            }
            Err(_) => (Value::Null, Value::Null),
        };
        let chat = match &thread_id {
            Some(id) if thread.is_some() => {
                let path = self.chat_grants_path(id);
                let grants = path
                    .as_ref()
                    .map(|path| themis_core::chat_grants::ChatGrants::load(path));
                json!({
                    "thread_id": id,
                    "grants_path": path,
                    "grants": grants.as_ref().map(|grants| grants.entries()).unwrap_or_default(),
                    "warning": grants.as_ref().and_then(|grants| grants.warning()),
                })
            }
            _ => Value::Null,
        };
        Ok(json!({
            "project_root": project,
            "mode": thread.as_ref().map(|(mode, _)| *mode),
            "default_mode": ApprovalMode::default(),
            "validation_error": resolved.as_ref().err(),
            "files": files,
            "approval": approval,
            "sandbox": sandbox,
            "chat": chat,
        }))
    }
}

/// One configuration file: where it is, whether it loaded, and its first
/// problem with a line number when one can be found.
fn file_entry(
    scope: &str,
    path: &Path,
    text: Option<&str>,
    semantic: Option<&anyhow::Error>,
) -> Value {
    let Some(text) = text else {
        let missing = !path.exists();
        return json!({
            "scope": scope,
            "path": path,
            "status": if missing { "missing" } else { "unreadable" },
            "error": if missing { Value::Null } else { json!({"message": "cannot read this file"}) },
        });
    };
    let issue = configuration::check_file(text).or_else(|| {
        semantic.map(|error| {
            let message = format!("{error:#}");
            configuration::FileIssue {
                line: configuration::locate_error_line(text, &message),
                message,
                column: None,
            }
        })
    });
    json!({
        "scope": scope,
        "path": path,
        "status": if issue.is_some() { "invalid" } else { "loaded" },
        "error": issue,
    })
}
