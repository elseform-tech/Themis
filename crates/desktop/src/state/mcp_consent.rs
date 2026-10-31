//! Consent for starting MCP connections.
//!
//! Enabling a connection is the consent to start it, but only when a person
//! enabled that exact definition. This module records, in `mcp_consent.json`
//! beside the plugin registry, a fingerprint of the definition (command, args,
//! url, env references, bearer variable and the plugin's resource files, which
//! `node server.js`-style commands run) that a person approved: directly in the
//! UI or CLI, or by answering the approval dialog for an agent's change. A
//! definition that is new, changed by anything else (an unapproved agent edit, a
//! catalog update, a repository's `.themis/plugins/`), applied without a person's
//! answer (YOLO or an allow rule), or missing from the file does not start until
//! a person enables it again. The file has no integrity protection, like the
//! plugin registry beside it.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use themis_core::plugins::{connections::McpServer, Plugin};

use super::AppState;

const VERSION: u32 = 1;
const MAX_ENTRIES: usize = 4096;
/// Resource files hashed into a fingerprint; more than this fails closed.
const MAX_FILES: usize = 256;
const MAX_FILE_BYTES: usize = 8 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConsentFile {
    version: u32,
    #[serde(default)]
    approved: BTreeMap<String, String>,
}

impl Default for ConsentFile {
    fn default() -> Self {
        Self {
            version: VERSION,
            approved: BTreeMap::new(),
        }
    }
}

/// What a person approved: the launch definition plus the plugin's resource
/// files. `None` when the resources exceed the bound (fail closed).
pub(super) fn definition_digest(
    server: &McpServer,
    files: &BTreeMap<String, String>,
    executable: &[String],
) -> Option<String> {
    if files.len() > MAX_FILES || files.values().map(String::len).sum::<usize>() > MAX_FILE_BYTES {
        return None;
    }
    let mut hasher = Sha256::new();
    let definition = json!([
        server.command,
        server.args,
        server.url,
        server.env,
        server.bearer_env,
        server.cwd
    ]);
    hasher.update(definition.to_string().as_bytes());
    for (path, content) in files {
        hasher.update([0]);
        hasher.update(path.as_bytes());
        hasher.update([0]);
        hasher.update(Sha256::digest(content.as_bytes()));
        hasher.update([u8::from(executable.contains(path))]);
    }
    Some(
        hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect(),
    )
}

pub(super) fn plugin_digest(plugin: &Plugin, server: &McpServer) -> Option<String> {
    definition_digest(
        server,
        &plugin.spec.approved_files(),
        &plugin.spec.executable_files,
    )
}

/// Namespace for a connection's generated tool names. It includes the approved
/// definition, so chat grants saved for one definition never cover another.
pub(super) fn mcp_namespace(plugin: &Plugin, name: &str, server: &McpServer) -> String {
    format!(
        "{}#{}",
        plugin.skill_id(name),
        plugin_digest(plugin, server).unwrap_or_default()
    )
}

fn consent_key(root: &Path, scope: &str, plugin: &str, server: &str) -> String {
    // A project's definitions are consented per canonical project root (a
    // worktree run of the same project sees the same consent); global ones everywhere.
    let place = if scope == "local" {
        root.canonicalize()
            .unwrap_or_else(|_| root.to_owned())
            .to_string_lossy()
            .into_owned()
    } else {
        String::new()
    };
    format!("{scope}\0{place}\0{plugin}\0{server}")
}

fn root_option(root: &Path) -> Option<PathBuf> {
    (!root.as_os_str().is_empty()).then(|| root.to_owned())
}

impl AppState {
    fn consent_path(&self) -> PathBuf {
        self.inner
            .skills_path
            .parent()
            .unwrap_or(Path::new("."))
            .join("mcp_consent.json")
    }

