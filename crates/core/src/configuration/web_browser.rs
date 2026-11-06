use super::*;

/// Web tools. `search_url` is the person's own SearXNG instance; without it
/// `web_search` is not offered. Only the user file may set it, so a project
/// cannot send searches elsewhere.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WebConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub search_url: Option<String>,
}

/// The built-in browser and what the agent may do in it (Settings › Browser). A project may
/// only tighten it; `executable`, `downloads_directory` and `open_workspace` may only come
/// from the user file. See `docs/web-tools.md`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BrowserConfig {
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub executable: Option<String>,
    /// Developer mode, "Enable full CDP access": off means no debugging tools at all.
    pub developer_mode: bool,
    /// The defaults for each permission; site exceptions override them.
    pub permissions: BrowserPermissions,
    /// Per-site values, the most specific site first wins. Arrays replace earlier arrays,
    /// so a person can remove the starter entries.
    pub site_exceptions: Vec<SiteException>,
    /// Block-only exceptions a project added; any match blocks, whatever the user allows.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub site_restrictions: Vec<SiteException>,
    /// Show cookie values, auth headers and token-like values to the agent.
    pub show_secrets: bool,
    /// Where downloads are saved; the system Downloads folder when unset.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub downloads_directory: Option<String>,
    /// Open Workspace on the Browser tab when the agent starts browsing.
    pub open_workspace: bool,
    /// A screenshot the agent takes also goes to its model when the model reads images (and
    /// to the goal check's, when that one does). Off, every model gets the text result only.
    pub screenshots_to_model: bool,
}
impl Default for BrowserConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            executable: None,
            developer_mode: true,
            permissions: BrowserPermissions::default(),
            site_exceptions: starter_site_exceptions(),
            site_restrictions: Vec::new(),
            show_secrets: false,
            downloads_directory: None,
            open_workspace: false,
            screenshots_to_model: true,
        }
    }
}

/// Allow (no prompt), Ask (an approval prompt; YOLO allows) or Block (refused in every mode).
/// Ordered from most to least permissive, so a project may only move a value up.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum SiteAccess {
    Allow,
    Ask,
    Block,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct BrowserPermissions {
    /// Open, navigate, click, type, screenshots and recording.
    pub browsing: SiteAccess,
    /// Saving a file the page or an agent action downloads.
    pub downloads: SiteAccess,
    /// The debugging tools (console, elements, network, debugger, storage, profiling).
    pub debugging: SiteAccess,
}
impl Default for BrowserPermissions {
    fn default() -> Self {
        Self {
            browsing: SiteAccess::Allow,
            downloads: SiteAccess::Allow,
            debugging: SiteAccess::Block,
        }
    }
}

/// One site's own values; an unset permission falls back to the defaults.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct SiteException {
    /// A host such as `example.com`; it covers its subdomains.
    pub site: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub browsing: Option<SiteAccess>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub downloads: Option<SiteAccess>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub debugging: Option<SiteAccess>,
}

/// The starter exceptions: sign-in, payment and identity sites the agent does not browse.
/// A short starter set, not complete; the person edits it in Settings › Browser.
pub const STARTER_BLOCKED_SITES: [&str; 9] = [
    "paypal.com",
    "stripe.com",
    "wise.com",
    "revolut.com",
    "accounts.google.com",
    "login.microsoftonline.com",
    "appleid.apple.com",
    "id.me",
    "login.gov",
];

pub fn starter_site_exceptions() -> Vec<SiteException> {
    STARTER_BLOCKED_SITES
        .iter()
        .map(|site| SiteException {
            site: (*site).to_owned(),
            browsing: Some(SiteAccess::Block),
            ..SiteException::default()
        })
        .collect()
}

/// Project and run layers may not point web or browser settings elsewhere;
/// they may only turn the browser off.
pub(super) fn check_web_restriction(layer: &Value, origin: &str) -> Result<()> {
    ensure!(
        layer.pointer("/web/search_url").is_none(),
        "web.search_url may only be set in the user runtime.jsonc, not the {origin} configuration"
    );
    for field in [
        "executable",
        "downloads_directory",
        "open_workspace",
        "site_restrictions",
    ] {
        ensure!(
            layer.pointer(&format!("/browser/{field}")).is_none(),
            "browser.{field} may only be set in the user runtime.jsonc, not the {origin} configuration"
        );
    }
    for field in [
        "enabled",
        "developer_mode",
        "show_secrets",
        "screenshots_to_model",
    ] {
        if let Some(value) = layer.pointer(&format!("/browser/{field}")) {
            ensure!(
                value == &Value::Bool(false),
                "the {origin} configuration may only set browser.{field} to false"
            );
        }
    }
    Ok(())
}

