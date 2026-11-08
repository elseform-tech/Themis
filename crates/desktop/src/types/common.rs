//! Shared enums: provider family, risk level, approval decision and theme.

use serde::{Deserialize, Serialize};
use themis_core::runtime::ApprovalDecision as CoreApprovalDecision;

/// The model provider family (`ProviderKind` in types.ts): OpenCode Go, plus the
/// OpenAI-compatible providers from `providers` in runtime.jsonc, whose models a chat names
/// as `<provider>/<model>`. Files from older versions may name a retired provider; it reads
/// as `go` (the settings store also drops a default model saved with one).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    Go,
}

impl<'de> Deserialize<'de> for ProviderKind {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(|_| Self::Go)
    }
}

impl ProviderKind {
    /// Canonical lowercase name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Go => "go",
        }
    }
}

/// Tool-action risk classification (`RiskLevel` in types.ts).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RiskLevel {
    Read,
    Write,
    Execute,
    Network,
    Destructive,
}

impl From<themis_core::tools::RiskLevel> for RiskLevel {
    fn from(value: themis_core::tools::RiskLevel) -> Self {
        match value {
            themis_core::tools::RiskLevel::Read => Self::Read,
            themis_core::tools::RiskLevel::Write => Self::Write,
            themis_core::tools::RiskLevel::Execute => Self::Execute,
            themis_core::tools::RiskLevel::Network => Self::Network,
            themis_core::tools::RiskLevel::Destructive => Self::Destructive,
        }
    }
}

/// Approval outcome (`ApprovalDecision` in types.ts: `once|chat|deny|timed_out`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ApprovalDecision {
    Once,
    /// Allow now and for every later run in this chat ("Allow in this chat").
    /// `always` is the pre-chat-grant spelling and means the same.
    #[serde(alias = "always")]
    Chat,
    Deny,
    /// Nobody answered before the wait ran out (only ever reported, never sent as an answer).
    #[serde(rename = "timed_out")]
    TimedOut,
}

impl ApprovalDecision {
    /// Maps to the `themis-core` approval decision.
    #[must_use]
    pub const fn core_approval(self) -> themis_core::tools::Approval {
        match self {
            Self::Once => themis_core::tools::Approval::AllowOnce,
            Self::Chat => themis_core::tools::Approval::AllowAlways,
            Self::Deny => themis_core::tools::Approval::Deny,
            Self::TimedOut => themis_core::tools::Approval::DenyTimedOut,
        }
    }
}

impl From<CoreApprovalDecision> for ApprovalDecision {
    fn from(value: CoreApprovalDecision) -> Self {
        match value {
            CoreApprovalDecision::AllowOnce => Self::Once,
            CoreApprovalDecision::AllowAlways => Self::Chat,
            CoreApprovalDecision::Deny => Self::Deny,
            CoreApprovalDecision::TimedOut => Self::TimedOut,
        }
    }
}

/// UI theme (`ThemeMode` in types.ts).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    #[default]
    Dark,
    Light,
    System,
}
