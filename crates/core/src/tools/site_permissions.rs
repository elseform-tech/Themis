//! What the agent may do on a site in its built-in browser (Settings › Browser): each of
//! Browsing, Downloads and Debugging is Allow (no prompt), Ask (an approval prompt, which
//! YOLO allows) or Block (refused in every mode). Local apps are always allowed. The
//! browser tools and `fetch_url` check here before anything else; see `docs/web-tools.md`.

use std::sync::{Arc, Mutex};

use super::{
    tool_error, Approval, ApprovalHook, RiskLevel, ToolAction, ToolCallError, REFUSAL_NOTE,
};
use crate::configuration::{site_matches, BrowserConfig, SiteAccess, SiteException};

/// One of the three Settings › Browser permissions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permission {
    Browsing,
    Downloads,
    Debugging,
}

impl Permission {
    fn label(self) -> &'static str {
        match self {
            Self::Browsing => "Browsing",
            Self::Downloads => "Downloads",
            Self::Debugging => "Debugging",
        }
    }

    fn of(self, exception: &SiteException) -> Option<SiteAccess> {
        match self {
            Self::Browsing => exception.browsing,
            Self::Downloads => exception.downloads,
            Self::Debugging => exception.debugging,
        }
    }

    fn set(self, exception: &mut SiteException, access: SiteAccess) {
        match self {
            Self::Browsing => exception.browsing = Some(access),
            Self::Downloads => exception.downloads = Some(access),
            Self::Debugging => exception.debugging = Some(access),
        }
    }
}

/// Saves "Always allow on this site" for a permission (the person's Settings).
pub type RememberSite = Arc<dyn Fn(&str, Permission) -> Result<(), String> + Send + Sync>;

/// A page the agent's own work lives on: localhost and loopback addresses, and pages
/// without a host (`about:blank`, `data:`, files Themis opened).
#[must_use]
pub fn is_local(site: &str) -> bool {
    let host = site.trim_end_matches('.').to_ascii_lowercase();
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    host.is_empty()
        || host == "about:blank"
        || host == "localhost"
        || host.ends_with(".localhost")
        || host == "0.0.0.0"
        || bare == "::1"
        || bare
            .parse::<std::net::Ipv4Addr>()
            .is_ok_and(|address| address.is_loopback())
}

/// The run's browser permissions, captured from configuration; "Always allow on this site"
/// also updates them for the rest of the run.
pub struct SitePermissions {
    config: Mutex<BrowserConfig>,
    remember: Option<RememberSite>,
}

impl SitePermissions {
    pub fn new(config: BrowserConfig, remember: Option<RememberSite>) -> Arc<Self> {
        Arc::new(Self {
            config: Mutex::new(config),
            remember,
        })
    }