/// A project's browser layer may only tighten: each permission default may only move toward
/// Block, and its site exceptions may only block. They become `site_restrictions`, which
/// block whatever the user's own exceptions allow, instead of replacing the user's list.
pub(super) fn tighten_browser_layer(
    current: &BrowserConfig,
    layer: &mut Value,
    origin: &str,
) -> Result<()> {
    let Some(browser) = layer.get_mut("browser").and_then(Value::as_object_mut) else {
        return Ok(());
    };
    if let Some(permissions) = browser.get("permissions") {
        let permissions: BrowserPermissions = serde_json::from_value(
            serde_json::json!(current.permissions)
                .as_object()
                .cloned()
                .map(|mut base| {
                    if let Some(over) = permissions.as_object() {
                        base.extend(over.clone());
                    }
                    Value::Object(base)
                })
                .unwrap_or_default(),
        )
        .context("invalid browser.permissions")?;
        for (name, before, after) in [
            (
                "browsing",
                current.permissions.browsing,
                permissions.browsing,
            ),
            (
                "downloads",
                current.permissions.downloads,
                permissions.downloads,
            ),
            (
                "debugging",
                current.permissions.debugging,
                permissions.debugging,
            ),
        ] {
            ensure!(
                after >= before,
                "the {origin} configuration may only make browser.permissions.{name} stricter"
            );
        }
    }
    if let Some(exceptions) = browser.remove("site_exceptions") {
        let exceptions: Vec<SiteException> =
            serde_json::from_value(exceptions).context("invalid browser.site_exceptions")?;
        for exception in &exceptions {
            ensure!(
                [exception.browsing, exception.downloads, exception.debugging]
                    .iter()
                    .all(|value| matches!(value, None | Some(SiteAccess::Block))),
                "the {origin} configuration may only add site exceptions that block (site '{}')",
                exception.site
            );
        }
        let mut restrictions = exceptions;
        restrictions.extend(current.site_restrictions.iter().cloned());
        browser.insert(
            "site_restrictions".into(),
            serde_json::to_value(restrictions)?,
        );
    }
    Ok(())
}

pub(super) fn validate_web(config: &RuntimeConfig) -> Result<()> {
    if let Some(url) = &config.web.search_url {
        let parsed = reqwest::Url::parse(url)
            .ok()
            .filter(|_| url.len() <= 1024)
            .context("web.search_url must be an http or https URL of at most 1,024 bytes")?;
        ensure!(
            matches!(parsed.scheme(), "http" | "https") && parsed.host_str().is_some(),
            "web.search_url must be an http or https URL"
        );
        ensure!(
            parsed.username().is_empty() && parsed.password().is_none(),
            "web.search_url must not contain credentials"
        );
    }
    if let Some(path) = &config.browser.executable {
        ensure!(
            path.len() <= 1024 && std::path::Path::new(path).is_absolute(),
            "browser.executable must be an absolute path of at most 1,024 bytes"
        );
    }
    if let Some(path) = &config.browser.downloads_directory {
        ensure!(
            path.len() <= 1024 && std::path::Path::new(path).is_absolute(),
            "browser.downloads_directory must be an absolute path of at most 1,024 bytes"
        );
    }
    let exceptions = &config.browser.site_exceptions;
    ensure!(
        exceptions.len() + config.browser.site_restrictions.len() <= 500,
        "browser.site_exceptions holds at most 500 sites"
    );
    for exception in exceptions.iter().chain(&config.browser.site_restrictions) {
        ensure!(
            valid_site(&exception.site) && exception.site == exception.site.to_ascii_lowercase(),
            "browser.site_exceptions: '{}' is not a site such as example.com (lowercase host, no scheme, port or path)",
            exception.site
        );
    }
    Ok(())
}
