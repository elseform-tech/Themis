//! Versioned runtime configuration and immutable per-run approval policy.
use crate::chat_grants::ChatGrants;
use crate::tools::sensitive::{SensitiveAccess, SensitiveMatcher, SENSITIVE_TOOL};
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

mod approval;
mod approval_hook;
mod guidance;
mod jsonc;
mod resolve;
mod schema;
mod sections;
mod web_browser;
pub use approval::*;
pub use approval_hook::*;
pub use guidance::*;
pub use jsonc::*;
pub use resolve::*;
pub use schema::*;
pub use sections::*;
pub use web_browser::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RuntimeConfig {
    pub skills: SkillConfig,
    pub instructions: Vec<String>,
    /// Read the project's AGENTS.md (or CLAUDE.md) and list its other docs for agents and the goal check.
    pub project_guidance: bool,
    pub logging: LoggingConfig,
    #[serde(rename = "$schema", skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    pub schema_version: u32,
    pub approval: ApprovalPolicy,
    /// OS sandbox for commands the agent runs; see [`crate::sandbox`].
    pub sandbox: crate::sandbox::SandboxConfig,
    pub compaction: CompactionConfig,
    /// Subagents; see [`SubagentsConfig`].
    pub subagents: SubagentsConfig,
    /// Plugin hooks; see [`HooksConfig`].
    pub hooks: HooksConfig,
    /// Memory across chats; see [`MemoryConfig`].
    pub memory: MemoryConfig,
    /// `web_search` backend; see [`WebConfig`].
    pub web: WebConfig,
    /// The built-in browser; see [`BrowserConfig`].
    pub browser: BrowserConfig,
    /// OpenAI-compatible providers besides OpenCode Go; user file only.
    pub providers: Vec<ProviderDefinition>,
    /// Per-model overrides keyed by model id or `<provider>/<id>`.
    pub models: BTreeMap<String, ModelOverride>,
    /// `"auto"` or a cap in tokens; omitted falls back to the app setting
    /// (Auto by default). See [`ContextTokenBudget`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_token_budget: Option<ContextTokenBudget>,
    /// Earlier messages sent with each request; file-only (default
    /// [`DEFAULT_RECENT_MESSAGES`]).
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
            project_guidance: true,
            logging: LoggingConfig::default(),
            schema: None,
            schema_version: 1,
            approval: ApprovalPolicy::default(),
            sandbox: crate::sandbox::SandboxConfig::default(),
            compaction: CompactionConfig::default(),
            subagents: SubagentsConfig::default(),
            hooks: HooksConfig::default(),
            memory: MemoryConfig::default(),
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
/// Earlier messages sent with each request when `recent_messages` is omitted.
pub const DEFAULT_RECENT_MESSAGES: usize = 20;
/// Smallest configurable context cap.
pub const MIN_CONTEXT_TOKENS: usize = 2_000;

/// How much working context a run may use (`context_token_budget`).
///
/// `Auto` follows the selected model's context window up to
/// [`crate::context_window::AUTO_CONTEXT_CEILING`]; `Tokens(n)` is a cap that
/// is still clamped to the window. Written as `"auto"` or a number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ContextTokenBudget {
    #[default]
    Auto,
    Tokens(usize),
}

impl ContextTokenBudget {
    /// The cap a run applies before clamping to the model's window.
    #[must_use]
    pub const fn cap(self) -> usize {
        match self {
            Self::Auto => crate::context_window::AUTO_CONTEXT_CEILING,
            Self::Tokens(tokens) => tokens,
        }
    }
}

impl Serialize for ContextTokenBudget {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Auto => serializer.serialize_str("auto"),
            Self::Tokens(tokens) => serializer.serialize_u64(*tokens as u64),
        }
    }
}

impl<'de> Deserialize<'de> for ContextTokenBudget {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Tokens(usize),
            Text(String),
        }
        match Raw::deserialize(deserializer)? {
            Raw::Tokens(tokens) => Ok(Self::Tokens(tokens)),
            Raw::Text(text) if text == "auto" => Ok(Self::Auto),
            Raw::Text(_) => Err(serde::de::Error::custom(
                "context_token_budget must be \"auto\" or a number of tokens",
            )),
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

impl ResolvedConfig {
    /// The sandbox settings YOLO keeps: those some file or override wrote.
    pub fn yolo_sandbox(&self) -> crate::sandbox::YoloSandbox {
        let written = |key: &str| {
            self.provenance
                .get(key)
                .is_some_and(|origin| origin != "default")
        };
        crate::sandbox::YoloSandbox {
            network_written: written("sandbox.network"),
            keychain_written: written("sandbox.allow_keychain"),
        }
    }
}

/// Every runtime setting with its allowed values and default, as commented JSONC. A new
/// user `runtime.jsonc` carries it with every setting commented out.
pub const REFERENCE: &str = include_str!("configuration_reference.jsonc");

#[cfg(test)]
mod approval_tests;
#[cfg(test)]
mod settings_tests;
