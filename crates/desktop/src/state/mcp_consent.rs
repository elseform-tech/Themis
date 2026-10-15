//! Consent for starting MCP connections.
//!
//! Enabling a connection is the consent to start it, but only when a person
//! enabled that exact definition. This module records, in `mcp_consent.json`
//! beside the plugin registry, a fingerprint of the definition (command, args,
//! url, env references, bearer variable) a person approved: directly in the UI
//! or CLI, or by answering the approval dialog for an agent's change. A
//! definition that is new, changed by anything else (an agent edit that was not
//! approved, a catalog update, a repository's `.themis/plugins/`), or missing
//! from the file does not start until a person enables it again.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use themis_core::plugins::{connections::McpServer, Plugin};

use super::AppState;

const VERSION: u32 = 1;
const MAX_ENTRIES: usize = 4096;

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConsentFile {
    version: u32,
    #[serde(default)]
    approved: BTreeMap<String, String>,
}

/// Stable identity of what a person approved. `enabled` is excluded: it is the
/// switch the consent applies to.
pub(super) fn definition_fingerprint(server: &McpServer) -> String {
    let canonical = serde_json::json!([
        server.command,
        server.args,
        server.url,
        server.env,
        server.bearer_env
    ]);
    Sha256::digest(canonical.to_string().as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn root_option(root: &Path) -> Option<PathBuf> {
    (!root.as_os_str().is_empty()).then(|| root.to_owned())
}

fn consent_key(root: &Path, scope: &str, plugin: &str, server: &str) -> String {
    // A project's definitions are consented per project; global ones everywhere.
    let place = if scope == "local" {
        root.to_string_lossy().into_owned()
    } else {
        String::new()
    };
    format!("{scope}\0{place}\0{plugin}\0{server}")
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
    /// project definitions are never seeded. An unreadable file approves nothing.
    fn load_consent(&self) -> ConsentFile {
        let path = self.consent_path();
        match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str::<ConsentFile>(&text)
                .ok()
                .filter(|file| file.version == VERSION && file.approved.len() <= MAX_ENTRIES)
                .unwrap_or_default(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let mut file = ConsentFile {
                    version: VERSION,
                    approved: BTreeMap::new(),
                };
                if let Ok(plugins) = self.plugin_store(None).list() {
                    for plugin in plugins.iter().filter(|plugin| plugin.scope == "global") {
                        for (name, server) in plugin.spec.mcp.iter().filter(|(_, s)| s.enabled) {
                            file.approved.insert(
                                consent_key(Path::new(""), "global", &plugin.spec.name, name),
                                definition_fingerprint(server),
                            );
                        }
                    }
                }
                self.save_consent(&file);
                file
            }
            Err(_) => ConsentFile::default(),
        }
    }

    fn save_consent(&self, file: &ConsentFile) {
        if let Ok(text) = serde_json::to_string_pretty(file) {
            let path = self.consent_path();
            let tmp = path.with_extension("json.tmp");
            if std::fs::write(&tmp, text).is_ok() {
                let _ = std::fs::rename(&tmp, path);
            }
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
        self.load_consent()
            .approved
            .get(&consent_key(root, &plugin.scope, &plugin.spec.name, name))
            .is_some_and(|approved| *approved == definition_fingerprint(server))
    }

    /// Records approval of the named servers' current stored definitions.
    /// Called only after a direct user action or an approved agent change.
    pub(super) fn record_mcp_consent(
        &self,
        root: Option<&Path>,
        scope: &str,
        plugin_name: &str,
        servers: &[String],
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
        let mut file = self.load_consent();
        for name in servers {
            if let Some(server) = plugin.spec.mcp.get(name).filter(|s| s.enabled) {
                file.approved.insert(
                    consent_key(root, scope, plugin_name, name),
                    definition_fingerprint(server),
                );
            }
        }
        if file.approved.len() > MAX_ENTRIES {
            return;
        }
        self.save_consent(&file);
    }

    /// Consent for every enabled server of a plugin a person just saved or enabled.
    pub(super) fn record_user_mcp_consent(
        &self,
        root: Option<&Path>,
        scope: &str,
        plugin_name: &str,
        only: Option<&str>,
    ) {
        let servers: Vec<String> = self
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
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        self.record_mcp_consent(root, scope, plugin_name, &servers);
    }
}

impl AppState {
    /// Whether the stored consent already covers exactly `server`.
    pub(super) fn mcp_definition_consented(
        &self,
        root: &Path,
        scope: &str,
        plugin: &str,
        name: &str,
        server: &McpServer,
    ) -> bool {
        self.load_consent()
            .approved
            .get(&consent_key(root, scope, plugin, name))
            .is_some_and(|approved| *approved == definition_fingerprint(server))
    }
}
