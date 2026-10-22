//! Integrations › MCP › Open with: your personal MCP servers as one `mcp.json` copy, opened in
//! your editor and applied when you save it. The copy is not a second registry: each save is
//! validated and written through the integrations store with the revision the copy was made
//! from, so a change made elsewhere meanwhile is a conflict rather than overwritten. Changed
//! commands still need enabling again, as with any other edit.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime};

use serde_json::{json, Map, Value};

use super::{settings_app_dir, AppState};

/// How long a copy is watched for saves after it was opened.
const WATCH_FOR: Duration = Duration::from_secs(2 * 60 * 60);
const HEADER: &str = "// Your personal MCP servers, named integration/server. Save to apply.\n// Marketplace integrations are not listed: make a personal copy to edit one.\n";

/// Ready-to-use servers at the top of the list, commented out: one per way to authenticate.
const EXAMPLES: &str = r#"    // Remove the // in front of an example to add it, then edit it. New servers start
    // turned off; turn them on in Integrations › MCP.
    //
    // ${NAME} is the key called NAME in Settings › Keys (or an environment variable of
    // that name). Keys never go in this file.
    //
    // A local server that reads a key:
    // "personal/github": {
    //   "command": "npx",
    //   "args": ["-y", "@modelcontextprotocol/server-github"],
    //   "env": { "GITHUB_PERSONAL_ACCESS_TOKEN": "${GITHUB_TOKEN}" },
    // },
    //
    // A remote server that takes a key in a header (any header name and prefix work, such
    // as Authorization with "Bearer ${API_KEY}"):
    // "personal/context7": {
    //   "type": "http",
    //   "url": "https://mcp.context7.com/mcp",
    //   "headers": { "CONTEXT7_API_KEY": "${CONTEXT7_API_KEY}" },
    // },
    //
    // A remote server that signs in (OAuth): no key here; press Sign in on its card.
    // "personal/linear": {
    //   "type": "http",
    //   "url": "https://mcp.linear.app/mcp",
    // },
"#;

/// The copy's text: the header, then the servers with [`EXAMPLES`] first in the list.
fn mcp_file_text(servers: &Map<String, Value>) -> Result<String, String> {
    let mut text = format!("{HEADER}{{\n  \"mcpServers\": {{\n{EXAMPLES}");
    let count = servers.len();
    for (index, (name, server)) in servers.iter().enumerate() {
        let body = serde_json::to_string_pretty(server)
            .map_err(|e| e.to_string())?
            .replace('\n', "\n    ");
        let name = serde_json::to_string(name).map_err(|e| e.to_string())?;
        let comma = if index + 1 < count { "," } else { "" };
        text.push_str(&format!("\n    {name}: {body}{comma}\n"));
    }
    text.push_str("  }\n}\n");
    Ok(text)
}

/// One open copy: the revisions it was made from, and the last outcome.
struct Draft {
    path: PathBuf,
    revisions: BTreeMap<String, String>,
    seen: Option<SystemTime>,
    status: Value,
    watching: bool,
}

/// Open copies by app data folder (tests run several app states in one process).
fn drafts() -> &'static Mutex<HashMap<PathBuf, Draft>> {
    static DRAFTS: OnceLock<Mutex<HashMap<PathBuf, Draft>>> = OnceLock::new();
    DRAFTS.get_or_init(Default::default)
}

fn modified(path: &PathBuf) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

impl AppState {
    fn mcp_draft_key(&self) -> PathBuf {
        settings_app_dir(self.inner.settings.path())
    }

    /// Personal global integrations: name → (revision, mcp map).
    async fn personal_mcp(&self) -> Result<BTreeMap<String, (String, Map<String, Value>)>, String> {
        let list = self
            .plugin_action(json!({"action": "list", "scope": "global"}))
            .await?;
        Ok(list
            .as_array()
            .into_iter()
            .flatten()
            .filter(|p| p["scope"] == "global" && p["source"].is_null())
            .filter_map(|p| {
                let name = p["spec"]["name"].as_str()?.to_owned();
                let revision = p["revision"].as_str().unwrap_or_default().to_owned();
                let mcp = p["spec"]["mcp"].as_object().cloned().unwrap_or_default();
                Some((name, (revision, mcp)))
            })
            .collect())
    }

