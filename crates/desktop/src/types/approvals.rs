//! Approval requests, site grants and expiry notices.

use super::{RiskLevel, SubagentLabel};
use serde::{Deserialize, Serialize};

/// `approval-request` payload (`ApprovalRequest` in types.ts).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalRequest {
    pub thread_id: String,
    pub approval_id: String,
    pub tool: String,
    pub summary: String,
    pub risk: RiskLevel,
    /// What "allow in this chat" covers for `shell`, as raw argv tokens (the
    /// UI must render them as data); absent for tools granted per tool name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grant_scope: Option<themis_core::tools::GrantScope>,
    /// For web and browser tools: the site "allow in this chat" covers, and
    /// for which tools (`browser` = every browser action on that site).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grant_site: Option<GrantSite>,
    /// False for destructive actions: they ask every time, so the dialog
    /// offers only "Allow once" and "Deny".
    #[serde(default = "default_grantable")]
    pub grantable: bool,
    /// Structured command for shell/git approvals; the UI shows this instead
    /// of the joined `summary`, which can be spoofed by argument contents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<themis_core::tools::CommandDisplay>,
    /// Set when a subagent, not the main agent, asks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagent: Option<SubagentLabel>,
    /// The sensitive project files this action touches: "allow in this chat" covers these
    /// (and, for a command, that command), not the whole tool.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sensitive_files: Vec<String>,
}

/// What a site grant covers (`GrantSite` in types.ts).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrantSite {
    /// `browser`, `fetch_url` or `web_search`.
    pub tools: String,
    pub site: String,
}

const fn default_grantable() -> bool {
    true
}

/// `approval-expired` payload: the dialog for `approval_id` was denied without
/// an answer, so the UI removes it (`ApprovalExpired` in types.ts): it timed
/// out, or (`steered`) the person sent a new message past it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalExpired {
    pub thread_id: String,
    pub approval_id: String,
    #[serde(default)]
    pub steered: bool,
}
