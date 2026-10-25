//! Versioned runtime configuration and immutable per-run approval policy.
use crate::chat_grants::ChatGrants;
use crate::tools::{is_destructive_variant, Approval, ApprovalHook, RiskLevel, ToolAction};
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, sync::Arc};

mod providers;
pub use providers::{
    model_override, provider_for, valid_model_id, ModelOverride, ModelPrice, ProviderDefinition,
    ProviderModel, GO_PROVIDER_ID, MAX_PROVIDER_MODELS, REASONING_EFFORTS,
};

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalMode {
    Yolo,
    #[default]
    Custom,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PolicyAction {
    Allow,
    #[default]
    Ask,
    Deny,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalRule {
    pub tool: String,
    pub action: PolicyAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub risk: Option<String>,
    /// Argv prefix (program, then arguments) a `shell` call must start with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ApprovalPolicy {
    pub default: PolicyAction,
    pub rules: Vec<ApprovalRule>,
}
impl Default for ApprovalPolicy {
    fn default() -> Self {
        Self {
            default: PolicyAction::Ask,
            rules: vec![ApprovalRule {
                command: None,
                tool: "*".into(),
                action: PolicyAction::Allow,
                risk: Some("read".into()),
            }],
        }
    }
}
impl ApprovalPolicy {
    /// First matching rule decides. `recall` reads the same saved evidence that
    /// `read_file` can open, so a deny that covers `read_file` also denies it.
    pub fn decision(&self, action: &ToolAction) -> PolicyAction {
        let decision = self.rule_decision(action);
        if action.tool == "recall" && decision != PolicyAction::Deny {
            let mirrored = ToolAction {
                tool: "read_file".into(),
                ..action.clone()
            };
            if self.rule_decision(&mirrored) == PolicyAction::Deny {
                return PolicyAction::Deny;
            }
        }
        decision
    }

    fn rule_decision(&self, action: &ToolAction) -> PolicyAction {
        self.rules
            .iter()
            .find(|rule| rule_matches(rule, action))
            .map_or(self.default, |rule| rule.action)
    }
}
/// Whether `rule` applies to `action` (tool name or `*` prefix, command
/// prefix with deny-only widening, risk).
fn rule_matches(rule: &ApprovalRule, action: &ToolAction) -> bool {
    let matches = rule
        .tool
        .strip_suffix('*')
        .map_or(rule.tool == action.tool, |prefix| {
            action.tool.starts_with(prefix)
        });
    if crate::tools::is_site_tool(&action.tool) {
        // Web and browser rules name a site: the host or any of its subdomains.
        return matches
            && rule.command.as_ref().is_none_or(|sites| {
                action
                    .command
                    .first()
                    .is_some_and(|host| site_matches(host, &sites[0]))
            })
            && rule
                .risk
                .as_ref()
                .is_none_or(|risk| risk == risk_name(action.risk));
    }
    matches
        && match rule.command.as_ref() {
            // A broad allow without `command` is trust in the tool,
            // not in its destructive variants (`git push --force`,
            // `rm -rf`): those need a rule that names them or an
            // explicit `"risk": "destructive"`.
            None => {
                rule.action != PolicyAction::Allow
                    || rule.risk.as_deref() == Some("destructive")
                    || !is_destructive_variant(&action.command)
            }
            Some(prefix) => {
                // A broad allow (`["git","push"]`) must not silently cover
                // a destructive variant (`git push --force`) unless the
                // rule itself names that variant.
                let covers_variant = rule.action != PolicyAction::Allow
                    || !is_destructive_variant(&action.command)
                    || is_destructive_variant(prefix);
                // Deny rules also see commands inside wrappers, so
                // `sh -c "git push --force"` is denied by `["git","push","--force"]`.
                covers_variant
                    && (command_has_prefix(&action.command, prefix)
                        || (rule.action == PolicyAction::Deny
                            && crate::tools::commands_run_by(&action.command)
                                .unwrap_or_else(|| vec![action.command.clone()])
                                .iter()
                                .any(|command| deny_matches(command, prefix))))
            }
        }
        && rule
            .risk
            .as_ref()
            .is_none_or(|risk| risk == risk_name(action.risk))
}
/// `host` is `site` or one of its subdomains (`docs.example.com` for
/// `example.com`), compared without case.
pub fn site_matches(host: &str, site: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    let site = site.trim_end_matches('.').to_ascii_lowercase();
    !site.is_empty()
        && (host == site
            || host
                .strip_suffix(&site)
                .is_some_and(|rest| rest.ends_with('.')))
}

/// A site in an approval rule: a host name or IP address, without scheme,
/// port, path or wildcard.
fn valid_site(site: &str) -> bool {
    !site.is_empty()
        && site.len() <= 253
        && site
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '[' | ']'))
        && !site.starts_with('.')
}
/// Exact token-prefix match: `["cargo","test"]` matches `cargo test --lib` but
/// not `cargo testing`, `/usr/bin/cargo test` or an empty command.
pub fn command_has_prefix(command: &[String], prefix: &[String]) -> bool {
    !prefix.is_empty() && command.len() >= prefix.len() && command[..prefix.len()] == *prefix
}
/// Normalized program name: basename, lowercased on macOS/Windows
/// (case-insensitive file systems), `.exe`/`.cmd`/`.bat` stripped on Windows.
fn normalize_program(program: &str) -> String {
    let base = program
        .rsplit(|c| c == '/' || (cfg!(windows) && c == '\\'))
        .next()
        .unwrap_or(program);
    let mut name = if cfg!(any(target_os = "macos", windows)) {
        base.to_lowercase()
    } else {
        base.to_owned()
    };
    if cfg!(windows) {
        for ext in [".exe", ".cmd", ".bat"] {
            if let Some(stem) = name.strip_suffix(ext) {
                name = stem.to_owned();
                break;
            }
        }
    }
    name
}
/// Deny-rule match: the program token is normalized on both sides (so `/bin/rm`,
/// `./rm` and `RM` on case-insensitive systems are still denied by `["rm"]`) and
/// flags are compared by meaning rather than spelling (see [`flag_present`]):
/// the rule's non-flag tokens must begin the command's non-flag tokens, and each
/// flag the rule names must appear anywhere in the command before `--`. So
/// `["git","push","--force"]` also denies `git push -f origin main` and
/// `git push origin --force`, and `["rm","-rf"]` denies `rm -fr x` and
/// `rm -r -f x`. Deny rules may only widen this way; allow/ask rules stay exact.
fn deny_matches(command: &[String], prefix: &[String]) -> bool {
    let (Some((program, rest)), Some((first, wanted))) =
        (command.split_first(), prefix.split_first())
    else {
        return false;
    };
    let program = normalize_program(program);
    if program != normalize_program(first) {
        return false;
    }
    if rest.starts_with(wanted) {
        return true;
    }
    let before_separator: Vec<&str> = rest
        .iter()
        .map(String::as_str)
        .take_while(|arg| *arg != "--")
        .collect();
    let is_flag = |token: &str| token.len() > 1 && token.starts_with('-');
    let mut words = before_separator.iter().filter(|arg| !is_flag(arg));
    wanted
        .iter()
        .filter(|token| !is_flag(token))
        .all(|token| words.next() == Some(&token.as_str()))
        && wanted
            .iter()
            .filter(|token| is_flag(token))
            .all(|flag| flag_present(&program, flag, &before_separator))
}

/// Short/long spellings with one meaning for a program, used only to widen deny
/// rules: `(program, long name, short letters)`.
const FLAG_EQUIVALENTS: &[(&str, &str, &str)] = &[
    ("git", "force", "f"),
    ("git", "delete", "d"),
    ("git", "all", "a"),
    ("git", "recursive", "r"),
    ("rm", "force", "f"),
    ("rm", "recursive", "rR"),
    ("rm", "dir", "d"),
    ("cp", "force", "f"),
    ("cp", "recursive", "rR"),
    ("mv", "force", "f"),
    ("chmod", "recursive", "R"),
    ("chown", "recursive", "R"),
];

/// Whether the command (tokens before `--`) carries `flag` under any equivalent
/// spelling: the exact token, `--name=value` for `--name`, a short-flag cluster
/// containing every letter of a short rule flag (`-rf` covers `-f`, `-fr`,
/// `-r -f`), or the short/long equivalent from [`FLAG_EQUIVALENTS`].
fn flag_present(program: &str, flag: &str, args: &[&str]) -> bool {
    let shorts: String = args
        .iter()
        .filter(|arg| arg.starts_with('-') && !arg.starts_with("--"))
        .flat_map(|arg| arg.chars().skip(1))
        .collect();
    let has_long = |name: &str| {
        args.iter().any(|arg| {
            arg.strip_prefix("--")
                .is_some_and(|long| long == name || long.starts_with(&format!("{name}=")))
        })
    };
    if args.contains(&flag) {
        return true;
    }
    if let Some(long) = flag.strip_prefix("--") {
        let name = long.split('=').next().unwrap_or(long);
        if long == name && has_long(name) {
            return true;
        }
        return long == name
            && FLAG_EQUIVALENTS
                .iter()
                .filter(|(p, l, _)| *p == program && *l == name)
                .any(|(_, _, letters)| letters.chars().any(|c| shorts.contains(c)));
    }
    let letters = &flag[1..];
    !letters.is_empty()
        && letters.chars().all(|letter| {
            shorts.contains(letter)
                || FLAG_EQUIVALENTS
                    .iter()
                    .filter(|(p, _, short)| *p == program && short.contains(letter))
                    .any(|(_, long, short)| {
                        has_long(long) || short.chars().any(|c| shorts.contains(c))
                    })
        })
}
fn risk_name(risk: RiskLevel) -> &'static str {
    match risk {
        RiskLevel::Read => "read",
        RiskLevel::Write => "write",
        RiskLevel::Execute => "execute",
        RiskLevel::Network => "network",
        RiskLevel::Destructive => "destructive",
    }
}

struct ConfiguredHook {
    mode: ApprovalMode,
    policy: ApprovalPolicy,
    interactive: Arc<dyn ApprovalHook>,
    grants: Arc<ChatGrants>,
}
impl ApprovalHook for ConfiguredHook {
    fn denied_by_rule(&self, action: &ToolAction) -> bool {
        self.rule_denies(action)
    }

    fn approve(&self, action: &ToolAction) -> Approval {
        if action.tool == crate::tools::UNSANDBOXED_TOOL {
            return self.approve_escape(action);
        }
        if self.mode == ApprovalMode::Yolo {
            return Approval::AllowOnce;
        }
        // Browser permissions are kept in Settings › Browser, not chat grants: an "always"
        // answer goes back to the browser tool, which saves it as a site exception.
        let browser = action.tool.starts_with("browser_");
        // Rules decide first, so a deny rule always beats a chat grant.
        match self.policy.decision(action) {
            PolicyAction::Allow => Approval::AllowOnce,
            PolicyAction::Deny => Approval::Deny,
            PolicyAction::Ask => {
                if !browser && self.grants.covers(action) {
                    return Approval::AllowOnce;
                }
                match self.interactive.approve(action) {
                    Approval::AllowAlways if browser => Approval::AllowAlways,
                    Approval::AllowAlways => {
                        // Destructive actions are allowed this once only.
                        self.grants.remember(action);
                        Approval::AllowOnce
                    }
                    decision => decision,
                }
            }
        }
    }
}
impl ConfiguredHook {
    fn rule_denies(&self, action: &ToolAction) -> bool {
        self.mode == ApprovalMode::Custom && self.policy.decision(action) == PolicyAction::Deny
    }

    /// Running a sandbox-blocked command outside the sandbox needs a person:
    /// a dialog answer, a chat grant they made for `shell_unsandboxed`, or a
    /// rule naming `shell_unsandboxed` exactly. Wildcard, prefix and risk-only
    /// allow rules never cover it (deny rules of any shape still deny), the
    /// default action is never an allow, and YOLO (which never asks) denies.
    fn approve_escape(&self, action: &ToolAction) -> Approval {
        if self.mode == ApprovalMode::Yolo {
            return Approval::Deny;
        }
        match self.policy.escape_decision(action) {
            PolicyAction::Deny => return Approval::Deny,
            PolicyAction::Allow if action.grantable() => return Approval::AllowOnce,
            _ => {}
        }
        if self.grants.covers(action) {
            return Approval::AllowOnce;
        }
        match self.interactive.approve(action) {
            Approval::AllowAlways => {
                self.grants.remember(action);
                Approval::AllowOnce
            }
            decision => decision,
        }
    }
}

impl ApprovalPolicy {
    /// Decision for a `shell_unsandboxed` escape: the first deny rule that
    /// matches (normal matching, wildcards included) or the first allow/ask
    /// rule whose tool is exactly `shell_unsandboxed`, in order; otherwise ask
    /// (a `deny` default denies, an `allow` default still asks).
    pub fn escape_decision(&self, action: &ToolAction) -> PolicyAction {
        for rule in &self.rules {
            let exact = rule.tool == crate::tools::UNSANDBOXED_TOOL;
            if (rule.action == PolicyAction::Deny || exact) && rule_matches(rule, action) {
                return rule.action;
            }
        }
        if self.default == PolicyAction::Deny {
            PolicyAction::Deny
        } else {
            PolicyAction::Ask
        }
    }
}

/// Wrap the existing UI approval hook; automatic decisions are never cached by tool name.
/// Grants made through it last as long as the hook (one run).
pub fn configured_hook(
    mode: ApprovalMode,
    policy: ApprovalPolicy,
    interactive: Arc<dyn ApprovalHook>,
) -> Arc<dyn ApprovalHook> {
    configured_hook_with_grants(mode, policy, interactive, ChatGrants::in_memory())
}