    /// Writes the copy, opens it with `editor` (or the one chosen in Settings) unless `open`
    /// is false, and watches it for saves. Returns the status with the copy's path.
    pub async fn open_mcp_config(
        &self,
        editor: Option<String>,
        open: bool,
    ) -> Result<Value, String> {
        let personal = self.personal_mcp().await?;
        let mut servers = Map::new();
        for (name, (_, mcp)) in &personal {
            for (server, definition) in mcp {
                // Shown in the standard mcpServers shape; a definition the store cannot
                // read is shown as stored rather than hidden.
                let standard = serde_json::from_value(definition.clone())
                    .map(|server| themis_core::plugins::mcp_server_to_standard(&server))
                    .unwrap_or_else(|_| definition.clone());
                servers.insert(format!("{name}/{server}"), standard);
            }
        }
        let text = mcp_file_text(&servers)?;
        let directory = self.mcp_draft_key().join("mcp-edit");
        std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
        let path = directory.join("mcp.json");
        std::fs::write(&path, text).map_err(|e| e.to_string())?;
        let status = json!({"state": "open", "servers": servers.len(), "path": path});
        let start_watch = {
            let mut drafts = drafts().lock().unwrap_or_else(|e| e.into_inner());
            let draft = drafts.entry(self.mcp_draft_key()).or_insert(Draft {
                path: path.clone(),
                revisions: BTreeMap::new(),
                seen: None,
                status: Value::Null,
                watching: false,
            });
            draft.revisions = personal.into_iter().map(|(n, (r, _))| (n, r)).collect();
            draft.seen = modified(&path);
            draft.status = status.clone();
            !std::mem::replace(&mut draft.watching, true)
        };
        if open {
            let editor = match editor {
                Some(editor) => editor,
                None => self.get_settings().await.config_editor,
            };
            super::editors::open_with(&editor, &path)?;
        }
        if start_watch {
            let state = self.clone();
            tokio::spawn(async move { state.watch_mcp_config().await });
        }
        Ok(status)
    }

    async fn watch_mcp_config(&self) {
        let until = std::time::Instant::now() + WATCH_FOR;
        while std::time::Instant::now() < until {
            tokio::time::sleep(Duration::from_millis(800)).await;
            let changed = {
                let drafts = drafts().lock().unwrap_or_else(|e| e.into_inner());
                drafts
                    .get(&self.mcp_draft_key())
                    .is_some_and(|draft| modified(&draft.path) != draft.seen)
            };
            if changed {
                let _ = self.apply_mcp_config().await;
            }
        }
        if let Some(draft) = drafts()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_mut(&self.mcp_draft_key())
        {
            draft.watching = false;
        }
    }

    /// Applies the saved copy: each integration whose servers changed is saved through the
    /// store with the revision the copy was made from. Returns (and records) the outcome.
    pub async fn apply_mcp_config(&self) -> Result<Value, String> {
        let (path, revisions) = {
            let mut drafts = drafts().lock().unwrap_or_else(|e| e.into_inner());
            let draft = drafts
                .get_mut(&self.mcp_draft_key())
                .ok_or("Open the MCP copy first")?;
            draft.seen = modified(&draft.path);
            (draft.path.clone(), draft.revisions.clone())
        };
        let status = self.apply_mcp_file(&path, &revisions).await;
        let mut drafts = drafts().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(draft) = drafts.get_mut(&self.mcp_draft_key()) {
            if let Ok((_, updated)) = &status {
                draft.revisions = updated.clone();
            }
            draft.status = match &status {
                Ok((value, _)) => value.clone(),
                Err(value) => value.clone(),
            };
            return Ok(draft.status.clone());
        }
        Ok(Value::Null)
    }

