//! Shared file configuration and diagnostic access for desktop and CLI.
use super::*;
use serde_json::{json, Value};
use themis_core::configuration::{self, ApprovalMode, ResolvedConfig};

tokio::task_local! { pub static RUN_OVERRIDE: Option<String>; }

fn read_optional(path: &Path) -> Result<Option<String>, String> {
    match std::fs::read_to_string(path) {
        Ok(text) if text.len() <= 1024 * 1024 => Ok(Some(text)),
        Ok(_) => Err(format!("{} exceeds 1 MiB", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("cannot read {}: {error}", path.display())),
    }
}

fn read_project(path: &Path, root: &Path) -> Result<Option<String>, String> {
    if let Ok(metadata) = std::fs::symlink_metadata(path) {
        if metadata.file_type().is_symlink()
            || !path
                .canonicalize()
                .map_err(|e| e.to_string())?
                .starts_with(root)
        {
            return Err("project configuration must be a regular file inside the project".into());
        }
    }
    read_optional(path)
}

impl AppState {
    pub(crate) fn log_server_event(&self, event: &str) {
        let _ = self.inner.diagnostics.append(
            "server",
            event,
            "info",
            json!({"version":env!("CARGO_PKG_VERSION")}),
        );
    }
    pub(super) fn runtime_configuration(
        &self,
        project: Option<&Path>,
        explicit: Option<&str>,
    ) -> Result<ResolvedConfig, String> {
        let user_path = settings_app_dir(self.inner.settings.path()).join("runtime.jsonc");
        let user = read_optional(&user_path)?;
        let project_path = project
            .as_ref()
            .map(|root| root.join(".themis/config.jsonc"));
        let local = project_path
            .as_ref()
            .zip(project)
            .map(|(path, root)| read_project(path, root))
            .transpose()?
            .flatten();
        let mut resolved = configuration::resolve(user.as_deref(), local.as_deref(), explicit)
            .map_err(|e| format!("runtime configuration: {e:#}"))?;
        if resolved.config.compaction.prompt_file.is_some() {
            let origin = resolved
                .provenance
                .get("compaction.prompt_file")
                .map(String::as_str);
            let parent = if origin == Some("project") {
                project_path.as_ref().and_then(|path| path.parent())
            } else {
                user_path.parent()
            };
            configuration::load_prompt(
                &mut resolved.config,
                parent.ok_or("missing prompt configuration directory")?,
            )
            .map_err(|e| format!("runtime configuration: {e:#}"))?;
        }
        Ok(resolved)
    }

    /// The global `runtime.jsonc`, created with [`RUNTIME_CONFIG_STARTER`]
    /// when it does not exist yet. An existing file is never touched.
    pub fn ensure_runtime_config(&self) -> Result<PathBuf, String> {
        let path = settings_app_dir(self.inner.settings.path()).join("runtime.jsonc");
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("create '{}': {e}", parent.display()))?;
        }
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => {
                use std::io::Write;
                file.write_all(RUNTIME_CONFIG_STARTER.as_bytes())
                    .map_err(|e| format!("write '{}': {e}", path.display()))?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(format!("create '{}': {error}", path.display())),
        }
        Ok(path)
    }

    /// Opens the global `runtime.jsonc` (creating the starter if needed) as
    /// plain text in the system's text editor. It opens nothing else.
    pub fn open_runtime_config(&self) -> Result<(), String> {
        spawn_text_editor(&self.ensure_runtime_config()?)
    }

    pub fn get_runtime_configuration(&self, project_root: Option<String>) -> Result<Value, String> {
        let project = project_root
            .as_deref()
            .map(canonical_project_dir)
            .transpose()?;
        let resolved = self.runtime_configuration(project.as_deref(), None);
        let user_path = settings_app_dir(self.inner.settings.path()).join("runtime.jsonc");
        let project_path = project
            .as_ref()
            .map(|root| root.join(".themis/config.jsonc"));
        Ok(
            json!({"config":resolved.as_ref().ok().map(|r|&r.config),"provenance":resolved.as_ref().ok().map(|r|&r.provenance),"rule_sources":resolved.as_ref().ok().map(|r|&r.rule_sources),"validation_error":resolved.as_ref().err(),"user_path":user_path,"project_path":project_path,
            "user_json":read_optional(&user_path)?.unwrap_or_else(||"{}".into()),"project_json":project_path.as_ref().zip(project.as_ref()).map(|(p,root)|read_project(p,root)).transpose()?.flatten().unwrap_or_else(||"{}".into()),
            "schema":configuration::schema()}),
        )
    }

    pub fn save_runtime_configuration(
        &self,
        scope: &str,
        project_root: Option<String>,
        content: &str,
    ) -> Result<Value, String> {
        let project = project_root
            .as_deref()
            .map(canonical_project_dir)
            .transpose()?;
        let user_path = settings_app_dir(self.inner.settings.path()).join("runtime.jsonc");
        let project_path = project
            .as_ref()
            .map(|root| root.join(".themis/config.jsonc"));
        let (path, user, local) = match scope {
            "user" => (
                user_path.clone(),
                Some(content.to_owned()),
                project_path
                    .as_ref()
                    .zip(project.as_ref())
                    .map(|(p, root)| read_project(p, root))
                    .transpose()?
                    .flatten(),
            ),
            "project" => (
                project_path
                    .clone()
                    .ok_or("Choose a project before saving project configuration")?,
                read_optional(&user_path)?,
                Some(content.to_owned()),
            ),
            _ => return Err("configuration scope must be user or project".into()),
        };
        let mut resolved = configuration::resolve(user.as_deref(), local.as_deref(), None)
            .map_err(|e| format!("runtime configuration: {e:#}"))?;
        let parent = path.parent().ok_or("Invalid configuration path")?;
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        if let Some(root) = &project {
            if scope == "project"
                && !parent
                    .canonicalize()
                    .map_err(|e| e.to_string())?
                    .starts_with(root)
            {
                return Err("project configuration directory escapes the project".into());
            }
        }
        if std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err("configuration file cannot be a symlink".into());
        }
        if resolved.config.compaction.prompt_file.is_some() {
            let origin = resolved
                .provenance
                .get("compaction.prompt_file")
                .map(String::as_str);
            let directory = if origin == Some("project") {
                project_path.as_ref().and_then(|p| p.parent())
            } else {
                user_path.parent()
            };
            configuration::load_prompt(
                &mut resolved.config,
                directory.ok_or("missing prompt directory")?,
            )
            .map_err(|e| format!("runtime configuration: {e:#}"))?;
        }
        let temporary = parent.join(format!(".runtime-{}.tmp", uuid::Uuid::new_v4()));
        std::fs::write(&temporary, content).map_err(|e| e.to_string())?;
        if let Err(error) = std::fs::rename(&temporary, &path) {
            let _ = std::fs::remove_file(temporary);
            return Err(error.to_string());
        }
        if scope == "user" {
            self.inner.diagnostics.set_level(
                &configuration::resolve(user.as_deref(), None, None)
                    .map_err(|e| format!("runtime configuration: {e:#}"))?
                    .config
                    .logging
                    .level,
            );
        }
        let _ =
            self.inner
                .diagnostics
                .append("configuration", "saved", "info", json!({"scope":scope}));
        self.refresh_mcp_connections();
        self.get_runtime_configuration(project_root)
    }

    pub fn query_diagnostic_logs(
        &self,
        filter: themis_core::diagnostics::DiagnosticFilter,
    ) -> Result<Vec<themis_core::diagnostics::DiagnosticRecord>, String> {
        self.inner
            .diagnostics
            .query(&filter)
            .map_err(|e| e.to_string())
    }

    pub async fn set_thread_approval_mode(
        &self,
        thread_id: String,
        mode: ApprovalMode,
    ) -> Result<ThreadInfo, String> {
        let info = {
            let mut threads = self.inner.threads.write().await;
            let record = threads
                .get_mut(&thread_id)
                .ok_or_else(|| format!("unknown thread '{thread_id}'"))?;
            record.approval_mode = mode;
            thread_info(record, &self.inner.transcript)?
        };
        self.persist_registry().await;
        let _ = self.inner.diagnostics.append(
            "approval",
            "mode_changed",
            "info",
            json!({"thread_id":thread_id,"mode":mode}),
        );
        Ok(info)
    }
}

