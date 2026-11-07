use super::connections::McpOauth;
use super::*;
use serde_json::{json, Value};

mod command_source;
mod directory_import;
mod hook_import;
mod mcp_import;
#[cfg(test)]
mod size_tests;
#[cfg(test)]
mod source_tests;
mod sources;
mod store;

use command_source::*;
use directory_import::*;
pub(super) use directory_import::{collect_files, parse_skill};
use hook_import::*;
use mcp_import::*;
pub use mcp_import::{mcp_server_from_standard, mcp_server_to_standard};
use sources::*;
pub(super) use sources::{git_remote_ref, tree_fingerprint};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportComponent {
    pub kind: String,
    pub name: String,
    pub status: String,
    pub field: Option<String>,
    pub reason: Option<String>,
    pub remedy: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportReport {
    pub status: String,
    pub components: Vec<ImportComponent>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportPreview {
    pub spec: PluginSpec,
    pub report: ImportReport,
    /// Identifies a saved marketplace check of this entry, so an install can refuse a newer one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check_revision: Option<String>,
    /// [`package_hash`] of the checked package, kept when the saved check drops its contents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_hash: Option<String>,
}
/// What a package contains, ignoring what people change after installing it: switches and the
/// record of an earlier hash.
pub(super) fn package_hash(spec: &PluginSpec) -> String {
    use sha2::{Digest, Sha256};
    let mut value = serde_json::to_value(spec).unwrap_or_default();
    for server in value["mcp"]
        .as_object_mut()
        .into_iter()
        .flat_map(|mcp| mcp.values_mut())
    {
        server["enabled"] = Value::Null;
    }
    for hook in value["hooks"].as_array_mut().into_iter().flatten() {
        hook["enabled"] = Value::Null;
    }
    for field in [
        "package_hash",
        "disabled_skills",
        "origin",
        "icon_asset",
        "brand",
    ] {
        value[field] = Value::Null;
    }
    format!("{:x}", Sha256::digest(value.to_string()))
}
/// Most bytes kept of a plugin's own icon.
const ICON_LIMIT: usize = 256 * 1024;
/// Whether an icon named by a manifest can be used: HTTPS, or a bundled SVG, PNG or WebP.
fn usable_icon(icon: &str, files: &BTreeMap<String, String>, root: &Path) -> bool {
    icon.starts_with("https://")
        || (icon.ends_with(".svg") && files.contains_key(icon))
        || ((icon.ends_with(".png") || icon.ends_with(".webp"))
            && safe_relative(icon)
            && root.join(icon).is_file())
}
/// The file name an icon is stored under, or `None` for anything but an SVG, PNG or WebP.
pub(super) fn icon_asset_name(bytes: &[u8]) -> Option<String> {
    use sha2::{Digest, Sha256};
    if bytes.is_empty() || bytes.len() > ICON_LIMIT {
        return None;
    }
    let text = std::str::from_utf8(bytes).unwrap_or_default().trim_start();
    let extension = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        "png"
    } else if bytes.len() > 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        "webp"
    } else if text.starts_with("<svg") || (text.starts_with("<?xml") && text.contains("<svg")) {
        "svg"
    } else {
        return None;
    };
    Some(format!("{:x}.{extension}", Sha256::digest(bytes)))
}
/// An HTTPS icon, bounded in size and time. Only ever called during a compatibility check, so
/// browsing makes no network requests.
async fn download_icon(url: &str) -> anyhow::Result<Vec<u8>> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.url().scheme() == "https" && attempt.previous().len() < 3 {
                attempt.follow()
            } else {
                attempt.stop()
            }
        }))
        .build()?;
    let mut response = client.get(url).send().await?.error_for_status()?;
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        bytes.extend_from_slice(&chunk);
        if bytes.len() > ICON_LIMIT {
            bail!("Icon is too large");
        }
    }
    Ok(bytes)
}
/// A definition without its switch, to tell a changed part from one only turned on or off.
fn same_definition<T: Serialize>(old: &T, new: &T) -> bool {
    let plain = |part: &T| {
        let mut value = serde_json::to_value(part).unwrap_or_default();
        value["enabled"] = Value::Null;
        value
    };
    plain(old) == plain(new)
}
/// Switches off every MCP server and hook of `spec` that is new or changed since `previous`
/// (all of them without one); an unchanged part keeps its old switch.
pub(crate) fn switch_off_new_parts(previous: Option<&PluginSpec>, spec: &mut PluginSpec) {
    for (name, server) in &mut spec.mcp {
        server.enabled = previous
            .and_then(|previous| previous.mcp.get(name))
            .is_some_and(|old| old.enabled && same_definition(old, &*server));
    }
    for hook in &mut spec.hooks {
        hook.enabled = previous
            .and_then(|previous| previous.hooks.iter().find(|old| old.name == hook.name))
            .is_some_and(|old| old.enabled && same_definition(old, &*hook));
    }
}
impl ImportPreview {
    pub fn from_spec(spec: PluginSpec) -> anyhow::Result<Self> {
        validate(&spec)?;
        Ok(Self::new(spec))
    }
    fn new(spec: PluginSpec) -> Self {
        let mut components = spec.import_issues.clone();
        for (kind, names) in [
            (
                "skill",
                spec.skills.iter().map(|s| s.id.clone()).collect::<Vec<_>>(),
            ),
            ("mcp", spec.mcp.keys().cloned().collect()),
            ("hook", spec.hooks.iter().map(|h| h.name.clone()).collect()),
            (
                "subagent",
                spec.subagents.iter().map(|p| p.name.clone()).collect(),
            ),
        ] {
            components.extend(names.into_iter().map(|name| ImportComponent {
                kind: kind.into(),
                name,
                status: "supported".into(),
                field: None,
                reason: None,
                remedy: None,
            }));
        }
        components.extend(spec.unsupported.iter().map(|reason| ImportComponent {
            kind: "package".into(),
            name: spec.name.clone(),
            status: "unsupported".into(),
            field: None,
            reason: Some(reason.clone()),
            remedy: Some(
                "Remove the unsupported component or install only supported components".into(),
            ),
        }));
        components.extend(spec.notices.iter().map(|reason| ImportComponent {
            kind: "package".into(),
            name: spec.name.clone(),
            status: "notice".into(),
            field: None,
            reason: Some(reason.clone()),
            remedy: None,
        }));
        let status = if !spec.has_capabilities() {
            "unsupported"
        } else if components
            .iter()
            .any(|c| !matches!(c.status.as_str(), "supported" | "notice"))
        {
            "partial"
        } else {
            "supported"
        };
        Self {
            spec,
            report: ImportReport {
                status: status.into(),
                components,
            },
            check_revision: None,
            package_hash: None,
        }
    }
}

