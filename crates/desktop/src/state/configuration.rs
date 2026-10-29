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
    /// The latest sample of Themis's own CPU and memory use, by group.
    pub fn resource_usage(&self) -> Option<crate::resources::Snapshot> {
        self.inner.resources.snapshot()
    }

    /// Takes one resource sample (blocking) and logs what changed.
    pub(crate) fn sample_resources(&self) {
        self.inner.resources.sample();
        self.inner.resources.log(&self.inner.diagnostics);
    }

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

    /// The global `runtime.jsonc`, created with [`runtime_config_starter`] when it does not
    /// exist yet (or still sets nothing), and the reference beside it. A file with any
    /// setting in it is never touched.
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
                file.write_all(runtime_config_starter().as_bytes())
                    .map_err(|e| format!("write '{}': {e}", path.display()))?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                // The old long starter, still all comments, becomes the current one; any
                // other file (a setting or only someone's own notes in it) is never touched.
                if let Ok(text) = std::fs::read_to_string(&path) {
                    if text.starts_with(OLD_STARTER_HEADER) && sets_nothing(&text) {
                        // Kept once beside it, in case a commented line held a note.
                        let _ = std::fs::write(path.with_file_name(PREVIOUS_STARTER), &text);
                        std::fs::write(&path, runtime_config_starter())
                            .map_err(|e| format!("write '{}': {e}", path.display()))?;
                    }
                }
            }
            Err(error) => return Err(format!("create '{}': {error}", path.display())),
        }
        // The reference is rewritten every time, so it always matches this version.
        if let Some(parent) = path.parent() {
            let reference = parent.join(REFERENCE_FILE);
            if std::fs::read_to_string(&reference).ok().as_deref() != Some(configuration::REFERENCE)
            {
                std::fs::write(&reference, configuration::REFERENCE)
                    .map_err(|e| format!("write '{}': {e}", reference.display()))?;
            }
        }
        Ok(path)
    }

    /// Opens the global `runtime.jsonc` (creating the starter if needed) as plain text in
    /// `editor`, or the one chosen in Settings (the system text editor by default). It opens
    /// nothing else.
    pub async fn open_runtime_config(&self, editor: Option<String>) -> Result<(), String> {
        let editor = match editor {
            Some(editor) => editor,
            None => self.get_settings().await.config_editor,
        };
        super::editors::open_with(&editor, &self.ensure_runtime_config()?)
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

    /// The person's own browser settings: the user `runtime.jsonc` over the defaults, without
    /// any project's restrictions (what Settings › Browser shows and edits).
    pub fn user_browser_config(&self) -> Result<configuration::BrowserConfig, String> {
        let user_path = settings_app_dir(self.inner.settings.path()).join("runtime.jsonc");
        let user = read_optional(&user_path)?;
        configuration::resolve(user.as_deref(), None, None)
            .map(|resolved| resolved.config.browser)
            .map_err(|e| format!("runtime configuration: {e:#}"))
    }

    /// The browser settings with the revision a later edit must name (Settings › Browser).
    pub fn user_browser_view(&self) -> Result<(configuration::BrowserConfig, String), String> {
        let browser = self.user_browser_config()?;
        let revision = browser_revision(&browser);
        Ok((browser, revision))
    }

    /// Saves only the `changes` (top-level browser fields) on top of what the file holds now.
    /// With `revision`, refuses when the file's browser settings changed since that revision
    /// (an edit in the file or another window), so a stale page never overwrites them.
    pub fn patch_user_browser_config(
        &self,
        changes: serde_json::Map<String, Value>,
        revision: Option<&str>,
    ) -> Result<(configuration::BrowserConfig, String), String> {
        let saved = self.update_user_browser_config(|browser| {
            if revision.is_some_and(|expected| expected != browser_revision(browser)) {
                return Err(STALE_BROWSER_SETTINGS.to_owned());
            }
            let mut value = serde_json::to_value(&*browser).map_err(|e| e.to_string())?;
            let object = value
                .as_object_mut()
                .ok_or("browser settings are not an object")?;
            for (key, field) in changes {
                object.insert(key, field);
            }
            *browser = serde_json::from_value(value)
                .map_err(|e| format!("runtime configuration: browser: {e}"))?;
            Ok(())
        })?;
        let revision = browser_revision(&saved);
        Ok((saved, revision))
    }

    /// Applies `change` to the `browser` settings in the user `runtime.jsonc` and saves
    /// them, leaving the rest of the file, comments included, as written. Later runs pick
    /// them up; a run in progress keeps its own copy.
    pub fn update_user_browser_config(
        &self,
        change: impl FnOnce(&mut configuration::BrowserConfig) -> Result<(), String>,
    ) -> Result<configuration::BrowserConfig, String> {
        self.update_user_block("browser", |config| {
            let mut browser = config.browser.clone();
            change(&mut browser)?;
            browser.site_restrictions.clear();
            serde_json::to_value(&browser).map_err(|e| e.to_string())
        })
        .map(|config| config.browser)
    }

    /// Applies `change` to the `web` settings in the user `runtime.jsonc`, in place.
    pub fn update_user_web_config(
        &self,
        change: impl FnOnce(&mut configuration::WebConfig) -> Result<(), String>,
    ) -> Result<configuration::WebConfig, String> {
        self.update_user_block("web", |config| {
            let mut web = config.web.clone();
            change(&mut web)?;
            serde_json::to_value(&web).map_err(|e| e.to_string())
        })
        .map(|config| config.web)
    }

    /// Replaces the top-level `key` of the user `runtime.jsonc` with what `block` builds from
    /// the file's current settings, keeping the rest of the file and its comments. The result
    /// must validate, and the file is replaced atomically; writes are serialized.
    fn update_user_block(
        &self,
        key: &str,
        block: impl FnOnce(&configuration::RuntimeConfig) -> Result<Value, String>,
    ) -> Result<configuration::RuntimeConfig, String> {
        static WRITING: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _writing = WRITING.lock().unwrap_or_else(|e| e.into_inner());
        let path = self.ensure_runtime_config()?;
        if std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err("configuration file cannot be a symlink".into());
        }
        let content = read_optional(&path)?.unwrap_or_default();
        let current = configuration::resolve(Some(&content), None, None)
            .map_err(|e| format!("runtime configuration: {e:#}"))?
            .config;
        let value = block(&current)?;
        let updated = configuration::set_top_level(&content, key, &value)
            .map_err(|e| format!("runtime configuration: {e:#}"))?;
        let saved = configuration::resolve(Some(&updated), None, None)
            .map_err(|e| format!("runtime configuration: {e:#}"))?
            .config;
        let parent = path.parent().ok_or("Invalid configuration path")?;
        let temporary = parent.join(format!(".runtime-{}.tmp", uuid::Uuid::new_v4()));
        std::fs::write(&temporary, &updated).map_err(|e| e.to_string())?;
        if let Err(error) = std::fs::rename(&temporary, &path) {
            let _ = std::fs::remove_file(temporary);
            return Err(error.to_string());
        }
        let _ = self.inner.diagnostics.append(
            "configuration",
            "saved",
            "info",
            json!({"scope":"user","section":key}),
        );
        Ok(saved)
    }

    /// The person's subagent settings with the revision a later edit must name
    /// (Settings › Subagents).
    pub fn user_subagents_view(&self) -> Result<(configuration::SubagentsConfig, String), String> {
        let user_path = settings_app_dir(self.inner.settings.path()).join("runtime.jsonc");
        let user = read_optional(&user_path)?;
        let subagents = configuration::resolve(user.as_deref(), None, None)
            .map(|resolved| resolved.config.subagents)
            .map_err(|e| format!("runtime configuration: {e:#}"))?;
        let revision = revision_of(&subagents);
        Ok((subagents, revision))
    }

    /// Saves only the `changes` (Settings › Subagents fields) on top of the file's
    /// `subagents` block, refusing a stale `revision` like Settings › Browser.
    pub fn patch_user_subagents_config(
        &self,
        changes: serde_json::Map<String, Value>,
        revision: Option<&str>,
    ) -> Result<(configuration::SubagentsConfig, String), String> {
        const EDITABLE: [&str; 3] = ["enabled", "max_parallel", "model"];
        if let Some(key) = changes.keys().find(|key| !EDITABLE.contains(&key.as_str())) {
            return Err(format!("Settings › Subagents cannot change `{key}`"));
        }
        let saved = self.update_user_block("subagents", |config| {
            if revision.is_some_and(|expected| expected != revision_of(&config.subagents)) {
                return Err(STALE_SUBAGENT_SETTINGS.to_owned());
            }
            let mut value = serde_json::to_value(&config.subagents).map_err(|e| e.to_string())?;
            let object = value
                .as_object_mut()
                .ok_or("subagent settings are not an object")?;
            for (key, field) in changes {
                if field.is_null() {
                    object.remove(&key);
                } else {
                    object.insert(key, field);
                }
            }
            Ok(value)
        })?;
        let revision = revision_of(&saved.subagents);
        Ok((saved.subagents, revision))
    }

    /// The `web` settings from the user `runtime.jsonc` (Settings › Web).
    pub fn user_web_config(&self) -> Result<configuration::WebConfig, String> {
        let user_path = settings_app_dir(self.inner.settings.path()).join("runtime.jsonc");
        let user = read_optional(&user_path)?;
        configuration::resolve(user.as_deref(), None, None)
            .map(|resolved| resolved.config.web)
            .map_err(|e| format!("runtime configuration: {e:#}"))
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

/// The settings a new `runtime.jsonc` offers, each one line and commented out, by section:
/// `(section, [(what it does, the setting)])`. Every option and its meaning is in the
/// reference written beside it ([`REFERENCE_FILE`]).
pub const RUNTIME_CONFIG_PRESETS: [(&str, &[(&str, &str)]); 6] = [
    (
        "Approvals (Custom mode); use one line",
        &[
            (
                "Reads run, everything else asks (the default)",
                r#""approval": { "default": "ask", "rules": [{ "tool": "*", "risk": "read", "action": "allow" }] },"#,
            ),
            (
                "Also run tests without asking, never push",
                r#""approval": { "default": "ask", "rules": [{ "tool": "*", "risk": "read", "action": "allow" }, { "tool": "shell", "command": ["cargo", "test"], "action": "allow" }, { "tool": "shell", "command": ["npm", "test"], "action": "allow" }, { "tool": "shell", "command": ["git", "push"], "action": "deny" }] },"#,
            ),
            (
                "Read-only: refuse anything that writes, runs or browses",
                r#""approval": { "default": "deny", "rules": [{ "tool": "*", "risk": "read", "action": "allow" }] },"#,
            ),
        ],
    ),
    (
        "Sandbox",
        &[
            (
                "Commands get no network",
                r#""sandbox": { "network": "block" },"#,
            ),
            (
                "Commands may also write here",
                r#""sandbox": { "writable_paths": ["~/.cargo"] },"#,
            ),
        ],
    ),
    (
        "Browser and web",
        &[
            ("No built-in browser", r#""browser": { "enabled": false },"#),
            (
                "Ask before the agent opens a site",
                r#""browser": { "permissions": { "browsing": "ask", "downloads": "ask", "debugging": "block" } },"#,
            ),
            (
                "Use your own SearXNG for web search",
                r#""web": { "search_url": "http://127.0.0.1:8888" },"#,
            ),
        ],
    ),
    (
        "Working memory and steps (instead of Settings › Advanced)",
        &[
            (
                "Working memory in tokens; under 200,000 is for evals",
                r#""context_token_budget": 300000,"#,
            ),
            ("Steps per reply", r#""total_turns": 400,"#),
        ],
    ),
    (
        "Models",
        &[
            (
                "Check goal turns with a cheaper model",
                r#""goal": { "check_model": "glm-5.3" },"#,
            ),
            (
                "Write checkpoint summaries with another model",
                r#""compaction": { "summary_model": "glm-5.3" },"#,
            ),
        ],
    ),
    (
        "Logs",
        &[(
            "More detail in the logs",
            r#""logging": { "level": "debug" },"#,
        )],
    ),
];

/// The full reference Themis writes beside `runtime.jsonc` and never reads.
pub const REFERENCE_FILE: &str = "runtime.reference.jsonc";

/// The first line of the long starter Themis wrote before; only such a file is upgraded.
const OLD_STARTER_HEADER: &str =
    "// Themis settings for every project (JSON with // comments and trailing commas).";

/// Where an older starter that set nothing is kept when the current one replaces it.
pub const PREVIOUS_STARTER: &str = "runtime.previous.jsonc";

/// A new `runtime.jsonc`: one-line settings, each commented out under a note, so the file
/// changes nothing until a line is uncommented.
pub fn runtime_config_starter() -> String {
    let mut text = String::from(
        "// Your Themis settings, for every project.\n\
         //\n\
         // Lines starting with // \" are settings: delete the // to use one, then edit its value.\n\
         // Every option and what it does: runtime.reference.jsonc, next to this file.\n\
         // Changes apply from each chat's next message. Settings › Permissions shows problems.\n\
         {\n  \"schema_version\": 1,\n",
    );
    for (section, settings) in RUNTIME_CONFIG_PRESETS {
        text.push_str(&format!("\n  // ── {section}\n"));
        for (note, setting) in settings.iter() {
            text.push_str(&format!("  //   {note}\n  // {setting}\n"));
        }
    }
    text.push_str("}\n");
    text
}

/// Whether `text` sets nothing: only comments, blank lines, braces and the schema version,
/// as a starter written by any version is. Such a file can be replaced without losing a
/// setting.
fn sets_nothing(text: &str) -> bool {
    text.lines().map(str::trim).all(|line| {
        line.is_empty()
            || line.starts_with("//")
            || line == "{"
            || line == "}"
            || line.trim_end_matches(',') == "\"schema_version\": 1"
    })
}

/// Refusal when the file's browser settings changed under an open Settings page.
pub(crate) const STALE_BROWSER_SETTINGS: &str =
    "Browser settings changed in runtime.jsonc since this page loaded; showing the latest";

/// A short, stable fingerprint of browser settings as resolved from the file.
pub(crate) const STALE_SUBAGENT_SETTINGS: &str =
    "Subagent settings changed in runtime.jsonc since this page loaded; showing the latest";

fn browser_revision(browser: &configuration::BrowserConfig) -> String {
    revision_of(browser)
}

fn revision_of(settings: &impl serde::Serialize) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    serde_json::to_string(settings)
        .unwrap_or_default()
        .hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

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
            runtime_config_starter()
        );
        // The full reference sits beside it, as this version knows it.
        assert_eq!(
            std::fs::read_to_string(temporary.path().join(REFERENCE_FILE)).unwrap(),
            configuration::REFERENCE
        );
        // The starter is valid and sets nothing.
        let starter = state.runtime_configuration(None, None).unwrap();
        let defaults = configuration::resolve(None, None, None).unwrap();
        assert_eq!(
            serde_json::to_value(&starter.config).unwrap(),
            serde_json::to_value(&defaults.config).unwrap()
        );
        // Each one-line setting, uncommented on its own, is valid; settings start with // "
        // and notes never do.
        let text = runtime_config_starter();
        for (section, settings) in RUNTIME_CONFIG_PRESETS {
            for (note, setting) in settings.iter() {
                assert!(!setting.contains('\n'), "'{note}' is more than one line");
                assert!(
                    text.contains(&format!("  // {setting}\n")),
                    "'{note}' is missing"
                );
                let file = format!("{{{}}}", setting.trim_end_matches(','));
                configuration::resolve(Some(&file), None, None)
                    .unwrap_or_else(|e| panic!("{section}: '{note}' is invalid: {e:#}"));
            }
        }
        for line in text
            .lines()
            .map(str::trim)
            .filter(|line| line.starts_with("//"))
        {
            let comment = line.trim_start_matches("//").trim_start();
            let is_setting = comment.starts_with('"');
            assert!(
                is_setting || !comment.contains("\": "),
                "a note reads like a setting: {line}"
            );
        }
        // Settings › Browser and Web still write their blocks into it, notes kept.
        state
            .update_user_browser_config(|browser| {
                browser.enabled = false;
                Ok(())
            })
            .unwrap();
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(written.contains("// ── Sandbox"));
        assert!(
            !state
                .runtime_configuration(None, None)
                .unwrap()
                .config
                .browser
                .enabled
        );
        // A file with a setting in it is left exactly as it is, even an invalid one.
        state.ensure_runtime_config().unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), written);
        std::fs::write(&path, "{ my edits").unwrap();
        state.ensure_runtime_config().unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ my edits");
        // An older starter nobody changed (all comments) becomes the current one.
        std::fs::write(
            &path,
            format!("{OLD_STARTER_HEADER}\n{{\n  // \"approval\": {{\n    // \"default\": \"ask\",\n  // }},\n}}\n"),
        )
        .unwrap();
        state.ensure_runtime_config().unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            runtime_config_starter()
        );
        assert!(
            std::fs::read_to_string(temporary.path().join(PREVIOUS_STARTER))
                .unwrap()
                .starts_with(OLD_STARTER_HEADER)
        );
        // A file of someone's own notes, setting nothing, stays as it is.
        let notes = "{\n  // my notes\n  \"schema_version\": 1\n}\n";
        std::fs::write(&path, notes).unwrap();
        state.ensure_runtime_config().unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), notes);
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