/// Written once when Settings opens a `runtime.jsonc` that does not exist:
/// every setting is commented out, so it changes nothing until edited.
pub const RUNTIME_CONFIG_STARTER: &str = r#"// Themis settings for every project. Changes apply from the next run.
// Remove the // in front of a line to use it. Reference: docs/configuration.md
// and docs/sandbox.md.
{
  // "approval": {
  //   "rules": [
  //     { "tool": "shell", "command": ["cargo", "test"], "action": "allow" },
  //     { "tool": "shell", "command": ["git", "push"], "action": "deny" }
  //   ]
  // },
  // "sandbox": {
  //   "enabled": true,
  //   "backend": "auto",
  //   "network": "allow",
  //   "writable_paths": []
  // }
}
"#;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn runtime_config_starter_changes_nothing_and_is_never_rewritten() {
        let temporary = tempfile::tempdir().unwrap();
        let state = AppState::new_for_test(temporary.path().join("settings.json"));
        let path = state.ensure_runtime_config().unwrap();
        assert_eq!(path, temporary.path().join("runtime.jsonc"));
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            RUNTIME_CONFIG_STARTER
        );
        // The starter is valid and every setting in it is commented out.
        let starter = state.runtime_configuration(None, None).unwrap();
        let defaults = configuration::resolve(None, None, None).unwrap();
        assert_eq!(
            serde_json::to_value(&starter.config).unwrap(),
            serde_json::to_value(&defaults.config).unwrap()
        );
        // An existing file, even an invalid one, is left exactly as it is.
        std::fs::write(&path, "{ my edits").unwrap();
        state.ensure_runtime_config().unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ my edits");
    }
    #[test]
    fn project_level_does_not_change_global_logger_when_user_saves() {
        let temporary = tempfile::tempdir().unwrap();
        let project = temporary.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        let state = AppState::new_for_test(temporary.path().join("settings.json"));
        let project_root = Some(project.to_string_lossy().into_owned());
        state
            .save_runtime_configuration(
                "project",
                project_root.clone(),
                r#"{"logging":{"level":"error"}}"#,
            )
            .unwrap();
        let result = state
            .save_runtime_configuration("user", project_root, r#"{"logging":{"level":"info"}}"#)
            .unwrap();
        assert_eq!(result["config"]["logging"]["level"], "error");
        state
            .inner
            .diagnostics
            .append("server", "user_level_proof", "info", json!({}))
            .unwrap();
        let records = state.query_diagnostic_logs(Default::default()).unwrap();
        assert!(records.iter().any(|r| r.event == "user_level_proof"));
    }
}