    /// Reads the consent file. A missing file is seeded once with the global
    /// definitions already enabled (they live in app data, not in a repository);
    /// project definitions are never seeded. A damaged file approves nothing and
    /// is noted in diagnostics; the next approval replaces it.
    fn load_consent(&self) -> ConsentFile {
        let path = self.consent_path();
        match std::fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str::<ConsentFile>(&text) {
                Ok(file) if file.version == VERSION && file.approved.len() <= MAX_ENTRIES => file,
                _ => self.consent_ignored("invalid consent file"),
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let mut file = ConsentFile::default();
                if let Ok(plugins) = self.plugin_store(None).list() {
                    for plugin in plugins.iter().filter(|plugin| plugin.scope == "global") {
                        for (name, server) in plugin.spec.mcp.iter().filter(|(_, s)| s.enabled) {
                            if let Some(digest) = plugin_digest(plugin, server) {
                                file.approved.insert(
                                    consent_key(Path::new(""), "global", &plugin.spec.name, name),
                                    digest,
                                );
                            }
                        }
                    }
                }
                self.save_consent(&file);
                file
            }
            Err(_) => self.consent_ignored("unreadable consent file"),
        }
    }

    fn consent_ignored(&self, reason: &str) -> ConsentFile {
        let _ = self.inner.diagnostics.append(
            "approval",
            "mcp_consent_ignored",
            "warn",
            json!({"reason": reason}),
        );
        ConsentFile::default()
    }

    fn save_consent(&self, file: &ConsentFile) {
        if let Ok(text) = serde_json::to_string_pretty(file) {
            let _ = themis_core::chat_grants::write_atomic(&self.consent_path(), &text);
        }
    }

    /// Whether a person approved exactly this definition for starting.
    pub(super) fn mcp_consented(
        &self,
        root: &Path,
        plugin: &Plugin,
        name: &str,
        server: &McpServer,
    ) -> bool {
        let _guard = themis_core::chat_grants::WRITE_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let Some(digest) = plugin_digest(plugin, server) else {
            return false;
        };
        self.load_consent()
            .approved
            .get(&consent_key(root, &plugin.scope, &plugin.spec.name, name))
            .is_some_and(|approved| *approved == digest)
    }

    /// Whether stored consent already covers `server` of a plugin about to change.
    pub(super) fn mcp_definition_consented(
        &self,
        root: &Path,
        scope: &str,
        plugin: &str,
        name: &str,
        digest: Option<&str>,
    ) -> bool {
        let _guard = themis_core::chat_grants::WRITE_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        digest.is_some_and(|digest| {
            self.load_consent()
                .approved
                .get(&consent_key(root, scope, plugin, name))
                .is_some_and(|approved| approved == digest)
        })
    }

    /// Records approval of the named servers. Each carries the digest of what
    /// the dialog showed (`Some`); consent is stored only while the stored
    /// definition still has exactly that digest, so a concurrent change made
    /// after the person answered is never approved by that answer. Direct user
    /// actions pass `None`: the person reviewed the stored definition itself.
    pub(super) fn record_mcp_consent(
        &self,
        root: Option<&Path>,
        scope: &str,
        plugin_name: &str,
        servers: &[(String, Option<String>)],
    ) {
        let root = root.unwrap_or(Path::new(""));
        let Ok(plugins) = self.plugin_store(root_option(root)).list() else {
            return;
        };
        let Some(plugin) = plugins
            .iter()
            .find(|plugin| plugin.scope == scope && plugin.spec.name == plugin_name)
        else {
            return;
        };
        let _guard = themis_core::chat_grants::WRITE_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut file = self.load_consent();
        for (name, shown) in servers {
            let Some(server) = plugin.spec.mcp.get(name).filter(|s| s.enabled) else {
                continue;
            };
            let Some(current) = plugin_digest(plugin, server) else {
                continue;
            };
            if shown.as_ref().is_some_and(|shown| *shown != current) {
                let _ = self.inner.diagnostics.append(
                    "approval",
                    "mcp_consent_skipped",
                    "warn",
                    json!({"reason": "definition changed after approval"}),
                );
                continue;
            }
            file.approved
                .insert(consent_key(root, scope, plugin_name, name), current);
        }
        if file.approved.len() <= MAX_ENTRIES {
            self.save_consent(&file);
        }
    }

    /// Consent for the enabled servers of a plugin a person just saved or enabled.
    pub(super) fn record_user_mcp_consent(
        &self,
        root: Option<&Path>,
        scope: &str,
        plugin_name: &str,
        only: Option<&str>,
    ) {
        let servers: Vec<(String, Option<String>)> = self
            .plugin_store(root.map(Path::to_owned))
            .list()
            .ok()
            .and_then(|plugins| {
                plugins
                    .into_iter()
                    .find(|plugin| plugin.scope == scope && plugin.spec.name == plugin_name)
            })
            .map(|plugin| {
                plugin
                    .spec
                    .mcp
                    .keys()
                    .filter(|name| only.is_none_or(|only| only == name.as_str()))
                    .map(|name| (name.clone(), None))
                    .collect()
            })
            .unwrap_or_default();
        self.record_mcp_consent(root, scope, plugin_name, &servers);
    }

    /// One async lock per plugin: agent management calls on the same plugin run
    /// one at a time, so the definition a dialog shows is the one that is saved.
    pub(super) fn plugin_call_lock(
        &self,
        scope: &str,
        plugin: &str,
    ) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks: std::sync::MutexGuard<HashMap<String, Arc<tokio::sync::Mutex<()>>>> = self
            .inner
            .plugin_locks
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        locks
            .entry(format!("{scope}\0{plugin}"))
            .or_default()
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use themis_core::plugins::{PluginSpec, PluginStore};

    fn spec(args: &[&str], files: &[(&str, &str)]) -> PluginSpec {
        serde_json::from_value(json!({
            "name": "docs",
            "mcp": {"docs": {"command": "node", "args": args, "enabled": true}},
            "files": files.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<BTreeMap<_, _>>()
        }))
        .unwrap()
    }

    fn fixture() -> (AppState, tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new_for_test(dir.path().join("settings.json"));
        let root = dir.path().join("project");
        std::fs::create_dir(&root).unwrap();
        (state, dir, root.canonicalize().unwrap())
    }

    fn store(dir: &Path, root: &Path) -> PluginStore {
        PluginStore::new(dir.to_owned(), Some(root.to_owned()))
    }

    fn current(store: &PluginStore) -> Plugin {
        store
            .list()
            .unwrap()
            .into_iter()
            .find(|p| p.spec.name == "docs")
            .unwrap()
    }

    fn digest_of(store: &PluginStore) -> Option<String> {
        let plugin = current(store);
        plugin_digest(&plugin, &plugin.spec.mcp["docs"])
    }

    fn consented(state: &AppState, root: &Path, store: &PluginStore) -> bool {
        let plugin = current(store);
        state.mcp_consented(root, &plugin, "docs", &plugin.spec.mcp["docs"])
    }

    #[test]
    fn a_change_made_after_the_dialog_is_not_approved_by_it() {
        let (state, dir, root) = fixture();
        let store = store(dir.path(), &root);
        store.save("local", spec(&["c1.js"], &[]), None).unwrap();
        let shown = digest_of(&store);
        // Another call changes the same server to C2 before the answer is recorded.
        let revision = current(&store).revision;
        store
            .save("local", spec(&["c2.js"], &[]), Some(&revision))
            .unwrap();
        state.record_mcp_consent(Some(&root), "local", "docs", &[("docs".into(), shown)]);
        assert!(
            !consented(&state, &root, &store),
            "C2 must not inherit C1's approval"
        );
        // The matching definition is approved.
        let shown = digest_of(&store);
        state.record_mcp_consent(Some(&root), "local", "docs", &[("docs".into(), shown)]);
        assert!(consented(&state, &root, &store));
    }

    #[test]
    fn changing_a_resource_file_under_an_unchanged_command_needs_approval() {
        let (state, dir, root) = fixture();
        let store = store(dir.path(), &root);
        store
            .save(
                "local",
                spec(&["server.js"], &[("server.js", "good")]),
                None,
            )
            .unwrap();
        state.record_user_mcp_consent(Some(&root), "local", "docs", None);
        assert!(consented(&state, &root, &store));
        let revision = current(&store).revision;
        store
            .save(
                "local",
                spec(&["server.js"], &[("server.js", "evil")]),
                Some(&revision),
            )
            .unwrap();
        assert!(!consented(&state, &root, &store));
    }

    #[test]
    fn oversized_resources_fail_closed() {
        let server = McpServer {
            command: Some("node".into()),
            ..Default::default()
        };
        let many: BTreeMap<String, String> = (0..=MAX_FILES)
            .map(|i| (format!("f{i}"), String::new()))
            .collect();
        assert!(definition_digest(&server, &many, &[]).is_none());
        let big = BTreeMap::from([("big".to_owned(), "x".repeat(MAX_FILE_BYTES + 1))]);
        assert!(definition_digest(&server, &big, &[]).is_none());
    }

    #[test]
    fn grants_and_namespaces_follow_the_definition() {
        let (_state, dir, root) = fixture();
        let store = store(dir.path(), &root);
        store.save("local", spec(&["a.js"], &[]), None).unwrap();
        let plugin = current(&store);
        let first = mcp_namespace(&plugin, "docs", &plugin.spec.mcp["docs"]);
        let mut changed = plugin.clone();
        changed.spec.mcp.get_mut("docs").unwrap().args = vec!["b.js".into()];
        let second = mcp_namespace(&changed, "docs", &changed.spec.mcp["docs"]);
        let id = |ns: &str| {
            themis_core::tools::mcp_server_id(&format!(
                "mcp_{}",
                themis_core::plugins::connections::tool_identity(ns, "lookup")
            ))
            .map(str::to_owned)
        };
        assert_ne!(
            id(&first),
            id(&second),
            "a changed command must not inherit chat grants"
        );
    }

    #[test]
    fn a_corrupt_consent_file_is_replaced_by_the_next_approval() {
        let (state, dir, root) = fixture();
        let store = store(dir.path(), &root);
        store.save("local", spec(&["a.js"], &[]), None).unwrap();
        std::fs::write(state.consent_path(), "{ not json").unwrap();
        assert!(!consented(&state, &root, &store));
        state.record_user_mcp_consent(Some(&root), "local", "docs", None);
        assert!(
            consented(&state, &root, &store),
            "version must be current, not 0"
        );
        let text = std::fs::read_to_string(state.consent_path()).unwrap();
        assert!(text.contains(&format!("\"version\": {VERSION}")));
    }

    #[test]
    fn concurrent_approvals_do_not_lose_each_other() {
        let (state, dir, root) = fixture();
        let store = store(dir.path(), &root);
        let spec: PluginSpec = serde_json::from_value(json!({
            "name": "docs",
            "mcp": (0..8).map(|i| (format!("s{i}"), json!({"command":"node","args":[i.to_string()],"enabled":true}))).collect::<serde_json::Map<_,_>>()
        }))
        .unwrap();
        store.save("local", spec, None).unwrap();
        std::thread::scope(|scope| {
            for i in 0..8 {
                let (state, root) = (&state, &root);
                scope.spawn(move || {
                    state.record_user_mcp_consent(
                        Some(root),
                        "local",
                        "docs",
                        Some(&format!("s{i}")),
                    );
                });
            }
        });
        let plugin = current(&store);
        for name in plugin.spec.mcp.keys() {
            assert!(
                state.mcp_consented(&root, &plugin, name, &plugin.spec.mcp[name]),
                "{name}"
            );
        }
    }
}
