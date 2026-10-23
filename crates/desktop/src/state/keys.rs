//! Settings › Keys: named keys that MCP servers reference as `${NAME}`. Values live in the
//! OS credential store and never cross back to the frontend; `keys.json` in app data keeps
//! only the names, because credential stores cannot list their entries.

use super::*;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use themis_core::plugins::keys;

impl AppState {
    fn key_names_path(&self) -> PathBuf {
        settings_app_dir(self.inner.settings.path()).join("keys.json")
    }

    fn saved_key_names(&self) -> Vec<String> {
        std::fs::read(self.key_names_path())
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Vec<String>>(&bytes).ok())
            .unwrap_or_default()
            .into_iter()
            .filter(|name| keys::valid_name(name))
            .collect()
    }

    fn write_key_names(&self, mut names: Vec<String>) -> Result<(), String> {
        names.sort();
        names.dedup();
        let path = self.key_names_path();
        let temporary = path.with_extension("json.tmp");
        std::fs::write(
            &temporary,
            serde_json::to_vec_pretty(&names).map_err(|e| e.to_string())?,
        )
        .and_then(|()| std::fs::rename(&temporary, &path))
        .map_err(|e| format!("write '{}': {e}", path.display()))
    }

    /// Every saved key and every key an installed server reads: its name, whether it is
    /// saved or found in the environment, and which servers use it. Never values.
    pub async fn list_keys(&self, project: Option<PathBuf>) -> Result<Value, String> {
        let mut used: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for plugin in self
            .plugin_store(project)
            .list()
            .map_err(|e| e.to_string())?
        {
            for (server, definition) in &plugin.spec.mcp {
                for name in definition.key_names() {
                    let entry = used.entry(name).or_default();
                    let label = format!("{}/{server}", plugin.spec.name);
                    if !entry.contains(&label) {
                        entry.push(label);
                    }
                }
            }
        }
        // OpenCode Go's secret (Settings › Models), listed even before it is saved.
        used.entry(self.go_secret_name().await)
            .or_default()
            .insert(0, "OpenCode Go".to_owned());
        for name in self.saved_key_names() {
            used.entry(name).or_default();
        }
        // Saved without its name recorded (copied from the old entry, or by the CLI with
        // no server running): still listed, so it can be seen and removed.
        let go = crate::secrets::GO_KEY_NAME;
        if self
            .inner
            .secrets
            .stored(&keys::store_key(go))
            .is_ok_and(|value| value.is_some())
        {
            used.entry(go.to_owned()).or_default();
        }
        Ok(json!(used
            .into_iter()
            .map(|(name, used_by)| {
                let saved = self
                    .inner
                    .secrets
                    .stored(&keys::store_key(&name))
                    .is_ok_and(|value| value.is_some());
                let environment = !saved && std::env::var_os(&name).is_some_and(|v| !v.is_empty());
                json!({"name": name, "saved": saved, "environment": environment, "used_by": used_by})
            })
            .collect::<Vec<_>>()))
    }

    /// Saves `value` as the key `name`, replacing an older value.
    pub fn set_key(&self, name: &str, value: &str) -> Result<(), String> {
        if !keys::valid_name(name) {
            return Err("Key names use letters, digits and _ (up to 128)".into());
        }
        crate::secrets::validate_provider_key(value)
            .map_err(|_| "A key is 1–4096 printable characters without spaces".to_owned())?;
        self.inner
            .secrets
            .set(&keys::store_key(name), value)
            .map_err(|e| format!("Could not save {name}: {}", store_reason(&e)))?;
        self.record_key_name(name)
    }

    /// Remembers that a secret named `name` is saved (credential stores cannot list).
    pub(super) fn record_key_name(&self, name: &str) -> Result<(), String> {
        let mut names = self.saved_key_names();
        names.push(name.to_owned());
        self.write_key_names(names)
    }

    /// Forgets the key `name`; servers that read it then report it missing.
    pub fn remove_key(&self, name: &str) -> Result<(), String> {
        if !keys::valid_name(name) {
            return Err("Unknown key".into());
        }
        super::preferences::clear_secret_named(self.inner.secrets.as_ref(), name)
            .map_err(|e| format!("Could not remove {name}: {}", store_reason(&e)))?;
        let names = self
            .saved_key_names()
            .into_iter()
            .filter(|n| n != name)
            .collect();
        self.write_key_names(names)
    }
}

/// The credential store's own reason, without its internal entry name.
fn store_reason(error: &str) -> &str {
    error.rsplit(": ").next().unwrap_or(error)
}