    async fn apply_mcp_file(
        &self,
        path: &PathBuf,
        revisions: &BTreeMap<String, String>,
    ) -> Result<(Value, BTreeMap<String, String>), Value> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| json!({"state": "invalid", "message": e.to_string()}))?;
        let document = themis_core::configuration::parse_jsonc(&text).map_err(
            |(message, line)| json!({"state": "invalid", "line": line, "message": message}),
        )?;
        let invalid = |message: String| json!({"state": "invalid", "message": message});
        let servers = document["mcpServers"]
            .as_object()
            .ok_or_else(|| invalid("The file needs an \"mcpServers\" object".into()))?;
        let current = self
            .personal_mcp()
            .await
            .map_err(|message| json!({"state": "error", "message": message}))?;
        let mut wanted: BTreeMap<String, Map<String, Value>> = revisions
            .keys()
            .map(|name| (name.clone(), Map::new()))
            .collect();
        // Every global integration name, to tell a new personal one from a marketplace one.
        let global = self
            .plugin_action(json!({"action": "list", "scope": "global"}))
            .await
            .map_err(|message| json!({"state": "error", "message": message}))?;
        let taken = |name: &str| {
            global.as_array().into_iter().flatten().any(|p| {
                p["scope"] == "global" && p["spec"]["name"] == name && !p["source"].is_null()
            })
        };
        for (key, definition) in servers {
            let (integration, server) = key
                .split_once('/')
                .filter(|(i, s)| !i.is_empty() && !s.is_empty())
                .ok_or_else(|| invalid(format!("\"{key}\" must be named integration/server")))?;
            // A new name makes a new personal integration; a marketplace one stays read-only.
            if !wanted.contains_key(integration) {
                if taken(integration) {
                    return Err(invalid(format!(
                        "\"{integration}\" is a marketplace integration; make a personal copy to edit it"
                    )));
                }
                wanted.insert(integration.to_owned(), Map::new());
            }
            let servers = wanted.get_mut(integration).expect("added above");
            let mut parsed = themis_core::plugins::mcp_server_from_standard(server, definition)
                .map_err(|e| invalid(format!("{key}: {e:#}")))?;
            // Whether it runs stays as switched in Integrations unless the file says so.
            if definition.get("enabled").is_none() {
                parsed.enabled = current
                    .get(integration)
                    .and_then(|(_, mcp)| mcp.get(server))
                    .and_then(|old| old["enabled"].as_bool())
                    .unwrap_or(false);
            }
            let value = serde_json::to_value(&parsed)
                .map_err(|e| json!({"state": "error", "message": e.to_string()}))?;
            servers.insert(server.to_owned(), value);
        }
        let mut changed = Vec::new();
        let mut updated = revisions.clone();
        for (name, servers) in wanted {
            if !revisions.contains_key(&name) {
                if current.contains_key(&name) {
                    return Err(
                        json!({"state": "conflict", "message": format!("\"{name}\" was added elsewhere since the copy was opened; open it again")}),
                    );
                }
                self.plugin_action(json!({"action": "save", "scope": "global", "spec": {"name": name, "mcp": servers}}))
                    .await
                    .map_err(|message| json!({"state": "invalid", "message": message}))?;
                updated.insert(name.clone(), String::new());
                changed.push(name);
                continue;
            }
            let Some((revision, mcp)) = current.get(&name) else {
                return Err(
                    json!({"state": "conflict", "message": format!("\"{name}\" was removed since the copy was opened; open it again")}),
                );
            };
            if *mcp == servers {
                updated.insert(name, revision.clone());
                continue;
            }
            if revisions.get(&name) != Some(revision) {
                return Err(
                    json!({"state": "conflict", "message": format!("\"{name}\" changed since the copy was opened; open it again")}),
                );
            }
            let list = self
                .plugin_action(json!({"action": "list", "scope": "global"}))
                .await
                .map_err(|message| json!({"state": "error", "message": message}))?;
            let mut spec = list
                .as_array()
                .into_iter()
                .flatten()
                .find(|p| p["scope"] == "global" && p["spec"]["name"] == name.as_str())
                .map(|p| p["spec"].clone())
                .ok_or_else(
                    || json!({"state": "conflict", "message": format!("\"{name}\" is gone")}),
                )?;
            spec["mcp"] = Value::Object(servers);
            self.plugin_action(
                json!({"action": "save", "scope": "global", "spec": spec, "expectedRevision": revision}))
            .await
            .map_err(|message| json!({"state": "invalid", "message": message}))?;
            changed.push(name);
        }
        // Take the new revisions so the next save compares against what was just written.
        for (name, (revision, _)) in self.personal_mcp().await.unwrap_or_default() {
            if updated.contains_key(&name) {
                updated.insert(name, revision);
            }
        }
        Ok((json!({"state": "applied", "changed": changed}), updated))
    }

    /// The last outcome for the open copy (`closed` when none is open).
    pub fn mcp_config_status(&self) -> Value {
        drafts()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&self.mcp_draft_key())
            .map(|draft| draft.status.clone())
            .unwrap_or_else(|| json!({"state": "closed"}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_example_applies_once_uncommented() {
        let servers = Map::from_iter([(
            "personal/files".to_owned(),
            json!({"command": "npx", "args": ["-y", "x"]}),
        )]);
        let text = mcp_file_text(&servers).unwrap();
        let parsed = themis_core::configuration::parse_jsonc(&text).unwrap();
        assert_eq!(parsed["mcpServers"].as_object().unwrap().len(), 1);
        // Uncommenting every example line gives a file with each example as a valid server.
        let uncommented: String = text
            .lines()
            .map(|line| match line.strip_prefix("    // ") {
                Some(rest)
                    if rest.starts_with('"') || rest.starts_with(' ') || rest.starts_with('}') =>
                {
                    format!("    {rest}\n")
                }
                _ => format!("{line}\n"),
            })
            .collect();
        let parsed = themis_core::configuration::parse_jsonc(&uncommented).unwrap();
        let all = parsed["mcpServers"].as_object().unwrap();
        assert_eq!(all.len(), 4);
        for (name, raw) in all {
            let server = name.split_once('/').unwrap().1;
            let parsed = themis_core::plugins::mcp_server_from_standard(server, raw)
                .unwrap_or_else(|e| panic!("{name}: {e:#}"));
            parsed.validate().unwrap();
        }
    }
}