/// Like [`configured_hook`], with the chat's grants: stored grants are honored
/// where the policy asks, and an "allow in this chat" answer is saved to them.
pub fn configured_hook_with_grants(
    mode: ApprovalMode,
    policy: ApprovalPolicy,
    interactive: Arc<dyn ApprovalHook>,
    grants: Arc<ChatGrants>,
) -> Arc<dyn ApprovalHook> {
    Arc::new(ConfiguredHook {
        mode,
        policy,
        interactive,
        grants,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CompactionConfig {
    pub timeout_seconds: u64,
    pub request_timeout_seconds: u64,
    /// Overall attachment preparation bound, independent of each document's job.
    pub document_batch_timeout_seconds: u64,
    /// Initial summary HTTP 429 backoff; subsequent retries double this floor.
    pub rate_limit_backoff_seconds: u64,
    /// Internal full-document preparation input capacity; ordinary work keeps its own budget.
    pub document_context_token_budget: usize,
    pub maintenance_threshold_ratio: f64,
    pub semantic_threshold_ratio: f64,
    pub target_headroom_interactions: usize,
    pub max_stages: usize,
    pub retry_limit: u8,
    pub section_max_tokens: u32,
    pub summary_max_tokens: u32,
    /// Explicit internal routing only; absent retains ordinary-model-based routing. Any model
    /// a connected provider offers (`<provider>/<id>` for one from `providers`); a run checks
    /// it against the captured catalogs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_file: Option<String>,
    /// Resolved before a run starts; never accepted from JSON.
    #[serde(skip)]
    pub prompt: Option<String>,
}
impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            timeout_seconds: 900,
            request_timeout_seconds: 300,
            document_batch_timeout_seconds: 3600,
            rate_limit_backoff_seconds: 10,
            document_context_token_budget: 200000,
            maintenance_threshold_ratio: 0.83,
            semantic_threshold_ratio: 0.93,
            target_headroom_interactions: 6,
            max_stages: 32,
            retry_limit: 2,
            section_max_tokens: 4096,
            summary_max_tokens: 8192,
            summary_model: None,
            prompt_file: None,
            prompt: None,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LoggingConfig {
    pub level: String,
    pub context_metrics: ContextMetricsLevel,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextMetricsLevel {
    Off,
    #[default]
    Summary,
    Detailed,
}
impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "info".into(),
            context_metrics: ContextMetricsLevel::Summary,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SkillConfig {
    pub paths: Vec<String>,
}
impl Default for SkillConfig {
    fn default() -> Self {
        Self {
            paths: [".agents/skills", ".codex/skills", ".claude/skills"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
        }
    }
}

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
            open_workspace: true,
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
    /// Open, navigate, click, type, screenshots, recording and tabs.
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
fn check_web_restriction(layer: &Value, origin: &str) -> Result<()> {
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
    for field in ["enabled", "developer_mode", "show_secrets"] {
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
fn tighten_browser_layer(current: &BrowserConfig, layer: &mut Value, origin: &str) -> Result<()> {
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

fn validate_web(config: &RuntimeConfig) -> Result<()> {
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RuntimeConfig {
    pub skills: SkillConfig,
    pub instructions: Vec<String>,
    pub logging: LoggingConfig,
    #[serde(rename = "$schema", skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    pub schema_version: u32,
    pub approval: ApprovalPolicy,
    /// OS sandbox for commands the agent runs; see [`crate::sandbox`].
    pub sandbox: crate::sandbox::SandboxConfig,
    pub compaction: CompactionConfig,
    /// `web_search` backend; see [`WebConfig`].
    pub web: WebConfig,
    /// The built-in browser; see [`BrowserConfig`].
    pub browser: BrowserConfig,
    /// OpenAI-compatible providers besides OpenCode Go; user file only.
    pub providers: Vec<ProviderDefinition>,
    /// Per-model overrides keyed by model id or `<provider>/<id>`.
    pub models: BTreeMap<String, ModelOverride>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_token_budget: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recent_messages: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_turns: Option<usize>,
}
impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            skills: SkillConfig::default(),
            instructions: Vec::new(),
            logging: LoggingConfig::default(),
            schema: None,
            schema_version: 1,
            approval: ApprovalPolicy::default(),
            sandbox: crate::sandbox::SandboxConfig::default(),
            compaction: CompactionConfig::default(),
            web: WebConfig::default(),
            browser: BrowserConfig::default(),
            providers: Vec::new(),
            models: BTreeMap::new(),
            context_token_budget: None,
            recent_messages: None,
            total_turns: None,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedConfig {
    pub config: RuntimeConfig,
    pub provenance: BTreeMap<String, String>,
    /// Where each of `config.approval.rules` came from, in the same order:
    /// `default` (built-in), `user`, `project` or `run`.
    #[serde(default)]
    pub rule_sources: Vec<String>,
}

pub fn parse(content: &str) -> Result<Value> {
    let bytes = strip_jsonc(content)?;
    let value: Value = serde_json::from_slice(&bytes).context("invalid configuration JSONC")?;
    ensure!(value.is_object(), "configuration must be an object");
    Ok(value)
}

/// `content` (a JSONC configuration) with its top-level `key` set to `value`. Everything
/// else, comments included, is left as written; only the old value of `key` (with any
/// comments inside it) is replaced. Settings use this for field-level writes.
pub fn set_top_level(content: &str, key: &str, value: &Value) -> Result<String> {
    let rendered = serde_json::to_string_pretty(value)?.replace('\n', "\n  ");
    if content.trim().is_empty() {
        return Ok(format!(
            "{{\n  {}: {rendered}\n}}\n",
            serde_json::to_string(key)?
        ));
    }
    parse(content)?;
    let bytes = strip_jsonc(content)?;
    let (mut depth, mut i) = (0usize, 0usize);
    let mut found: Option<(usize, usize)> = None;
    let mut last_close = None;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => {
                let start = i;
                i += 1;
                while i < bytes.len() && bytes[i] != b'"' {
                    i += if bytes[i] == b'\\' { 2 } else { 1 };
                }
                let end = i;
                i += 1;
                if depth != 1 {
                    continue;
                }
                let mut after = i;
                while after < bytes.len() && bytes[after].is_ascii_whitespace() {
                    after += 1;
                }
                if bytes.get(after) != Some(&b':') {
                    continue;
                }
                let name: String = serde_json::from_slice(&bytes[start..=end])?;
                let mut value_start = after + 1;
                while bytes[value_start].is_ascii_whitespace() {
                    value_start += 1;
                }
                // The value ends at the next ',' or '}' back at this depth.
                let (mut inner, mut j, mut in_string) = (0usize, value_start, false);
                while j < bytes.len() {
                    let c = bytes[j];
                    if in_string {
                        if c == b'\\' {
                            j += 1;
                        } else if c == b'"' {
                            in_string = false;
                        }
                    } else if c == b'"' {
                        in_string = true;
                    } else if c == b'{' || c == b'[' {
                        inner += 1;
                    } else if c == b'}' || c == b']' {
                        if inner == 0 {
                            break;
                        }
                        inner -= 1;
                    } else if c == b',' && inner == 0 {
                        break;
                    }
                    j += 1;
                }
                let mut value_end = j;
                while value_end > value_start && bytes[value_end - 1].is_ascii_whitespace() {
                    value_end -= 1;
                }
                if name == key {
                    found = Some((value_start, value_end));
                }
                i = j;
            }
            b'{' | b'[' => {
                depth += 1;
                i += 1;
            }
            b'}' | b']' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    last_close = Some(i);
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    let updated = if let Some((start, end)) = found {
        format!("{}{rendered}{}", &content[..start], &content[end..])
    } else {
        let close = last_close.context("configuration must be an object")?;
        let before = bytes[..close]
            .iter()
            .rposition(|c| !c.is_ascii_whitespace());
        let entry = format!("{}: {rendered}", serde_json::to_string(key)?);
        match before {
            // An object with no settings yet: the entry goes first, comments stay after it.
            Some(position) if bytes[position] == b'{' => format!(
                "{}\n  {entry},{}",
                &content[..=position],
                &content[position + 1..]
            ),
            Some(position) => format!(
                "{},\n  {entry}{}",
                &content[..=position],
                &content[position + 1..]
            ),
            None => bail_unbalanced()?,
        }
    };
    let check = parse(&updated).context("updated configuration is not valid JSONC")?;
    ensure!(
        check.get(key) == Some(value),
        "updated configuration lost the new value"
    );
    Ok(updated)
}

fn bail_unbalanced() -> Result<String> {
    anyhow::bail!("configuration must be an object")
}

/// Every runtime setting with its allowed values and default, as commented JSONC. A new
/// user `runtime.jsonc` carries it with every setting commented out.
pub const REFERENCE: &str = include_str!("configuration_reference.jsonc");

/// Parses JSONC (comments, trailing commas) of at most 1 MiB; an error carries the 1-based
/// line it was found on (0 when the input is too large).
pub fn parse_jsonc(content: &str) -> std::result::Result<Value, (String, usize)> {
    let bytes = strip_jsonc(content).map_err(|e| (e.to_string(), 0))?;
    serde_json::from_slice(&bytes).map_err(|e| {
        let message = e.to_string();
        let message = message
            .split(" at line ")
            .next()
            .unwrap_or(&message)
            .to_owned();
        (message, e.line())
    })
}

/// JSONC text as plain JSON bytes: comments and trailing commas become spaces,
/// so byte offsets, lines and columns stay those of the original file.
fn strip_jsonc(content: &str) -> Result<Vec<u8>> {
    ensure!(content.len() <= 1024 * 1024, "configuration exceeds 1 MiB");
    let mut bytes = content.as_bytes().to_vec();
    let mut i = 0;
    let mut quoted = false;
    let mut escaped = false;
    while i < bytes.len() {
        if quoted {
            if escaped {
                escaped = false;
            } else if bytes[i] == b'\\' {
                escaped = true;
            } else if bytes[i] == b'"' {
                quoted = false;
            }
            i += 1;
            continue;
        }
        if bytes[i] == b'"' {
            quoted = true;
            i += 1;
            continue;
        }
        if bytes[i] == b'/' && bytes.get(i + 1) == Some(&b'/') {
            while i < bytes.len() && bytes[i] != b'\n' {
                bytes[i] = b' ';
                i += 1;
            }
            continue;
        }
        if bytes[i] == b'/' && bytes.get(i + 1) == Some(&b'*') {
            bytes[i] = b' ';
            bytes[i + 1] = b' ';
            i += 2;
            let mut closed = false;
            while i < bytes.len() {
                if bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/') {
                    bytes[i] = b' ';
                    bytes[i + 1] = b' ';
                    i += 2;
                    closed = true;
                    break;
                }
                if bytes[i] != b'\n' {
                    bytes[i] = b' ';
                }
                i += 1;
            }
            ensure!(closed, "unterminated configuration comment");
            continue;
        }
        i += 1;
    }
    quoted = false;
    escaped = false;
    for i in 0..bytes.len() {
        if quoted {
            if escaped {
                escaped = false;
            } else if bytes[i] == b'\\' {
                escaped = true;
            } else if bytes[i] == b'"' {
                quoted = false;
            }
            continue;
        }
        if bytes[i] == b'"' {
            quoted = true;
            continue;
        }
        if bytes[i] == b','
            && bytes[i + 1..]
                .iter()
                .find(|b| !b.is_ascii_whitespace())
                .is_some_and(|b| *b == b'}' || *b == b']')
        {
            bytes[i] = b' ';
        }
    }
    Ok(bytes)
}

/// A problem in one configuration file, with its position when known.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FileIssue {
    pub message: String,
    pub line: Option<usize>,
    pub column: Option<usize>,
}

/// Syntax and schema check of one configuration file on its own (comments,
/// JSON syntax, unknown fields, wrong types), with line and column.
pub fn check_file(content: &str) -> Option<FileIssue> {
    let bytes = match strip_jsonc(content) {
        Ok(bytes) => bytes,
        Err(error) => {
            return Some(FileIssue {
                message: format!("{error:#}"),
                line: None,
                column: None,
            })
        }
    };
    let positioned = |error: serde_json::Error| {
        Some(FileIssue {
            // The position is reported separately; drop serde's trailing copy.
            message: {
                let text = error.to_string();
                let suffix = format!(" at line {} column {}", error.line(), error.column());
                text.strip_suffix(&suffix)
                    .map(str::to_owned)
                    .unwrap_or(text)
            },
            line: Some(error.line()).filter(|line| *line > 0),
            column: (error.line() > 0).then_some(error.column()),
        })
    };
    let value: Value = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(error) => return positioned(error),
    };
    if !value.is_object() {
        return Some(FileIssue {
            message: "configuration must be an object".into(),
            line: Some(1),
            column: None,
        });
    }
    if let Err(error) = serde_json::from_slice::<RuntimeConfig>(&bytes) {
        return positioned(error);
    }
    None
}

/// Line of the setting a validation message names (`sandbox.backend must ...`
/// points at the line with `"backend"`), when the file mentions it.
pub fn locate_error_line(content: &str, message: &str) -> Option<usize> {
    let field = message
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '.'))
        .find(|word| word.contains('.') && word.chars().next().is_some_and(char::is_alphabetic))?;
    let key = field.rsplit('.').next().filter(|key| !key.is_empty())?;
    let quoted = format!("\"{key}\"");
    content
        .lines()
        .position(|line| line.contains(&quoted))
        .map(|index| index + 1)
}
fn merge(
    target: &mut Value,
    source: Value,
    origin: &str,
    path: &str,
    provenance: &mut BTreeMap<String, String>,
) {
    if let Some(right) = source.as_object() {
        if !target.is_object() {
            *target = Value::Object(Default::default());
        }
        let left = target.as_object_mut().expect("object initialized above");
        for (key, value) in right {
            let path = if path.is_empty() {
                key.clone()
            } else {
                format!("{path}.{key}")
            };
            merge(
                left.entry(key.clone()).or_insert(Value::Null),
                value.clone(),
                origin,
                &path,
                provenance,
            );
        }
    } else {
        *target = source;
        provenance.retain(|key, _| !key.starts_with(&format!("{path}.")));
        provenance.insert(path.into(), origin.into());
    }
}
pub fn resolve(
    user: Option<&str>,
    project: Option<&str>,
    explicit: Option<&str>,
) -> Result<ResolvedConfig> {
    let defaults = serde_json::to_value(RuntimeConfig::default())?;
    let mut value = Value::Object(Default::default());
    let mut provenance = BTreeMap::new();
    merge(&mut value, defaults, "default", "", &mut provenance);
    let mut rule_sources = vec!["default".to_owned(); ApprovalPolicy::default().rules.len()];
    for (origin, content) in [("user", user), ("project", project), ("run", explicit)] {
        if let Some(content) = content {
            let mut layer = parse(content)?;
            if origin != "user" {
                // Project and run layers may only tighten the sandbox.
                if let Some(sandbox) = layer.get("sandbox") {
                    let current: crate::sandbox::SandboxConfig =
                        serde_json::from_value(value["sandbox"].clone())
                            .context("invalid sandbox configuration")?;
                    crate::sandbox::check_restriction(&current, sandbox, origin)?;
                }
                check_web_restriction(&layer, origin)?;
                providers::check_provider_restriction(&layer, origin)?;
                let current: BrowserConfig = serde_json::from_value(value["browser"].clone())
                    .context("invalid browser configuration")?;
                tighten_browser_layer(&current, &mut layer, origin)?;
            }
            if origin == "project" {
                if let Some(policy) = layer.as_object_mut().and_then(|m| m.remove("approval")) {
                    let mut policy = policy;
                    ensure!(policy.is_object(), "project approval must be an object");
                    if policy.get("rules").is_none() {
                        policy["rules"] = serde_json::json!([]);
                    }
                    let policy: ApprovalPolicy = serde_json::from_value(policy)
                        .context("invalid project approval policy")?;
                    ensure!(
                        policy.default != PolicyAction::Allow
                            && policy.rules.iter().all(|r| r.action == PolicyAction::Deny),
                        "project approval policy may only add deny rules"
                    );
                    let mut current: ApprovalPolicy =
                        serde_json::from_value(value["approval"].clone())?;
                    let added =
                        policy.rules.len() + usize::from(policy.default == PolicyAction::Deny);
                    rule_sources.splice(0..0, std::iter::repeat_n("project".to_owned(), added));
                    let mut rules = policy.rules;
                    rules.extend(current.rules);
                    current.rules = rules;
                    if policy.default == PolicyAction::Deny {
                        current.rules.insert(
                            0,
                            ApprovalRule {
                                command: None,
                                tool: "*".into(),
                                action: PolicyAction::Deny,
                                risk: None,
                            },
                        );
                        current.default = PolicyAction::Deny;
                    }
                    value["approval"] = serde_json::to_value(current)?;
                    provenance.insert(
                        "approval.rules".into(),
                        "user + project restrictions".into(),
                    );
                    if policy.default == PolicyAction::Deny {
                        provenance.insert("approval.default".into(), "project".into());
                    }
                }
            }
            if let Some(rules) = layer
                .get("approval")
                .and_then(|approval| approval.get("rules"))
                .and_then(Value::as_array)
            {
                rule_sources = vec![origin.to_owned(); rules.len()];
            }
            merge(&mut value, layer, origin, "", &mut provenance);
        }
    }
    let config: RuntimeConfig =
        serde_json::from_value(value).context("invalid runtime configuration")?;
    validate(&config)?;
    rule_sources.resize(config.approval.rules.len(), "default".to_owned());
    Ok(ResolvedConfig {
        config,
        provenance,
        rule_sources,
    })
}
pub fn validate_relative_paths(paths: &[String]) -> Result<()> {
    ensure!(
        paths.len() <= 32,
        "at most 32 configured paths are supported"
    );
    for path in paths {
        ensure!(
            !path.is_empty() && path.len() <= 1024,
            "configured paths must be 1..1024 bytes"
        );
        ensure!(
            std::path::Path::new(path).components().all(|c| matches!(
                c,
                std::path::Component::Normal(_) | std::path::Component::CurDir
            )),
            "configured paths must remain relative without parent traversal"
        );
        ensure!(
            path.split(['/', '\\']).all(|part| part != "..")
                && !path.contains(':')
                && !path.starts_with('\\'),
            "configured paths must remain relative without parent traversal"
        );
    }
    Ok(())
}

pub fn validate(config: &RuntimeConfig) -> Result<()> {
    crate::sandbox::validate(&config.sandbox)?;
    validate_web(config)?;
    providers::validate(&config.providers, &config.models)?;
    validate_relative_paths(&config.skills.paths)?;
    validate_relative_paths(&config.instructions)?;
    ensure!(
        ["debug", "info", "warn", "error"].contains(&config.logging.level.as_str()),
        "logging.level must be debug/info/warn/error"
    );
    ensure!(
        config.schema_version == 1,
        "unsupported schema_version; expected 1"
    );
    ensure!(
        (1..=1800).contains(&config.compaction.timeout_seconds),
        "compaction.timeout_seconds must be 1..1800"
    );
    let compaction = &config.compaction;
    ensure!(
        (1..=21600).contains(&compaction.document_batch_timeout_seconds),
        "compaction.document_batch_timeout_seconds must be 1..21600"
    );
    ensure!(
        (1..=300).contains(&compaction.rate_limit_backoff_seconds),
        "compaction.rate_limit_backoff_seconds must be 1..300"
    );
    ensure!(
        (24000..=1000000).contains(&compaction.document_context_token_budget),
        "compaction.document_context_token_budget must be 24000..1000000"
    );
    ensure!(
        (1..=300).contains(&compaction.request_timeout_seconds),
        "compaction.request_timeout_seconds must be 1..300"
    );
    ensure!(
        compaction.maintenance_threshold_ratio.is_finite()
            && compaction.semantic_threshold_ratio.is_finite()
            && (0.5..=0.95).contains(&compaction.maintenance_threshold_ratio)
            && compaction.semantic_threshold_ratio > compaction.maintenance_threshold_ratio
            && compaction.semantic_threshold_ratio <= 0.97,
        "compaction thresholds require 0.5 <= maintenance < semantic <= 0.97"
    );
    ensure!(
        (1..=20).contains(&compaction.target_headroom_interactions),
        "compaction.target_headroom_interactions must be 1..20"
    );
    ensure!(
        (1..=128).contains(&compaction.max_stages),
        "compaction.max_stages must be 1..128"
    );
    ensure!(
        config.compaction.retry_limit <= 2,
        "compaction.retry_limit must be 0..2"
    );
    if let Some(model) = &compaction.summary_model {
        ensure!(
            valid_model_id(model),
            "compaction.summary_model must be a model id (<provider>/<id> for a model from providers)"
        );
    }
    for (name, tokens) in [
        ("section_max_tokens", config.compaction.section_max_tokens),
        ("summary_max_tokens", config.compaction.summary_max_tokens),
    ] {
        ensure!(
            (256..=16384).contains(&tokens),
            "compaction.{name} must be 256..16384"
        );
    }
    for (name, value, low, high) in [
        (
            "context_token_budget",
            config.context_token_budget,
            2000,
            256000,
        ),
        ("recent_messages", config.recent_messages, 2, 200),
        ("total_turns", config.total_turns, 1, 2000),
    ] {
        if let Some(value) = value {
            ensure!(
                (low..=high).contains(&value),
                "{name} must be {low}..{high}"
            );
        }
    }
    ensure!(
        config.approval.rules.len() <= 256,
        "approval.rules exceeds 256 rules"
    );
    for rule in &config.approval.rules {
        let stem = rule.tool.strip_suffix('*').unwrap_or(&rule.tool);
        ensure!(
            !rule.tool.is_empty()
                && stem
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.'),
            "approval rule tool must be an exact name or trailing-star prefix"
        );
        if let Some(command) = &rule.command {
            if crate::tools::is_site_tool(&rule.tool) || rule.tool == "browser_*" {
                ensure!(
                    command.len() == 1 && valid_site(&command[0]),
                    "approval rule command for a web or browser tool must be one site, such as [\"example.com\"]"
                );
            } else {
                ensure!(
                    rule.tool == "shell" || rule.tool == crate::tools::UNSANDBOXED_TOOL,
                    "approval rule command is only valid with tool \"shell\" or \"shell_unsandboxed\", or one site with a web or browser tool"
                );
            }
            ensure!(
                !command.is_empty() && command.len() <= 16,
                "approval rule command must have 1..16 tokens"
            );
            ensure!(
                command.iter().all(|t| !t.is_empty() && t.len() <= 256),
                "approval rule command tokens must be 1..256 bytes"
            );
        }
        if let Some(risk) = &rule.risk {
            ensure!(
                ["read", "write", "execute", "network", "destructive"].contains(&risk.as_str()),
                "invalid approval rule risk"
            );
        }
    }
    Ok(())
}
/// Load a bounded prompt relative to the file that declares it; root containment is enforced.
pub fn load_prompt(config: &mut RuntimeConfig, directory: &std::path::Path) -> Result<()> {
    if let Some(path) = &config.compaction.prompt_file {
        let root = directory.canonicalize()?;
        let path = root
            .join(path)
            .canonicalize()
            .context("compaction.prompt_file is unavailable")?;
        ensure!(
            path.starts_with(root),
            "compaction.prompt_file escapes its configuration directory"
        );
        ensure!(
            std::fs::metadata(&path)?.len() <= 32768,
            "compaction.prompt_file exceeds 32 KiB"
        );
        config.compaction.prompt = Some(std::fs::read_to_string(path)?);
    }
    Ok(())
}

/// Read declared repository guidance, bounded and confined to the project root.
pub fn load_instructions(config: &RuntimeConfig, project: &std::path::Path) -> Result<String> {
    validate_relative_paths(&config.instructions)?;
    if config.instructions.is_empty() {
        return Ok(String::new());
    }
    let root = project.canonicalize()?;
    let mut text = String::new();
    for reference in &config.instructions {
        let path = root
            .join(reference)
            .canonicalize()
            .with_context(|| format!("instruction file {reference} is unavailable"))?;
        ensure!(
            path.starts_with(&root),
            "instruction file escapes the project root"
        );
        ensure!(
            std::fs::metadata(&path)?.len() <= 65536,
            "instruction file exceeds 64 KiB"
        );
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("instruction file {reference} is not UTF-8 text"))?;
        let heading = crate::prompts::fill(
            crate::prompts::RUNTIME_REPOSITORY_GUIDANCE_FILE,
            &[("reference", reference)],
        );
        text.push_str(&format!("\n{heading}\n{content}\n"));
        ensure!(
            text.len() <= 65536,
            "combined instruction guidance exceeds 64 KiB"
        );
    }
    Ok(text)
}

/// Editor schema for the same configuration accepted by the runtime.
pub fn schema() -> Value {
    let mut schema = base_schema();
    let (providers, models) = providers::schema();
    schema["properties"]["providers"] = providers;
    schema["properties"]["models"] = models;
    schema
}

fn base_schema() -> Value {
    serde_json::json!({
        "$schema":"https://json-schema.org/draft/2020-12/schema",
        "title":"Themis runtime configuration","type":"object","additionalProperties":false,
        "properties":{
            "skills":{"type":"object","additionalProperties":false,"properties":{"paths":{"type":"array","maxItems":32,"items":{"type":"string","minLength":1,"maxLength":1024},"default":[".agents/skills",".codex/skills",".claude/skills"]}}},
            "instructions":{"type":"array","maxItems":32,"items":{"type":"string","minLength":1,"maxLength":1024}},
            "logging":{"type":"object","additionalProperties":false,"properties":{"level":{"enum":["debug","info","warn","error"],"default":"info"},"context_metrics":{"enum":["off","summary","detailed"],"default":"summary"}}},
        "$schema":{"type":"string"},"schema_version":{"const":1,"default":1},
            "context_token_budget":{"type":"integer","minimum":2000,"maximum":256000},
            "recent_messages":{"type":"integer","minimum":2,"maximum":200},
            "total_turns":{"type":"integer","minimum":1,"maximum":2000},
            "approval":{"type":"object","additionalProperties":false,"properties":{
                "default":{"enum":["allow","ask","deny"],"default":"ask"},
                "rules":{"type":"array","maxItems":256,"items":{"type":"object","additionalProperties":false,"required":["tool","action"],"properties":{
                    "tool":{"type":"string","pattern":"^([A-Za-z0-9_.-]+\\*?|\\*)$"},
                    "action":{"enum":["allow","ask","deny"]},
                    "risk":{"enum":["read","write","execute","network","destructive"]},
                    "command":{"type":"array","minItems":1,"maxItems":16,"items":{"type":"string","minLength":1,"maxLength":256}}
                }}}
            }},
            "sandbox":{"type":"object","additionalProperties":false,"properties":{
                "enabled":{"type":"boolean","default":true},
                "backend":{"enum":["auto","seatbelt","bubblewrap","landlock"],"default":"auto"},
                "network":{"enum":["allow","block"],"default":"allow"},
                "writable_paths":{"type":"array","maxItems":32,"items":{"type":"string","minLength":1,"maxLength":1024},"default":[]},
                "allow_keychain":{"type":"boolean","default":false,"description":"macOS: let sandboxed commands use the Keychain; projects may only set false"}
            }},
            "web":{"type":"object","additionalProperties":false,"properties":{
                "search_url":{"type":"string","maxLength":1024,"description":"Your SearXNG instance (JSON output enabled); user file only"}
            }},
            "browser":{"type":"object","additionalProperties":false,"properties":{
                "enabled":{"type":"boolean","default":true},
                "executable":{"type":"string","maxLength":1024,"description":"Chrome, Chromium or Edge binary; user file only"},
                "developer_mode":{"type":"boolean","default":true,"description":"Enable full CDP access: the agent's debugging tools"},
                "permissions":{"type":"object","additionalProperties":false,"properties":{
                    "browsing":{"enum":["allow","ask","block"],"default":"allow"},
                    "downloads":{"enum":["allow","ask","block"],"default":"allow"},
                    "debugging":{"enum":["allow","ask","block"],"default":"block"}
                }},
                "site_exceptions":{"type":"array","maxItems":500,"items":{"type":"object","additionalProperties":false,"required":["site"],"properties":{
                    "site":{"type":"string","minLength":1,"maxLength":253},
                    "browsing":{"enum":["allow","ask","block"]},
                    "downloads":{"enum":["allow","ask","block"]},
                    "debugging":{"enum":["allow","ask","block"]}
                }}},
                "show_secrets":{"type":"boolean","default":false},
                "downloads_directory":{"type":"string","maxLength":1024,"description":"Absolute folder for downloads; user file only"},
                "open_workspace":{"type":"boolean","default":true}
            }},
            "compaction":{"type":"object","additionalProperties":false,"properties":{
                "timeout_seconds":{"type":"integer","minimum":1,"maximum":1800,"default":900},
                "request_timeout_seconds":{"type":"integer","minimum":1,"maximum":300,"default":300},
                "document_batch_timeout_seconds":{"type":"integer","minimum":1,"maximum":21600,"default":3600},
                "rate_limit_backoff_seconds":{"type":"integer","minimum":1,"maximum":300,"default":10},
                "document_context_token_budget":{"type":"integer","minimum":24000,"maximum":1000000,"default":200000},
                "maintenance_threshold_ratio":{"type":"number","minimum":0.5,"maximum":0.95,"default":0.83},
                "semantic_threshold_ratio":{"type":"number","exclusiveMinimum":0.5,"maximum":0.97,"default":0.93},
                "target_headroom_interactions":{"type":"integer","minimum":1,"maximum":20,"default":6},
                "max_stages":{"type":"integer","minimum":1,"maximum":128,"default":32},
                "retry_limit":{"type":"integer","minimum":0,"maximum":2,"default":2},
                "section_max_tokens":{"type":"integer","minimum":256,"maximum":16384,"default":4096},
                "summary_max_tokens":{"type":"integer","minimum":256,"maximum":16384,"default":8192},
                "summary_model":{"type":["string","null"],"minLength":1,"maxLength":256,"default":null,"description":"Any model a connected provider offers; <provider>/<id> for one from providers"},
                "prompt_file":{"type":"string"}
            }}
        }
    })
}

#[cfg(test)]
mod tests {

    /// The reference is valid, changes nothing (its values are the defaults) and names every
    /// field, so a new setting cannot be added without documenting it there.
    #[test]
    fn set_top_level_keeps_comments_of_a_file_without_settings() {
        let content = "// header\n{\n  // \"sandbox\": { \"network\": \"block\" },\n}\n";
        let updated =
            set_top_level(content, "browser", &serde_json::json!({ "enabled": false })).unwrap();
        assert!(updated.contains("// \"sandbox\": { \"network\": \"block\" },"));
        assert_eq!(
            parse(&updated).unwrap()["browser"]["enabled"],
            serde_json::json!(false)
        );
        let bare = set_top_level("{}", "web", &serde_json::json!({})).unwrap();
        assert_eq!(parse(&bare).unwrap()["web"], serde_json::json!({}));
    }
    #[test]
    fn the_reference_is_valid_and_lists_every_setting() {
        let resolved = resolve(Some(REFERENCE), None, None).expect("reference resolves");
        let reference = serde_json::to_value(&resolved.config).unwrap();
        let defaults = serde_json::to_value(RuntimeConfig::default()).unwrap();
        assert_eq!(reference, defaults, "the reference must show the defaults");
        // Optional fields shown commented out are named in the text.
        let text = REFERENCE;
        // Every field the configuration has (from its defaults), plus the optional ones that
        // have no default value, must be named in the reference.
        fn fields(value: &Value, out: &mut Vec<String>) {
            match value {
                Value::Object(object) => {
                    for (name, inner) in object {
                        out.push(name.clone());
                        fields(inner, out);
                    }
                }
                Value::Array(items) => items.iter().for_each(|item| fields(item, out)),
                _ => {}
            }
        }
        let mut names = vec![
            "executable",
            "downloads_directory",
            "search_url",
            "context_token_budget",
            "recent_messages",
            "total_turns",
            "summary_model",
            "prompt_file",
            "command",
            "downloads",
            "debugging",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
        fields(&defaults, &mut names);
        for name in names
            .iter()
            .filter(|name| name.as_str() != "site_restrictions")
        {
            assert!(
                text.contains(&format!("\"{name}\"")),
                "the reference does not mention \"{name}\""
            );
        }
        // The schema editors use knows the same fields.
        let schema = schema().to_string();
        for name in &names {
            assert!(
                schema.contains(&format!("\"{name}\"")),
                "the schema does not know \"{name}\""
            );
        }
    }

    use super::*;

    #[test]
    fn web_and_browser_settings_come_from_the_user_file_and_projects_may_only_disable() {
        // An absolute path on this OS.
        let chrome = if cfg!(windows) {
            r"C:\\Chrome\\chrome.exe"
        } else {
            "/opt/chrome"
        };
        let user = format!(
            r#"{{"web":{{"search_url":"http://127.0.0.1:8888"}},"browser":{{"executable":"{chrome}"}}}}"#
        );
        let user = user.as_str();
        let resolved = resolve(Some(user), Some(r#"{"browser":{"enabled":false}}"#), None).unwrap();
        assert_eq!(
            resolved.config.web.search_url.as_deref(),
            Some("http://127.0.0.1:8888")
        );
        assert!(!resolved.config.browser.enabled);
        assert_eq!(resolved.provenance["browser.enabled"], "project");
        assert!(RuntimeConfig::default().browser.enabled);
        for project in [
            r#"{"web":{"search_url":"https://evil.example"}}"#,
            r#"{"browser":{"executable":"/tmp/x"}}"#,
            r#"{"browser":{"enabled":true}}"#,
        ] {
            assert!(
                resolve(Some(user), Some(project), None).is_err(),
                "{project}"
            );
            assert!(
                resolve(None, None, Some(project)).is_err(),
                "run: {project}"
            );
        }
        for bad in [
            r#"{"web":{"search_url":"ftp://x"}}"#,
            r#"{"web":{"search_url":"http://u:p@x"}}"#,
            r#"{"web":{"search_url":"not a url"}}"#,
            r#"{"browser":{"executable":"chrome"}}"#,
            r#"{"web":{"other":1}}"#,
        ] {
            assert!(resolve(Some(bad), None, None).is_err(), "{bad}");
        }
        assert!(schema()["properties"]["web"]["properties"]["search_url"].is_object());
    }

    #[test]
    fn site_rules_match_the_host_and_its_subdomains() {
        let action = |tool: &str, host: &str| ToolAction {
            display: None,
            command: vec![host.into()],
            tool: tool.into(),
            summary: String::new(),
            risk: RiskLevel::Network,
        };
        let resolved = resolve(
            Some(r#"{"approval":{"rules":[{"tool":"fetch_url","command":["docs.rs"],"action":"allow"}]}}"#),
            Some(r#"{"approval":{"rules":[{"tool":"browser_*","command":["Example.com"],"action":"deny"}]}}"#),
            None,
        )
        .unwrap();
        let policy = &resolved.config.approval;
        assert_eq!(
            policy.decision(&action("browser_click", "example.com")),
            PolicyAction::Deny
        );
        assert_eq!(
            policy.decision(&action("browser_open", "login.example.com")),
            PolicyAction::Deny
        );
        assert_eq!(
            policy.decision(&action("browser_open", "notexample.com")),
            PolicyAction::Ask
        );
        assert_eq!(
            policy.decision(&action("fetch_url", "docs.rs")),
            PolicyAction::Allow
        );
        assert_eq!(
            policy.decision(&action("fetch_url", "evil-docs.rs")),
            PolicyAction::Ask
        );
        assert!(site_matches("A.Example.COM.", "example.com"));
        assert!(!site_matches("example.com", ""));
        for bad in [
            r#"{"tool":"fetch_url","command":["https://docs.rs"],"action":"deny"}"#,
            r#"{"tool":"fetch_url","command":["a.com","b.com"],"action":"deny"}"#,
            r#"{"tool":"fetch_url","command":["*.a.com"],"action":"deny"}"#,
            r#"{"tool":"read_file","command":["a.com"],"action":"deny"}"#,
        ] {
            let file = format!(r#"{{"approval":{{"rules":[{bad}]}}}}"#);
            assert!(resolve(Some(&file), None, None).is_err(), "{bad}");
        }
    }

    #[test]
    fn set_top_level_replaces_one_value_and_keeps_every_comment() {
        let file = "{\n  // my model\n  \"schema_version\": 1, /* keep */\n  \"browser\": { \"enabled\": true } // old\n}\n";
        let browser = serde_json::json!({"enabled": false, "show_secrets": true});
        let updated = set_top_level(file, "browser", &browser).unwrap();
        assert!(
            updated.contains("// my model")
                && updated.contains("/* keep */")
                && updated.contains("// old"),
            "{updated}"
        );
        assert_eq!(parse(&updated).unwrap()["browser"], browser);
        assert_eq!(parse(&updated).unwrap()["schema_version"], 1);
        // Absent key: appended, with or without other keys, and to an empty file.
        let added = set_top_level("{ \"schema_version\": 1, }", "browser", &browser).unwrap();
        assert_eq!(parse(&added).unwrap()["browser"], browser);
        assert_eq!(parse(&added).unwrap()["schema_version"], 1);
        assert_eq!(
            parse(&set_top_level("{}", "browser", &browser).unwrap()).unwrap()["browser"],
            browser
        );
        assert_eq!(
            parse(&set_top_level("", "browser", &browser).unwrap()).unwrap()["browser"],
            browser
        );
        // A nested key with the same name is not the top-level one.
        let nested = "{\"web\": {\"browser\": 1}}";
        let updated = set_top_level(nested, "browser", &browser).unwrap();
        assert_eq!(parse(&updated).unwrap()["web"]["browser"], 1);
        assert_eq!(parse(&updated).unwrap()["browser"], browser);
        assert!(set_top_level("[1]", "browser", &browser).is_err());
    }

    #[test]
    fn browser_permissions_default_and_projects_may_only_tighten() {
        let resolved = resolve(None, None, None).unwrap().config.browser;
        assert!(resolved.developer_mode && resolved.open_workspace && !resolved.show_secrets);
        assert_eq!(resolved.permissions.debugging, SiteAccess::Block);
        assert_eq!(resolved.site_exceptions.len(), STARTER_BLOCKED_SITES.len());
        // The user's list replaces the starter list, so entries can be removed.
        let user = r#"{"browser":{"site_exceptions":[{"site":"example.com","debugging":"allow"}],"permissions":{"debugging":"ask"}}}"#;
        let resolved = resolve(Some(user), None, None).unwrap().config.browser;
        assert_eq!(resolved.site_exceptions.len(), 1);
        assert_eq!(resolved.permissions.debugging, SiteAccess::Ask);
        assert_eq!(
            resolved.permissions.browsing,
            SiteAccess::Allow,
            "unset fields keep their default"
        );
        // A project may block more; its exceptions are kept apart and do not replace the user's.
        let project = r#"{"browser":{"site_exceptions":[{"site":"admin.example.com","browsing":"block"}],"permissions":{"downloads":"block"},"developer_mode":false}}"#;
        let resolved = resolve(Some(user), Some(project), None)
            .unwrap()
            .config
            .browser;
        assert_eq!(resolved.site_exceptions.len(), 1);
        assert_eq!(resolved.site_restrictions[0].site, "admin.example.com");
        assert_eq!(resolved.permissions.downloads, SiteAccess::Block);
        assert!(!resolved.developer_mode);
        // It may not loosen anything.
        for loosen in [
            r#"{"browser":{"permissions":{"debugging":"allow"}}}"#,
            r#"{"browser":{"site_exceptions":[{"site":"paypal.com","browsing":"allow"}]}}"#,
            r#"{"browser":{"developer_mode":true}}"#,
            r#"{"browser":{"show_secrets":true}}"#,
            r#"{"browser":{"downloads_directory":"/tmp"}}"#,
            r#"{"browser":{"open_workspace":false}}"#,
            r#"{"browser":{"site_restrictions":[]}}"#,
        ] {
            assert!(resolve(Some(user), Some(loosen), None).is_err(), "{loosen}");
        }
        // Sites are lowercase hosts.
        for bad in ["https://example.com", "Example.com", "example.com/path", ""] {
            let file = format!(
                r#"{{"browser":{{"site_exceptions":[{{"site":"{bad}","browsing":"block"}}]}}}}"#
            );
            assert!(resolve(Some(&file), None, None).is_err(), "{bad}");
        }
        assert!(resolve(
            Some(r#"{"browser":{"downloads_directory":"relative"}}"#),
            None,
            None
        )
        .is_err());
    }

    #[test]
    fn summary_model_override_follows_precedence_and_can_be_cleared() {
        let user = r#"{"compaction":{"summary_model":"gpt-5.6-luna"}}"#;
        let project = r#"{"compaction":{"summary_model":"gpt-6-luna"}}"#;
        let resolved = resolve(Some(user), Some(project), None).unwrap();
        assert_eq!(
            serde_json::to_value(&resolved.config).unwrap()["compaction"]["summary_model"],
            "gpt-6-luna"
        );
        assert_eq!(resolved.provenance["compaction.summary_model"], "project");
        let cleared = resolve(
            Some(user),
            Some(project),
            Some(r#"{"compaction":{"summary_model":null}}"#),
        )
        .unwrap();
        assert!(
            serde_json::to_value(cleared.config).unwrap()["compaction"]["summary_model"].is_null()
        );
        assert_eq!(cleared.provenance["compaction.summary_model"], "run");
        // Any model id is accepted here; a run checks it against the captured catalogs.
        for model in ["step-5-preview-free", "local/qwen/coder"] {
            assert!(resolve(
                Some(&format!(
                    r#"{{"compaction":{{"summary_model":"{model}"}}}}"#
                )),
                None,
                None
            )
            .is_ok());
        }
        for model in ["", "has space", "line\\nbreak"] {
            assert!(resolve(
                Some(&format!(
                    r#"{{"compaction":{{"summary_model":"{model}"}}}}"#
                )),
                None,
                None
            )
            .is_err());
        }
    }
    #[test]
    fn document_batch_and_summary_backoff_have_bounded_independent_configuration() {
        let mut config = RuntimeConfig::default();
        assert_eq!(config.compaction.document_batch_timeout_seconds, 3600);
        assert_eq!(config.compaction.rate_limit_backoff_seconds, 10);
        for invalid in [0, 21601] {
            config.compaction.document_batch_timeout_seconds = invalid;
            assert!(validate(&config)
                .unwrap_err()
                .to_string()
                .contains("document_batch_timeout_seconds"));
        }
        config.compaction.document_batch_timeout_seconds = 21600;
        for invalid in [0, 301] {
            config.compaction.rate_limit_backoff_seconds = invalid;
            assert!(validate(&config)
                .unwrap_err()
                .to_string()
                .contains("rate_limit_backoff_seconds"));
        }
        config.compaction.rate_limit_backoff_seconds = 300;
        validate(&config).unwrap();
        let schema = schema();
        assert_eq!(
            schema["properties"]["compaction"]["properties"]["document_batch_timeout_seconds"]
                ["maximum"],
            21600
        );
        assert_eq!(
            schema["properties"]["compaction"]["properties"]["rate_limit_backoff_seconds"]
                ["default"],
            10
        );
    }
    #[test]
    fn document_input_capacity_is_validated_independently_of_ordinary_context() {
        let mut config = RuntimeConfig {
            context_token_budget: Some(24000),
            compaction: CompactionConfig {
                document_context_token_budget: 1000000,
                ..Default::default()
            },
            ..Default::default()
        };
        validate(&config).unwrap();
        assert_eq!(config.context_token_budget, Some(24000));
        config.compaction.document_context_token_budget = 1000001;
        assert!(validate(&config)
            .unwrap_err()
            .to_string()
            .contains("document_context_token_budget"));
        config.compaction.document_context_token_budget = 23999;
        assert!(validate(&config).is_err());
    }

    #[test]
    fn adaptive_context_configuration_accepts_256k_and_validates_shared_controls() {
        let source = r#"{"context_token_budget":256000,"compaction":{"maintenance_threshold_ratio":0.83,"semantic_threshold_ratio":0.93,"target_headroom_interactions":6,"timeout_seconds":600,"request_timeout_seconds":300},"logging":{"context_metrics":"detailed"}}"#;
        let resolved = resolve(Some(source), None, None).unwrap();
        assert_eq!(resolved.config.context_token_budget, Some(256000));
        assert_eq!(
            resolved.provenance["compaction.request_timeout_seconds"],
            "user"
        );
        assert!(resolve(Some(r#"{"context_token_budget":256001}"#), None, None).is_err());
        for source in [
            r#"{"compaction":{"maintenance_threshold_ratio":0.96,"semantic_threshold_ratio":0.93}}"#,
            r#"{"compaction":{"semantic_threshold_ratio":1.0}}"#,
            r#"{"compaction":{"target_headroom_interactions":0}}"#,
            r#"{"compaction":{"request_timeout_seconds":0}}"#,
            r#"{"logging":{"context_metrics":"payloads"}}"#,
        ] {
            assert!(resolve(Some(source), None, None).is_err(), "{source}");
        }
    }
    #[test]
    fn longer_compaction_defaults_preserve_explicit_legacy_timeouts_and_match_schema() {
        assert_eq!(
            resolve(None, None, None)
                .unwrap()
                .config
                .compaction
                .timeout_seconds,
            900
        );
        assert!(resolve(
            Some(r#"{"compaction":{"timeout_seconds":300}}"#),
            None,
            None
        )
        .is_ok());
        assert!(resolve(
            Some(r#"{"compaction":{"timeout_seconds":1801}}"#),
            None,
            None
        )
        .is_err());
        assert!(resolve(Some(r#"{"compaction":{"timeout_seconds":0}}"#), None, None).is_err());
        let defaults = resolve(None, None, None).unwrap().config.compaction;
        assert_eq!(defaults.retry_limit, 2);
        assert_eq!(defaults.request_timeout_seconds, 300);
        for limit in 0..=2 {
            assert!(resolve(
                Some(&format!(r#"{{"compaction":{{"retry_limit":{limit}}}}}"#)),
                None,
                None
            )
            .is_ok());
        }
        assert!(resolve(Some(r#"{"compaction":{"retry_limit":3}}"#), None, None).is_err());
        assert_eq!(
            resolve(
                Some(r#"{"compaction":{"timeout_seconds":300,"retry_limit":1}}"#),
                None,
                None
            )
            .unwrap()
            .config
            .compaction
            .timeout_seconds,
            300
        );
        assert!(resolve(
            Some(r#"{"compaction":{"timeout_seconds":1800}}"#),
            None,
            None
        )
        .is_ok());
        let retry = &schema()["properties"]["compaction"]["properties"]["retry_limit"];
        assert_eq!(retry["default"], 2);
        assert_eq!(retry["maximum"], 2);
        let field = &schema()["properties"]["compaction"]["properties"]["timeout_seconds"];
        assert_eq!(field["default"], 900);
        assert_eq!(field["maximum"], 1800);
    }
    #[test]
    fn jsonc_layers_and_validation() {
        let c=resolve(Some("{\"compaction\":{\"timeout_seconds\":90,},//comment\n\"context_token_budget\":10000}"),Some("{\"context_token_budget\":12000}"),None).unwrap();
        assert_eq!(c.config.compaction.timeout_seconds, 90);
        assert_eq!(c.config.context_token_budget, Some(12000));
        assert_eq!(c.provenance["context_token_budget"], "project");
        assert!(resolve(Some("{\"unknown\":1}"), None, None).is_err());
        assert!(parse("{/*").is_err());
        assert!(resolve(None, Some("{\"approval\":7}"), None).is_err());
        assert!(resolve(Some("{\"logging\":{\"level\":\"verbose\"}}"), None, None).is_err());
        assert_eq!(c.provenance["compaction.retry_limit"], "default");
        assert_eq!(
            parse("{\"url\":\"https://x/a/*b\"}").unwrap()["url"],
            "https://x/a/*b"
        );
    }
    #[test]
    fn instructions_are_bounded_project_references() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("AGENTS.md"), "Preserve tests.").unwrap();
        let config = resolve(
            Some(r#"{"instructions":["AGENTS.md"],"skills":{"paths":["local-skills"]}}"#),
            None,
            None,
        )
        .unwrap()
        .config;
        assert!(load_instructions(&config, root.path())
            .unwrap()
            .contains("Preserve tests."));
        assert!(resolve(Some(r#"{"instructions":["../outside.md"]}"#), None, None).is_err());
        assert!(resolve(Some(r#"{"skills":{"paths":["/tmp/skills"]}}"#), None, None).is_err());
        std::fs::write(root.path().join("AGENTS.md"), "x".repeat(65537)).unwrap();
        assert!(load_instructions(&config, root.path()).is_err());
    }

    #[test]
    fn cached_user_grant_cannot_bypass_risk_deny() {
        let policy = ApprovalPolicy {
            default: PolicyAction::Ask,
            rules: vec![ApprovalRule {
                command: None,
                tool: "git".into(),
                action: PolicyAction::Deny,
                risk: Some("destructive".into()),
            }],
        };
        let hook = configured_hook(
            ApprovalMode::Custom,
            policy,
            Arc::new(crate::tools::AllowAllHook),
        );
        let action = |risk| ToolAction {
            display: None,
            command: Vec::new(),
            tool: "git".into(),
            summary: String::new(),
            risk,
        };
        assert_eq!(hook.approve(&action(RiskLevel::Read)), Approval::AllowOnce);
        assert_eq!(hook.approve(&action(RiskLevel::Read)), Approval::AllowOnce);
        assert_eq!(
            hook.approve(&action(RiskLevel::Destructive)),
            Approval::Deny
        );
    }

    fn shell_action(argv: &[&str]) -> ToolAction {
        ToolAction {
            display: None,
            command: argv.iter().map(|t| t.to_string()).collect(),
            tool: "shell".into(),
            summary: String::new(),
            risk: RiskLevel::Execute,
        }
    }

    fn decide(rules: &str, argv: &[&str]) -> PolicyAction {
        let policy = resolve(Some(rules), None, None).unwrap().config.approval;
        let mut action = shell_action(argv);
        action.risk = crate::tools::shell_risk(&action.command);
        policy.decision(&action)
    }

    #[test]
    fn sandbox_defaults_layering_and_provenance() {
        let defaults = resolve(None, None, None).unwrap();
        assert!(defaults.config.sandbox.enabled);
        assert_eq!(
            defaults.config.sandbox.network,
            crate::sandbox::NetworkPolicy::Allow
        );
        assert_eq!(defaults.provenance["sandbox.enabled"], "default");
        let user = r#"{"sandbox":{"writable_paths":["~/.cargo","/opt/cache"]}}"#;
        let project = r#"{"sandbox":{"network":"block","writable_paths":["~/.cargo"]}}"#;
        let resolved = resolve(Some(user), Some(project), None).unwrap();
        assert_eq!(
            resolved.config.sandbox.network,
            crate::sandbox::NetworkPolicy::Block
        );
        assert_eq!(resolved.config.sandbox.writable_paths, vec!["~/.cargo"]);
        assert_eq!(resolved.provenance["sandbox.network"], "project");
        assert_eq!(resolved.provenance["sandbox.writable_paths"], "project");
        for (layer, needle) in [
            (
                r#"{"sandbox":{"enabled":false}}"#,
                "cannot disable the sandbox",
            ),
            (
                r#"{"sandbox":{"writable_paths":["/etc"]}}"#,
                "cannot add sandbox.writable_paths",
            ),
            (
                r#"{"sandbox":{"backend":"landlock"}}"#,
                "cannot change sandbox.backend",
            ),
        ] {
            let error = format!("{:#}", resolve(Some(user), Some(layer), None).unwrap_err());
            assert!(error.contains(needle), "{error}");
            // A run override is held to the same rule.
            let error = format!("{:#}", resolve(Some(user), None, Some(layer)).unwrap_err());
            assert!(error.contains(needle), "{error}");
        }
        let blocked = r#"{"sandbox":{"network":"block"}}"#;
        assert!(resolve(
            Some(blocked),
            Some(r#"{"sandbox":{"network":"allow"}}"#),
            None
        )
        .is_err());
        // The user file may do anything valid, including turning it off.
        assert!(
            !resolve(Some(r#"{"sandbox":{"enabled":false}}"#), None, None)
                .unwrap()
                .config
                .sandbox
                .enabled
        );
        assert!(resolve(
            Some(r#"{"sandbox":{"writable_paths":["relative"]}}"#),
            None,
            None
        )
        .is_err());
        assert!(resolve(Some(r#"{"sandbox":{"mystery":1}}"#), None, None).is_err());
        let other_os = if cfg!(target_os = "macos") {
            "bubblewrap"
        } else {
            "seatbelt"
        };
        let error = format!(
            "{:#}",
            resolve(
                Some(&format!(r#"{{"sandbox":{{"backend":"{other_os}"}}}}"#)),
                None,
                None
            )
            .unwrap_err()
        );
        assert!(error.contains("not supported"), "{error}");
        assert!(schema()["properties"]["sandbox"]["properties"]["backend"]["enum"].is_array());
    }

    #[test]
    fn rule_sources_follow_the_layers() {
        let resolved = resolve(None, None, None).unwrap();
        assert_eq!(resolved.rule_sources, vec!["default"]);
        let user = r#"{"approval":{"rules":[{"tool":"shell","command":["cargo","test"],"action":"allow"},{"tool":"*","risk":"read","action":"allow"}]}}"#;
        let project = r#"{"approval":{"rules":[{"tool":"git","action":"deny"},{"tool":"shell","command":["rm"],"action":"deny"}]}}"#;
        let resolved = resolve(Some(user), Some(project), None).unwrap();
        assert_eq!(
            resolved.rule_sources,
            vec!["project", "project", "user", "user"]
        );
        assert_eq!(resolved.config.approval.rules.len(), 4);
        let resolved = resolve(None, Some(project), None).unwrap();
        assert_eq!(resolved.rule_sources, vec!["project", "project", "default"]);
        let deny_all = r#"{"approval":{"default":"deny"}}"#;
        let resolved = resolve(None, Some(deny_all), None).unwrap();
        assert_eq!(resolved.rule_sources, vec!["project", "default"]);
    }

    #[test]
    fn configuration_file_problems_carry_their_line() {
        assert_eq!(
            check_file("{\n  // fine\n  \"logging\": {\"level\": \"info\"},\n}"),
            None
        );
        let issue =
            check_file("{\n  \"sandbox\": {\n    \"enabled\": tru, \"network\": \"block\"\n  }\n}")
                .unwrap();
        assert_eq!(issue.line, Some(3), "{issue:?}");
        let issue = check_file("{\n  \"sandbox\": {\n    \"netwrk\": \"block\"\n  }\n}").unwrap();
        assert_eq!(issue.line, Some(3), "{issue:?}");
        assert!(issue.message.contains("netwrk"), "{issue:?}");
        assert!(!issue.message.contains(" at line "), "{issue:?}");
        let issue = check_file("[]").unwrap();
        assert_eq!(issue.line, Some(1));
        assert!(check_file("/* open").is_some());
        let text = "{\n  \"sandbox\": {\n    \"backend\": \"landlock\"\n  }\n}";
        assert_eq!(
            locate_error_line(text, "sandbox.backend \"landlock\" is not supported"),
            Some(3)
        );
        assert_eq!(locate_error_line(text, "no field here"), None);
    }

    #[test]
    fn deny_rules_match_equivalent_flag_spellings() {
        let deny = r#"{"approval":{"default":"allow","rules":[
            {"tool":"shell","command":["git","push","--force"],"action":"deny"},
            {"tool":"shell","command":["rm","-rf"],"action":"deny"}]}}"#;
        for denied in [
            vec!["git", "push", "--force"],
            vec!["git", "push", "-f", "origin", "main"],
            vec!["git", "push", "origin", "main", "--force"],
            vec!["/usr/bin/git", "push", "--force=yes"],
            vec!["rm", "-rf", "build"],
            vec!["rm", "-fr", "build"],
            vec!["rm", "-r", "-f", "build"],
            vec!["rm", "--recursive", "--force", "build"],
            vec!["rm", "-Rf", "build"],
        ] {
            assert_eq!(decide(deny, &denied), PolicyAction::Deny, "{denied:?}");
        }
        for allowed in [
            vec!["git", "push", "origin", "main"],
            vec!["git", "pull", "--force"],
            vec!["rm", "-r", "build"],
            vec!["rm", "file", "--", "-rf"],
        ] {
            assert_ne!(decide(deny, &allowed), PolicyAction::Deny, "{allowed:?}");
        }
    }

    #[test]
    fn broad_shell_allow_does_not_cover_destructive_variants() {
        let broad = r#"{"approval":{"rules":[{"tool":"shell","action":"allow"}]}}"#;
        assert_eq!(decide(broad, &["cargo", "test"]), PolicyAction::Allow);
        for variant in [
            vec!["git", "push", "--force"],
            vec!["git", "reset", "--hard"],
            vec!["git", "clean", "-fdx"],
            vec!["rm", "-rf", "target"],
            vec!["cargo", "publish"],
        ] {
            assert_eq!(decide(broad, &variant), PolicyAction::Ask, "{variant:?}");
        }
        let wildcard = r#"{"approval":{"rules":[{"tool":"*","action":"allow"}]}}"#;
        assert_eq!(decide(wildcard, &["rm", "-rf", "x"]), PolicyAction::Ask);
        let explicit =
            r#"{"approval":{"rules":[{"tool":"shell","risk":"destructive","action":"allow"}]}}"#;
        assert_eq!(decide(explicit, &["rm", "-rf", "x"]), PolicyAction::Allow);
        let execute =
            r#"{"approval":{"rules":[{"tool":"shell","risk":"execute","action":"allow"}]}}"#;
        assert_eq!(
            decide(execute, &["git", "reset", "--hard"]),
            PolicyAction::Ask
        );
        assert_eq!(decide(execute, &["git", "status"]), PolicyAction::Allow);
    }

    #[test]
    fn shell_destructive_variants_carry_the_destructive_risk() {
        for variant in [
            vec!["git", "push", "--force"],
            vec!["git", "reset", "--hard"],
            vec!["git", "clean", "-fdx"],
            vec!["rm", "-rf", "/tmp/x"],
            vec!["rm", "--recursive", "x"],
        ] {
            assert_eq!(
                crate::tools::shell_risk(&argv(&variant)),
                RiskLevel::Destructive,
                "{variant:?}"
            );
        }
        for ordinary in [
            vec!["rm", "file"],
            vec!["rm", "-f", "file"],
            vec!["ls", "-R"],
        ] {
            assert_eq!(
                crate::tools::shell_risk(&argv(&ordinary)),
                RiskLevel::Execute,
                "{ordinary:?}"
            );
        }
    }

    #[test]
    fn publish_words_as_operands_of_other_subcommands_are_harmless() {
        for harmless in [
            vec!["cargo", "test", "publish"],
            vec!["cargo", "test", "--", "publish"],
            vec!["cargo", "build", "-p", "publish"],
            vec!["cargo", "run", "--bin", "yank"],
            vec!["npm", "test", "publish"],
            vec!["pnpm", "install", "publisher"],
            vec!["yarn", "add", "deprecated-pkg"],
        ] {
            assert!(!is_destructive_variant(&argv(&harmless)), "{harmless:?}");
        }
        for danger in [
            vec!["npm", "run", "publish"],
            vec!["cargo", "publish", "--dry-run"],
            vec!["pnpm", "-r", "publish"],
        ] {
            assert!(is_destructive_variant(&argv(&danger)), "{danger:?}");
        }
    }

    #[test]
    fn command_rules_match_token_prefixes_only() {
        let config = resolve(
            Some(
                r#"{"approval":{"rules":[
                {"tool":"shell","command":["cargo","test"],"action":"allow"},
                {"tool":"shell","command":["echo"],"action":"allow"}]}}"#,
            ),
            Some(r#"{"approval":{"rules":[{"tool":"shell","command":["rm"],"action":"deny"}]}}"#),
            None,
        )
        .unwrap()
        .config;
        let decide = |argv: &[&str]| config.approval.decision(&shell_action(argv));
        assert_eq!(decide(&["cargo", "test", "--lib"]), PolicyAction::Allow);
        assert_eq!(decide(&["echo"]), PolicyAction::Allow);
        assert_eq!(decide(&["cargo", "build"]), PolicyAction::Ask);
        assert_eq!(decide(&["cargo", "testing"]), PolicyAction::Ask);
        assert_eq!(decide(&["cargo"]), PolicyAction::Ask);
        assert_eq!(decide(&["/bin/echo", "x"]), PolicyAction::Ask);
        assert_eq!(decide(&["rm", "-rf", "x"]), PolicyAction::Deny);
        // Deny rules also catch path-qualified spellings of the program...
        assert_eq!(decide(&["/bin/rm", "x"]), PolicyAction::Deny);
        assert_eq!(decide(&["./rm"]), PolicyAction::Deny);
        // ...but allow rules never widen: a local ./echo is not `echo`.
        assert_eq!(decide(&["./echo", "hi"]), PolicyAction::Ask);
        assert_eq!(decide(&["/bin/echo"]), PolicyAction::Ask);
        // Deny rules see through wrappers and shell scripts.
        assert_eq!(decide(&["env", "rm", "x"]), PolicyAction::Deny);
        assert_eq!(decide(&["sh", "-c", "ls && rm x"]), PolicyAction::Deny);
        assert_eq!(
            decide(&["sudo", "-u", "me", "/bin/rm", "x"]),
            PolicyAction::Deny
        );
        // ...but allow rules stay exact: `echo` does not allow a script that echoes.
        assert_eq!(decide(&["sh", "-c", "echo hi"]), PolicyAction::Ask);
        #[cfg(any(target_os = "macos", windows))]
        assert_eq!(decide(&["RM", "x"]), PolicyAction::Deny);
        #[cfg(windows)]
        assert_eq!(decide(&["rm.exe", "x"]), PolicyAction::Deny);
        // A command rule never matches a tool that carries no argv.
        let mut other = shell_action(&[]);
        other.tool = "git".into();
        assert_eq!(config.approval.decision(&other), PolicyAction::Ask);
    }

    #[test]
    fn command_rule_validation() {
        let bad = |rule: &str| {
            resolve(
                Some(&format!(r#"{{"approval":{{"rules":[{rule}]}}}}"#)),
                None,
                None,
            )
            .is_err()
        };
        assert!(!bad(
            r#"{"tool":"shell","command":["ls"],"action":"allow"}"#
        ));
        assert!(bad(r#"{"tool":"shell","command":[],"action":"allow"}"#));
        assert!(bad(r#"{"tool":"shell","command":[""],"action":"allow"}"#));
        assert!(bad(
            r#"{"tool":"git","command":["status"],"action":"allow"}"#
        ));
        assert!(bad(
            r#"{"tool":"shell*","command":["ls"],"action":"allow"}"#
        ));
        assert!(bad(
            r#"{"tool":"shell","command":["a"],"action":"allow","x":1}"#
        ));
        let seventeen = vec!["a"; 17].join("\",\"");
        assert!(bad(&format!(
            r#"{{"tool":"shell","command":["{seventeen}"],"action":"allow"}}"#
        )));
        let long = "a".repeat(257);
        assert!(bad(&format!(
            r#"{{"tool":"shell","command":["{long}"],"action":"allow"}}"#
        )));
        // Project layer may add command denies but not allows.
        assert!(resolve(
            None,
            Some(r#"{"approval":{"rules":[{"tool":"shell","command":["ls"],"action":"allow"}]}}"#),
            None
        )
        .is_err());
    }

    #[test]
    fn grant_key_scopes_shell_to_program_and_subcommand() {
        let key = |argv: &[&str]| shell_action(argv).grant_key();
        assert_eq!(key(&["cargo", "test", "--lib"]), key(&["cargo", "test"]));
        assert_ne!(key(&["cargo", "test"]), key(&["cargo", "publish"]));
        assert_ne!(key(&["cargo", "test"]), key(&["rm", "-rf", "x"]));
        // Flag-first calls are keyed by their full argv.
        assert_ne!(key(&["bash", "-c", "ls"]), key(&["bash", "-c", "rm x"]));
        assert_eq!(key(&["ls"]), key(&["ls"]));
        // Only subcommand-style CLIs share a key across their arguments.
        assert_eq!(key(&["cargo", "test", "--lib"]), key(&["cargo", "test"]));
        assert_ne!(
            key(&["find", ".", "-name", "x"]),
            key(&["find", ".", "-delete"])
        );
        assert_ne!(
            key(&["sudo", "rm", "foo"]),
            key(&["sudo", "rm", "-rf", "/"])
        );
        assert_ne!(key(&["env", "X=1", "rm"]), key(&["env", "X=1", "ls"]));
        assert_ne!(
            key(&["cargo", "run", "--", "a"]),
            key(&["cargo", "run", "--", "b"])
        );
        assert_ne!(key(&["npm", "exec", "a"]), key(&["npm", "exec", "b"]));
        assert_ne!(key(&["ssh", "host", "ls"]), key(&["ssh", "host", "rm"]));
        let mut git = shell_action(&[]);
        git.tool = "git".into();
        assert_eq!(git.grant_key(), "git");
        // The git tool carries `["git", args...]`: subcommand-scoped, but
        // destructive checkout/branch variants get their own key.
        let git_key = |argv: &[&str]| {
            let mut action = shell_action(argv);
            action.tool = "git".into();
            action.grant_key()
        };
        assert_eq!(git_key(&["git", "add", "a"]), git_key(&["git", "add", "b"]));
        assert_eq!(
            git_key(&["git", "checkout", "main"]),
            git_key(&["git", "checkout", "dev"])
        );
        assert_ne!(
            git_key(&["git", "checkout", "main"]),
            git_key(&["git", "checkout", "-f", "main"])
        );
        assert_ne!(
            git_key(&["git", "branch"]),
            git_key(&["git", "branch", "-D", "x"])
        );
        assert_ne!(
            git_key(&["git", "add", "a"]),
            shell_action(&["git", "add", "a"]).grant_key()
        );
    }

    #[test]
    fn destructive_variants_get_their_own_grant_key() {
        let key = |argv: &[&str]| shell_action(argv).grant_key();
        // Harmless siblings still share a key.
        assert_eq!(
            key(&["git", "push"]),
            key(&["git", "push", "origin", "main"])
        );
        assert_eq!(
            key(&["git", "clean", "-n"]),
            key(&["git", "clean", "--dry-run"])
        );
        assert_eq!(key(&["git", "reset"]), key(&["git", "reset", "HEAD"]));
        assert_eq!(key(&["git", "branch", "-d", "x"]), key(&["git", "branch"]));
        assert!(key(&["cargo", "publish", "--dry-run"]).contains("--dry-run"));
        for (safe, danger) in [
            (vec!["git", "push"], vec!["git", "push", "--force"]),
            (
                vec!["git", "push"],
                vec!["git", "push", "-f", "origin", "main"],
            ),
            (
                vec!["git", "push"],
                vec!["git", "push", "--force-with-lease"],
            ),
            (vec!["git", "push"], vec!["git", "push", "--mirror"]),
            (
                vec!["git", "push"],
                vec!["git", "push", "--delete", "origin", "x"],
            ),
            (vec!["git", "push"], vec!["git", "push", "origin", ":x"]),
            (vec!["git", "push"], vec!["git", "push", "origin", "+main"]),
            (vec!["git", "push"], vec!["git", "push", "-fu"]),
            (vec!["git", "clean", "-n"], vec!["git", "clean", "-fdx"]),
            (vec!["git", "clean", "-n"], vec!["git", "clean", "-nd"]),
            (vec!["git", "reset"], vec!["git", "reset", "--hard"]),
            (
                vec!["git", "checkout", "main"],
                vec!["git", "checkout", "-f", "main"],
            ),
            (
                vec!["git", "checkout", "main"],
                vec!["git", "checkout", "--force", "main"],
            ),
            (
                vec!["git", "switch", "main"],
                vec!["git", "switch", "--discard-changes", "main"],
            ),
            (
                vec!["git", "restore", "--staged", "f"],
                vec!["git", "restore", "f"],
            ),
            (vec!["git", "branch"], vec!["git", "branch", "-D", "x"]),
            (
                vec!["git", "branch"],
                vec!["git", "branch", "--delete", "--force", "x"],
            ),
            (vec!["git", "log"], vec!["git", "rebase", "main"]),
            (vec!["npm", "test"], vec!["npm", "publish"]),
            (vec!["pnpm", "test"], vec!["pnpm", "publish", "--tag", "x"]),
            (vec!["yarn", "test"], vec!["yarn", "publish"]),
            (vec!["cargo", "build"], vec!["cargo", "publish"]),
            (vec!["cargo", "build"], vec!["cargo", "yank", "x"]),
        ] {
            assert_ne!(key(&safe), key(&danger), "{safe:?} vs {danger:?}");
            assert!(
                is_destructive_variant(&danger.iter().map(|t| t.to_string()).collect::<Vec<_>>()),
                "{danger:?}"
            );
        }
        // Different destructive calls do not share a key with each other either.
        assert_ne!(
            key(&["git", "push", "--force", "origin", "a"]),
            key(&["git", "push", "--force", "origin", "b"])
        );
        // Global options before the subcommand do not hide the variant.
        assert!(is_destructive_variant(&[
            "git".into(),
            "-C".into(),
            "d".into(),
            "push".into(),
            "-f".into()
        ]));
        assert!(is_destructive_variant(&[
            "/usr/bin/GIT.exe".into(),
            "reset".into(),
            "--hard".into()
        ]));
        assert!(!is_destructive_variant(&[
            "git".into(),
            "status".into(),
            "-f".into()
        ]));
    }

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|t| t.to_string()).collect()
    }

    #[test]
    fn unflagged_work_and_history_discarding_git_variants_are_destructive() {
        for danger in [
            vec!["git", "checkout", "."],
            vec!["git", "checkout", "--", "src/a.rs"],
            vec!["git", "checkout", "main", "--", "a"],
            vec!["git", "stash", "drop"],
            vec!["git", "stash", "clear"],
            vec!["git", "-C", "d", "stash", "drop", "stash@{1}"],
            vec!["git", "reset", "HEAD~3"],
            vec!["git", "reset", "--soft", "abc123"],
            vec!["git", "commit", "--amend"],
            vec!["git", "commit", "-m", "x", "--amend"],
            vec!["git", "tag", "-d", "v1"],
            vec!["git", "tag", "--delete", "v1"],
            vec!["git", "update-ref", "-d", "refs/heads/x"],
            vec!["git", "reflog", "expire", "--expire=now", "--all"],
            vec!["git", "reflog", "delete", "HEAD@{1}"],
            vec!["git", "gc", "--prune=now"],
            vec!["git", "prune"],
            vec!["git", "filter-branch", "--all"],
            vec!["git", "worktree", "remove", "-f", "w"],
            vec!["git", "worktree", "remove", "--force", "w"],
        ] {
            assert!(is_destructive_variant(&argv(&danger)), "{danger:?}");
        }
        for harmless in [
            vec!["git", "checkout", "main"],
            vec!["git", "checkout", "-b", "feature"],
            vec!["git", "stash"],
            vec!["git", "stash", "list"],
            vec!["git", "reset"],
            vec!["git", "reset", "HEAD"],
            vec!["git", "reset", "--keep"],
            vec!["git", "reset", "--", "file"],
            vec!["git", "commit", "-m", "x"],
            vec!["git", "tag", "v1"],
            vec!["git", "reflog"],
            vec!["git", "gc"],
            vec!["git", "worktree", "list"],
            vec!["git", "worktree", "remove", "w"],
        ] {
            assert!(!is_destructive_variant(&argv(&harmless)), "{harmless:?}");
        }
        // The run-grant scenario: approving `git checkout main` never covers `git checkout -- .`.
        let key = |parts: &[&str]| shell_action(parts).grant_key();
        assert_ne!(
            key(&["git", "checkout", "main"]),
            key(&["git", "checkout", "--", "."])
        );
    }

    #[test]
    fn git_long_option_abbreviations_are_still_destructive() {
        for danger in [
            vec!["git", "reset", "--har"],
            vec!["git", "push", "--mir"],
            vec!["git", "push", "--dele", "origin", "x"],
            vec!["git", "push", "--forc", "origin"],
            vec!["git", "push", "--force-with", "origin"],
            vec!["git", "clean", "--forc"],
            vec!["git", "checkout", "--discard"],
            vec!["git", "commit", "--am"],
            vec!["git", "branch", "--forc", "x"],
        ] {
            assert!(is_destructive_variant(&argv(&danger)), "{danger:?}");
        }
        assert!(!is_destructive_variant(&argv(&[
            "git",
            "push",
            "--set-upstream"
        ])));
        assert!(!is_destructive_variant(&argv(&[
            "git", "restore", "--stag", "f"
        ])));
    }

    #[test]
    fn package_publish_is_found_behind_value_taking_options_and_abbreviations() {
        for danger in [
            vec!["pnpm", "--filter", "pkg", "publish"],
            vec!["pnpm", "-F", "pkg", "publish"],
            vec!["npm", "-w", "x", "publish"],
            vec!["npm", "--workspace", "x", "publish"],
            vec!["npm", "--otp", "123456", "publish"],
            vec!["npm", "--registry", "https://r.example", "unpublish", "x"],
            vec!["npm", "publi"],
            vec!["npm", "pub"],
            vec!["npm", "deprec", "x", "msg"],
            vec!["yarn", "--cwd", "pkg", "npm", "publish"],
            vec!["yarn", "npm", "publish"],
            vec!["yarn", "workspace", "pkg", "npm", "publish"],
            vec!["cargo", "--config", "k=v", "publish"],
            vec!["cargo", "+nightly", "publish"],
            vec!["cargo", "-Z", "unstable-options", "yank", "x"],
        ] {
            assert!(is_destructive_variant(&argv(&danger)), "{danger:?}");
        }
        for harmless in [
            vec!["npm", "test"],
            vec!["npm", "-w", "x", "test"],
            vec!["yarn", "npm", "whoami"],
            vec!["cargo", "--config", "k=v", "build"],
            vec!["pnpm", "--filter", "pkg", "test"],
        ] {
            assert!(!is_destructive_variant(&argv(&harmless)), "{harmless:?}");
        }
        let key = |parts: &[&str]| shell_action(parts).grant_key();
        assert_ne!(
            key(&["yarn", "npm", "whoami"]),
            key(&["yarn", "npm", "publish"])
        );
        // git `--config-env <name>=<ENV>` takes a separate value.
        assert!(is_destructive_variant(&argv(&[
            "git",
            "--config-env",
            "a=B",
            "push",
            "-f"
        ])));
    }

    #[test]
    fn allow_rule_for_git_push_does_not_cover_forced_push() {
        let config = resolve(
            Some(r#"{"approval":{"rules":[{"tool":"shell","command":["git","push"],"action":"allow"}]}}"#),
            None,
            None,
        )
        .unwrap();
        let policy = config.config.approval.clone();
        let decide = |argv: &[&str]| policy.decision(&shell_action(argv));
        assert_eq!(decide(&["git", "push"]), PolicyAction::Allow);
        assert_eq!(
            decide(&["git", "push", "origin", "main"]),
            PolicyAction::Allow
        );
        for argv in [
            ["git", "push", "--force"].as_slice(),
            &["git", "push", "origin", "main", "-f"],
            &["git", "push", "origin", ":old"],
        ] {
            assert_eq!(decide(argv), PolicyAction::Ask, "{argv:?}");
        }
        // A rule that names the variant explicitly is honored.
        let explicit = resolve(
            Some(r#"{"approval":{"rules":[{"tool":"shell","command":["git","push","--force"],"action":"allow"}]}}"#),
            None,
            None,
        )
        .unwrap();
        assert_eq!(
            explicit
                .config
                .approval
                .decision(&shell_action(&["git", "push", "--force", "origin"])),
            PolicyAction::Allow
        );
        // Deny rules are unaffected.
        let deny = resolve(
            Some(r#"{"approval":{"rules":[{"tool":"shell","command":["git","push"],"action":"deny"}]}}"#),
            None,
            None,
        )
        .unwrap();
        assert_eq!(
            deny.config
                .approval
                .decision(&shell_action(&["git", "push", "--force"])),
            PolicyAction::Deny
        );
    }

    #[test]
    fn approving_git_push_once_always_still_asks_for_force_push() {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = Arc::clone(&calls);
        let hook = configured_hook(
            ApprovalMode::Custom,
            ApprovalPolicy::default(),
            Arc::new(crate::tools::FnHook::new(move |_: &ToolAction| {
                counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Approval::AllowAlways
            })),
        );
        hook.approve(&shell_action(&["git", "push"]));
        hook.approve(&shell_action(&["git", "push", "origin", "main"]));
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        hook.approve(&shell_action(&["git", "push", "--force"]));
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[test]
    fn destructive_shell_variant_scope_is_the_exact_argv() {
        let danger = shell_action(&["git", "push", "--force", "origin", "main"]);
        let scope = danger.grant_scope().unwrap();
        assert_eq!(scope.argv, ["git", "push", "--force", "origin", "main"]);
        assert!(!scope.covers_further_args);
        assert_eq!(
            danger.grant_key(),
            format!("shell\0{}", scope.argv.join("\0"))
        );
        let safe = shell_action(&["git", "push", "origin", "main"])
            .grant_scope()
            .unwrap();
        assert_eq!(safe.argv, ["git", "push"]);
        assert!(safe.covers_further_args);
    }

    #[test]
    fn grant_scope_matches_grant_key_semantics() {
        let cases: &[&[&str]] = &[
            &["cargo", "test", "--lib"],
            &["cargo", "run", "--", "a"],
            &["find", ".", "-delete"],
            &["rm", "-rf /"],
            &["rm", "-rf", "/"],
            &["git", "-C", "x", "status"],
            &["echo", "a\nb`c\u{202e}"],
        ];
        for argv in cases {
            let action = shell_action(argv);
            let scope = action.grant_scope().expect("shell has a scope");
            // The key is exactly the scope's tokens: equal scopes <=> equal keys.
            assert_eq!(
                action.grant_key(),
                format!("shell\0{}", scope.argv.join("\0"))
            );
            assert_eq!(scope.covers_further_args, scope.argv.len() < argv.len());
            assert_eq!(
                scope.argv[..],
                argv.iter().map(|t| t.to_string()).collect::<Vec<_>>()[..scope.argv.len()]
            );
        }
        assert_eq!(
            shell_action(&["cargo", "test", "--lib"])
                .grant_scope()
                .unwrap()
                .argv,
            ["cargo", "test"]
        );
        // Tokens stay distinct: a space inside one argument is not two arguments.
        assert_ne!(
            shell_action(&["rm", "-rf /"]).grant_scope(),
            shell_action(&["rm", "-rf", "/"]).grant_scope()
        );
        let mut other = shell_action(&["status"]);
        other.tool = "git".into();
        assert_eq!(other.grant_scope(), None);
    }

    #[test]
    fn cached_shell_grant_does_not_extend_to_another_program() {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = Arc::clone(&calls);
        let hook = configured_hook(
            ApprovalMode::Custom,
            ApprovalPolicy::default(),
            Arc::new(crate::tools::FnHook::new(move |_: &ToolAction| {
                counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Approval::AllowAlways
            })),
        );
        assert_eq!(
            hook.approve(&shell_action(&["cargo", "test"])),
            Approval::AllowOnce
        );
        assert_eq!(
            hook.approve(&shell_action(&["cargo", "test", "-q"])),
            Approval::AllowOnce
        );
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        hook.approve(&shell_action(&["rm", "-rf", "x"]));
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    fn counting_hook(
        calls: &Arc<std::sync::atomic::AtomicUsize>,
        answer: Approval,
    ) -> Arc<dyn ApprovalHook> {
        let calls = calls.clone();
        Arc::new(crate::tools::FnHook::new(move |_| {
            calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            answer
        }))
    }

    fn escape(argv: &[&str]) -> ToolAction {
        let mut action = shell_action(argv);
        action.tool = crate::tools::UNSANDBOXED_TOOL.into();
        action
    }

    /// Decision for an escape under `rules`, with an interactive hook that
    /// answers `answer`; returns the decision and how often a person was asked.
    fn escape_with(
        rules: &str,
        mode: ApprovalMode,
        answer: Approval,
        grants: Arc<ChatGrants>,
    ) -> (Approval, usize) {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let policy = resolve(Some(rules), None, None).unwrap().config.approval;
        let hook = configured_hook_with_grants(mode, policy, counting_hook(&calls, answer), grants);
        let decision = hook.approve(&escape(&["touch", "/outside"]));
        (decision, calls.load(std::sync::atomic::Ordering::SeqCst))
    }

    #[test]
    fn broad_allow_rules_never_let_a_command_leave_the_sandbox() {
        for rules in [
            r#"{"approval":{"rules":[{"tool":"*","action":"allow"}]}}"#,
            r#"{"approval":{"rules":[{"tool":"shell*","action":"allow"}]}}"#,
            r#"{"approval":{"rules":[{"tool":"*","risk":"execute","action":"allow"}]}}"#,
            r#"{"approval":{"default":"allow","rules":[]}}"#,
            "{}",
        ] {
            // Without a person (the dialog denies) it never runs outside...
            let (decision, asked) = escape_with(
                rules,
                ApprovalMode::Custom,
                Approval::Deny,
                ChatGrants::in_memory(),
            );
            assert_eq!(decision, Approval::Deny, "{rules}");
            assert_eq!(asked, 1, "a person is asked: {rules}");
            // ...and the person's Allow once is what lets it.
            let (decision, _) = escape_with(
                rules,
                ApprovalMode::Custom,
                Approval::AllowOnce,
                ChatGrants::in_memory(),
            );
            assert_eq!(decision, Approval::AllowOnce, "{rules}");
        }
    }

    #[test]
    fn deny_rules_of_any_shape_refuse_the_escape_without_asking() {
        for rules in [
            r#"{"approval":{"rules":[{"tool":"*","action":"deny"}]}}"#,
            r#"{"approval":{"rules":[{"tool":"shell_unsandboxed","action":"deny"}]}}"#,
            r#"{"approval":{"default":"deny","rules":[]}}"#,
        ] {
            let (decision, asked) = escape_with(
                rules,
                ApprovalMode::Custom,
                Approval::AllowOnce,
                ChatGrants::in_memory(),
            );
            assert_eq!((decision, asked), (Approval::Deny, 0), "{rules}");
        }
    }

    #[test]
    fn only_a_grant_an_exact_rule_or_a_person_lets_a_command_leave() {
        let grants = ChatGrants::in_memory();
        assert!(grants.remember(&escape(&["touch", "/outside"])));
        let (decision, asked) = escape_with("{}", ApprovalMode::Custom, Approval::Deny, grants);
        assert_eq!((decision, asked), (Approval::AllowOnce, 0));
        // A plain `shell` grant for the same argv does not cover the escape.
        let shell_only = ChatGrants::in_memory();
        assert!(shell_only.remember(&shell_action(&["touch", "/outside"])));
        let (decision, asked) = escape_with("{}", ApprovalMode::Custom, Approval::Deny, shell_only);
        assert_eq!((decision, asked), (Approval::Deny, 1));
        let exact = r#"{"approval":{"rules":[{"tool":"shell_unsandboxed","command":["touch"],"action":"allow"}]}}"#;
        let (decision, asked) = escape_with(
            exact,
            ApprovalMode::Custom,
            Approval::Deny,
            ChatGrants::in_memory(),
        );
        assert_eq!((decision, asked), (Approval::AllowOnce, 0));
        // YOLO never asks, so it never leaves the sandbox either.
        let (decision, asked) = escape_with(
            "{}",
            ApprovalMode::Yolo,
            Approval::AllowOnce,
            ChatGrants::in_memory(),
        );
        assert_eq!((decision, asked), (Approval::Deny, 0));
    }

    #[test]
    fn chat_grant_made_in_one_run_covers_a_later_run_of_the_same_chat_only() {
        let dir = tempfile::tempdir().unwrap();
        let chat = dir.path().join("chat-a.json");
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let run = |path: &std::path::Path| {
            configured_hook_with_grants(
                ApprovalMode::Custom,
                ApprovalPolicy::default(),
                counting_hook(&calls, Approval::AllowAlways),
                ChatGrants::load(path),
            )
        };
        let first = run(&chat);
        first.approve(&shell_action(&["cargo", "test"]));
        // Covered at once inside the same run.
        first.approve(&shell_action(&["cargo", "test", "-q"]));
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        // A second run of the same chat loads the saved grant.
        run(&chat).approve(&shell_action(&["cargo", "test", "--lib"]));
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        // Another chat asks again.
        run(&dir.path().join("chat-b.json")).approve(&shell_action(&["cargo", "test"]));
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2);
        // A different command family in the same chat also asks.
        run(&chat).approve(&shell_action(&["cargo", "run"]));
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 3);
    }

    #[test]
    fn destructive_variants_ask_every_time_despite_a_chat_grant() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chat.json");
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let hook = configured_hook_with_grants(
            ApprovalMode::Custom,
            ApprovalPolicy::default(),
            counting_hook(&calls, Approval::AllowAlways),
            ChatGrants::load(&path),
        );
        let forced = shell_action(&["git", "push", "--force", "origin", "main"]);
        assert_eq!(hook.approve(&forced), Approval::AllowOnce);
        assert_eq!(hook.approve(&forced), Approval::AllowOnce);
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2);
        let mut delete = shell_action(&["x"]);
        delete.tool = "delete_file".into();
        delete.command.clear();
        delete.risk = RiskLevel::Destructive;
        hook.approve(&delete);
        hook.approve(&delete);
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 4);
        assert!(!path.exists(), "nothing destructive is ever stored");
    }

    #[test]
    fn deny_rules_beat_chat_grants() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chat.json");
        let grants = ChatGrants::load(&path);
        assert!(grants.remember(&shell_action(&["rm", "build.log"])));
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let policy = resolve(
            Some(r#"{"approval":{"rules":[{"tool":"shell","command":["rm"],"action":"deny"}]}}"#),
            None,
            None,
        )
        .unwrap()
        .config
        .approval;
        let hook = configured_hook_with_grants(
            ApprovalMode::Custom,
            policy,
            counting_hook(&calls, Approval::AllowOnce),
            grants,
        );
        assert_eq!(
            hook.approve(&shell_action(&["rm", "build.log"])),
            Approval::Deny
        );
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[test]
    fn invalid_grant_file_never_widens_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chat.json");
        std::fs::write(
            &path,
            r#"{"version":1,"grants":[{"tool":"shell","command":["echo"],"risk":"execute"},{"tool":"shell","command":[],"risk":"execute"}]}"#,
        )
        .unwrap();
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let hook = configured_hook_with_grants(
            ApprovalMode::Custom,
            ApprovalPolicy::default(),
            counting_hook(&calls, Approval::Deny),
            ChatGrants::load(&path),
        );
        assert_eq!(hook.approve(&shell_action(&["echo", "x"])), Approval::Deny);
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    mod prop {
        use super::*;
        use proptest::prelude::*;

        proptest! {
                   #[test]
                   fn prefix_match_agrees_with_slice_prefix(
                       command in proptest::collection::vec(".{0,8}", 0..6),
                       prefix in proptest::collection::vec(".{0,8}", 0..6),
                   ) {
                       let expected = !prefix.is_empty() && command.starts_with(&prefix);
                       prop_assert_eq!(command_has_prefix(&command, &prefix), expected);
                       // Any non-empty prefix of the command itself always matches.
                       for n in 1..=command.len() {
                           prop_assert!(command_has_prefix(&command, &command[..n]));
                       }
                   }

            #[test]
            fn deny_matching_only_widens_exact_prefixes(
                command in proptest::collection::vec("-{0,2}[a-zA-Z]{0,4}", 0..7),
                prefix in proptest::collection::vec("-{0,2}[a-zA-Z]{0,4}", 0..5),
            ) {
                // Never panics, and every exact prefix match is also a deny match.
                let widened = deny_matches(&command, &prefix);
                if command_has_prefix(&command, &prefix) {
                    prop_assert!(widened, "{command:?} {prefix:?}");
                }
            }

            #[test]
            fn deny_flags_match_in_any_order_and_cluster(
                letters in proptest::sample::subsequence(vec!['r', 'f', 'v', 'i'], 1..=4),
                split in proptest::bool::ANY,
            ) {
                let rule: Vec<String> = vec!["rm".into(), format!("-{}", letters.iter().collect::<String>())];
                let mut command = vec!["rm".to_string()];
                if split {
                    command.extend(letters.iter().rev().map(|c| format!("-{c}")));
                } else {
                    command.push(format!("-{}", letters.iter().rev().collect::<String>()));
                }
                command.push("target".into());
                prop_assert!(deny_matches(&command, &rule), "{command:?} {rule:?}");
            }

            #[test]
            fn destructive_flag_never_shares_the_harmless_key(
                sub in proptest::sample::select(vec!["push", "clean", "reset", "checkout", "branch"]),
                tail in proptest::collection::vec("[a-z/:+.-]{0,6}", 0..4),
            ) {
                let argv = |extra: &[String]| {
                    let mut v = vec!["git".to_string(), sub.to_string()];
                    v.extend(extra.iter().cloned());
                    v
                };
                let mk = |command: Vec<String>| ToolAction {
                    display: None, command, tool: "shell".into(), summary: String::new(), risk: RiskLevel::Execute,
                };
                let danger = mk(argv(&tail));
                let harmless = mk(argv(&[]));
                if is_destructive_variant(&danger.command) {
                    prop_assert_ne!(danger.grant_key(), harmless.grant_key());
                    // Full argv: never shorter than the whole command.
                    prop_assert_eq!(danger.grant_key().split('\0').count(), 1 + danger.command.len());
                } else {
                    prop_assert_eq!(danger.grant_key(), harmless.grant_key());
                }
            }

            #[test]
            fn any_prefix_of_a_destructive_long_option_is_destructive(
                case in proptest::sample::select(vec![
                    ("push", "force"), ("push", "mirror"), ("push", "delete"),
                    ("reset", "hard"), ("clean", "force"), ("commit", "amend"),
                    ("branch", "force"), ("tag", "delete"), ("gc", "prune"),
                ]),
                len in 1usize..=8,
            ) {
                let (sub, long) = case;
                let prefix = &long[..len.min(long.len())];
                let command = vec!["git".to_string(), sub.to_string(), format!("--{prefix}")];
                prop_assert!(is_destructive_variant(&command), "{command:?}");
            }

            #[test]
            fn registry_publish_is_found_after_any_option_prefix(
                tool in proptest::sample::select(vec!["npm", "pnpm", "yarn"]),
                flags in proptest::collection::vec("-{1,2}[a-z]{1,6}", 0..4),
                values in proptest::collection::vec("[a-z0-9=]{1,6}", 0..3),
            ) {
                let mut command = vec![tool.to_string()];
                for (i, flag) in flags.iter().enumerate() {
                    command.push(flag.clone());
                    if let Some(value) = values.get(i) { command.push(value.clone()); }
                }
                command.push("publish".into());
                prop_assert!(is_destructive_variant(&command), "{command:?}");
            }

            #[test]
            fn is_destructive_variant_never_panics(command in proptest::collection::vec(".{0,8}", 0..6)) {
                let _ = is_destructive_variant(&command);
            }

            #[test]
            fn grant_key_never_panics_and_is_stable(command in proptest::collection::vec(".{0,8}", 0..6)) {
                let action = ToolAction {
                    display: None,
                    command,
                    tool: "shell".into(),
                    summary: String::new(),
                    risk: RiskLevel::Execute,
                };
                prop_assert_eq!(action.grant_key(), action.grant_key());
            }
        }
    }

    #[test]
    fn a_read_file_deny_also_denies_recall() {
        let action = |tool: &str| ToolAction {
            display: None,
            command: Vec::new(),
            tool: tool.into(),
            summary: String::new(),
            risk: RiskLevel::Read,
        };
        let deny_read_file = resolve(
            Some(r#"{"approval":{"rules":[{"tool":"read_file","action":"deny"}]}}"#),
            None,
            None,
        )
        .unwrap()
        .config
        .approval;
        assert_eq!(
            deny_read_file.decision(&action("recall")),
            PolicyAction::Deny
        );
        let deny_recall = resolve(
            Some(r#"{"approval":{"rules":[{"tool":"recall","action":"deny"}]}}"#),
            None,
            None,
        )
        .unwrap()
        .config
        .approval;
        assert_eq!(deny_recall.decision(&action("recall")), PolicyAction::Deny);
        assert_ne!(
            deny_recall.decision(&action("read_file")),
            PolicyAction::Deny
        );
        // Defaults keep recall a no-prompt read.
        let defaults = ApprovalPolicy::default();
        assert_eq!(defaults.decision(&action("recall")), PolicyAction::Allow);
    }

    #[test]
    fn policy_order_and_project_restrictions() {
        let config = resolve(
            Some(r#"{"approval":{"default":"deny","rules":[{"tool":"read*","action":"allow"}]}}"#),
            Some(r#"{"approval":{"rules":[{"tool":"read_secret","action":"deny"}]}}"#),
            None,
        )
        .unwrap()
        .config;
        let action = |tool: &str| ToolAction {
            display: None,
            command: Vec::new(),
            tool: tool.into(),
            summary: String::new(),
            risk: RiskLevel::Read,
        };
        assert_eq!(
            config.approval.decision(&action("read_secret")),
            PolicyAction::Deny
        );
        assert_eq!(
            config.approval.decision(&action("read_file")),
            PolicyAction::Allow
        );
        assert!(resolve(None, Some(r#"{"approval":{"default":"allow"}}"#), None).is_err());
        let hook = configured_hook(
            ApprovalMode::Yolo,
            config.approval,
            Arc::new(crate::tools::DenyAllHook),
        );
        assert_eq!(hook.approve(&action("shell")), Approval::AllowOnce);
    }
}