#[derive(Debug)]
struct UnsupportedMcp {
    field: String,
    reason: &'static str,
    remedy: &'static str,
}
impl std::fmt::Display for UnsupportedMcp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}. {}", self.field, self.reason, self.remedy)
    }
}
impl std::error::Error for UnsupportedMcp {}
fn unsupported_mcp(
    field: impl Into<String>,
    reason: &'static str,
    remedy: &'static str,
) -> anyhow::Error {
    UnsupportedMcp {
        field: field.into(),
        reason,
        remedy,
    }
    .into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Marketplace {
    pub name: String,
    pub source: String,
    /// Whether Discover shows this source.
    #[serde(default = "shown")]
    pub enabled: bool,
}
fn shown() -> bool {
    true
}
/// The source every install starts with; it can be hidden but not removed.
pub(super) const OFFICIAL_MARKETPLACE: &str = "claude-plugins-official";
/// What a source is, read before adding it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SourcePreview {
    pub name: String,
    pub publisher: String,
    pub plugins: usize,
    pub source: String,
}
#[derive(Default, Serialize, Deserialize)]
struct Catalogs {
    sources: Vec<Marketplace>,
}

impl PluginSpec {
    fn has_capabilities(&self) -> bool {
        !(self.skills.is_empty()
            && self.mcp.is_empty()
            && self.hooks.is_empty()
            && self.subagents.is_empty())
    }
    /// Installation must add a capability; empty personal drafts may still be saved.
    pub fn ensure_installable(&self) -> anyhow::Result<()> {
        if !self.has_capabilities() {
            bail!("No supported capabilities to install. Inspect compatibility details or choose another package.");
        }
        Ok(())
    }
}