    /// The configuration as it stands now (with this run's "always allow" answers).
    pub fn config(&self) -> BrowserConfig {
        self.config
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Allow, Ask or Block for `permission` on `site`: local is Allow; a project
    /// restriction that matches blocks; else the most specific site exception that sets the
    /// permission; else the default.
    pub fn decide(&self, permission: Permission, site: &str) -> SiteAccess {
        if is_local(site) {
            return SiteAccess::Allow;
        }
        let config = self.config.lock().unwrap_or_else(|e| e.into_inner());
        if config.site_restrictions.iter().any(|restriction| {
            site_matches(site, &restriction.site)
                && permission.of(restriction) == Some(SiteAccess::Block)
        }) {
            return SiteAccess::Block;
        }
        config
            .site_exceptions
            .iter()
            .filter(|exception| site_matches(site, &exception.site))
            .filter_map(|exception| {
                permission
                    .of(exception)
                    .map(|access| (exception.site.len(), access))
            })
            .max_by_key(|(specificity, _)| *specificity)
            .map(|(_, access)| access)
            .unwrap_or(match permission {
                Permission::Browsing => config.permissions.browsing,
                Permission::Downloads => config.permissions.downloads,
                Permission::Debugging => config.permissions.debugging,
            })
    }

    /// Decides `permission` for `tool` on `site`, asking the person when the answer is Ask.
    /// Block refuses in every approval mode; Allow skips the prompt (a deny rule in
    /// `runtime.jsonc` still refuses in Custom); "Always allow on this site" is saved.
    pub fn gate(
        &self,
        approvals: &dyn ApprovalHook,
        tool: &str,
        permission: Permission,
        site: &str,
        summary: String,
        risk: RiskLevel,
    ) -> Result<(), ToolCallError> {
        let action = ToolAction {
            display: None,
            tool: tool.to_owned(),
            summary,
            risk,
            command: vec![site.to_owned()],
        };
        let refused = || {
            tool_error(format!(
                "approval denied for '{tool}': {} {REFUSAL_NOTE}",
                action.summary
            ))
        };
        let access = self.decide(permission, site);
        // Every decision is audited (metadata only; never the site), prompted or not.
        crate::diagnostics::emit(
            "browser",
            "permission_decided",
            "info",
            serde_json::json!({"tool": tool, "permission": permission.label(), "access": format!("{access:?}").to_ascii_lowercase()}),
        );
        match access {
            SiteAccess::Block => Err(tool_error(format!(
                "{} is blocked for {site} in Settings › Browser. {REFUSAL_NOTE}",
                permission.label()
            ))),
            SiteAccess::Allow if approvals.denied_by_rule(&action) => Err(refused()),
            SiteAccess::Allow => Ok(()),
            SiteAccess::Ask => match approvals.approve(&action) {
                Approval::AllowOnce => Ok(()),
                Approval::AllowAlways => {
                    self.allow_always(site, permission);
                    Ok(())
                }
                Approval::Deny => Err(refused()),
            },
        }
    }

    /// Records an Allow exception for `site` now and in the person's Settings.
    fn allow_always(&self, site: &str, permission: Permission) {
        {
            let mut config = self.config.lock().unwrap_or_else(|e| e.into_inner());
            match config
                .site_exceptions
                .iter_mut()
                .find(|exception| exception.site == site)
            {
                Some(exception) => permission.set(exception, SiteAccess::Allow),
                None => {
                    let mut exception = SiteException {
                        site: site.to_owned(),
                        ..SiteException::default()
                    };
                    permission.set(&mut exception, SiteAccess::Allow);
                    config.site_exceptions.push(exception);
                }
            }
        }
        if let Some(remember) = &self.remember {
            if let Err(error) = remember(site, permission) {
                crate::diagnostics::emit(
                    "browser",
                    "site_exception_not_saved",
                    "warn",
                    serde_json::json!({"permission": permission.label(), "error_chars": error.len()}),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::configuration::BrowserPermissions;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Answer(Approval, AtomicUsize, bool);
    impl ApprovalHook for Answer {
        fn approve(&self, _action: &ToolAction) -> Approval {
            self.1.fetch_add(1, Ordering::SeqCst);
            self.0
        }
        fn denied_by_rule(&self, _action: &ToolAction) -> bool {
            self.2
        }
    }
    fn hook(answer: Approval) -> Answer {
        Answer(answer, AtomicUsize::new(0), false)
    }
    fn gate(
        permissions: &SitePermissions,
        hook: &Answer,
        permission: Permission,
        site: &str,
    ) -> Result<(), String> {
        permissions
            .gate(
                hook,
                "browser_open",
                permission,
                site,
                format!("open {site}"),
                RiskLevel::Network,
            )
            .map_err(|error| error.to_string())
    }

    #[test]
    fn defaults_allow_browsing_and_downloads_and_block_debugging_except_locally() {
        let permissions = SitePermissions::new(BrowserConfig::default(), None);
        assert_eq!(
            permissions.decide(Permission::Browsing, "example.com"),
            SiteAccess::Allow
        );
        assert_eq!(
            permissions.decide(Permission::Downloads, "example.com"),
            SiteAccess::Allow
        );
        assert_eq!(
            permissions.decide(Permission::Debugging, "example.com"),
            SiteAccess::Block
        );
        for local in [
            "localhost",
            "app.localhost",
            "127.0.0.1",
            "127.1.2.3",
            "[::1]",
            "0.0.0.0",
            "about:blank",
            "",
        ] {
            assert_eq!(
                permissions.decide(Permission::Debugging, local),
                SiteAccess::Allow,
                "{local}"
            );
        }
        // The starter exceptions block sign-in and payment sites, subdomains included.
        assert_eq!(
            permissions.decide(Permission::Browsing, "www.paypal.com"),
            SiteAccess::Block
        );
        assert_eq!(
            permissions.decide(Permission::Browsing, "notpaypal.com"),
            SiteAccess::Allow
        );
    }

    #[test]
    fn the_most_specific_exception_wins_and_project_restrictions_always_block() {
        let config = BrowserConfig {
            permissions: BrowserPermissions {
                browsing: SiteAccess::Block,
                ..BrowserPermissions::default()
            },
            site_exceptions: vec![
                SiteException {
                    site: "example.com".into(),
                    browsing: Some(SiteAccess::Allow),
                    ..Default::default()
                },
                SiteException {
                    site: "pay.example.com".into(),
                    browsing: Some(SiteAccess::Ask),
                    ..Default::default()
                },
            ],
            site_restrictions: vec![SiteException {
                site: "admin.example.com".into(),
                browsing: Some(SiteAccess::Block),
                ..Default::default()
            }],
            ..BrowserConfig::default()
        };
        let permissions = SitePermissions::new(config, None);
        assert_eq!(
            permissions.decide(Permission::Browsing, "docs.example.com"),
            SiteAccess::Allow
        );
        assert_eq!(
            permissions.decide(Permission::Browsing, "pay.example.com"),
            SiteAccess::Ask
        );
        assert_eq!(
            permissions.decide(Permission::Browsing, "admin.example.com"),
            SiteAccess::Block
        );
        // Lockdown: anything else falls to the Block default.
        assert_eq!(
            permissions.decide(Permission::Browsing, "other.org"),
            SiteAccess::Block
        );
        // An exception that leaves a permission unset does not change it.
        assert_eq!(
            permissions.decide(Permission::Debugging, "example.com"),
            SiteAccess::Block
        );
    }

    #[test]
    fn block_refuses_without_asking_allow_skips_the_prompt_and_ask_asks() {
        let permissions = SitePermissions::new(BrowserConfig::default(), None);
        let asked = hook(Approval::AllowOnce);
        let error = gate(&permissions, &asked, Permission::Browsing, "paypal.com").unwrap_err();
        assert!(
            error.contains("Browsing is blocked for paypal.com in Settings › Browser"),
            "{error}"
        );
        assert_eq!(
            asked.1.load(Ordering::SeqCst),
            0,
            "Block never asks, so YOLO cannot allow it"
        );
        gate(&permissions, &asked, Permission::Browsing, "example.com").unwrap();
        assert_eq!(asked.1.load(Ordering::SeqCst), 0, "Allow never asks");
        let denying_rule = Answer(Approval::AllowOnce, AtomicUsize::new(0), true);
        assert!(gate(
            &permissions,
            &denying_rule,
            Permission::Browsing,
            "example.com"
        )
        .is_err());
        let config = BrowserConfig {
            permissions: BrowserPermissions {
                debugging: SiteAccess::Ask,
                ..BrowserPermissions::default()
            },
            ..BrowserConfig::default()
        };
        let permissions = SitePermissions::new(config, None);
        gate(&permissions, &asked, Permission::Debugging, "example.com").unwrap();
        assert_eq!(asked.1.load(Ordering::SeqCst), 1);
        assert!(gate(
            &permissions,
            &hook(Approval::Deny),
            Permission::Debugging,
            "example.com"
        )
        .is_err());
    }

    #[test]
    fn always_allow_on_this_site_is_kept_for_the_run_and_saved() {
        let saved = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&saved);
        let remember: RememberSite = Arc::new(move |site, permission| {
            sink.lock().unwrap().push((site.to_owned(), permission));
            Ok(())
        });
        let config = BrowserConfig {
            permissions: BrowserPermissions {
                debugging: SiteAccess::Ask,
                ..BrowserPermissions::default()
            },
            ..BrowserConfig::default()
        };
        let permissions = SitePermissions::new(config, Some(remember));
        let always = hook(Approval::AllowAlways);
        gate(&permissions, &always, Permission::Debugging, "example.com").unwrap();
        gate(
            &permissions,
            &always,
            Permission::Debugging,
            "docs.example.com",
        )
        .unwrap();
        assert_eq!(
            always.1.load(Ordering::SeqCst),
            1,
            "the second call is covered by the saved exception"
        );
        assert_eq!(
            *saved.lock().unwrap(),
            [("example.com".to_owned(), Permission::Debugging)]
        );
        assert_eq!(
            permissions.decide(Permission::Browsing, "example.com"),
            SiteAccess::Allow
        );
    }

    proptest::proptest! {
        #[test]
        fn local_pages_are_never_refused_and_blocks_never_ask(site in "[a-z0-9.:\\[\\]-]{0,40}") {
            let config = BrowserConfig {
                permissions: BrowserPermissions {
                    browsing: SiteAccess::Block,
                    downloads: SiteAccess::Block,
                    debugging: SiteAccess::Block,
                },
                ..BrowserConfig::default()
            };
            let permissions = SitePermissions::new(config, None);
            let asked = hook(Approval::AllowOnce);
            let result = gate(&permissions, &asked, Permission::Browsing, &site);
            proptest::prop_assert_eq!(result.is_ok(), is_local(&site));
            proptest::prop_assert_eq!(asked.1.load(Ordering::SeqCst), 0);
        }
    }
}
