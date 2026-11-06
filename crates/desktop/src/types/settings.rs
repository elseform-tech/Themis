//! App settings, secrets status, diagnostics and updater state.

use super::{ProviderKind, ThemeMode};
use serde::{Deserialize, Serialize};

/// Default `concurrency_limit` (also the serde fallback for settings files
/// written before the field existed; a plain `#[serde(default)]` would yield
/// `0`, which violates the `1..=16` contract).
#[must_use]
pub const fn default_concurrency_limit() -> u32 {
    3
}

/// Default `automations_enabled`: new installs run automations, and settings
/// files written before the field existed load as enabled.
#[must_use]
pub const fn default_automations_enabled() -> bool {
    true
}

/// App settings (`Settings` in types.ts).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default = "default_projects_directory")]
    pub projects_directory: String,
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub project_names: std::collections::HashMap<String, String>,
    #[serde(default = "default_text_size")]
    pub text_size: u32,
    /// Someone picked `text_size`, so a change of default leaves it alone.
    #[serde(default)]
    pub text_size_chosen: bool,
    #[serde(default = "default_appearance_choice")]
    pub theme_palette: String,
    #[serde(default = "default_appearance_choice")]
    pub font_family: String,
    #[serde(default = "default_automations_enabled")]
    pub sidebar_hover: bool,
    /// Whether opening the app looks for plugin updates in sources not checked today.
    #[serde(default = "default_automations_enabled")]
    pub check_sources_for_updates: bool,
    pub theme: ThemeMode,
    pub default_provider: ProviderKind,
    pub default_model: String,
    #[serde(default = "default_max_total_turns")]
    pub max_total_turns: u32,
    /// Context cap in tokens; `0` is Auto (the model's window, up to
    /// [`themis_core::context_window::AUTO_CONTEXT_CEILING`]). Files from
    /// before this field carry `context_token_budget`, migrated on load
    /// (see [`crate::settings::SettingsStore`]).
    #[serde(default)]
    pub context_token_cap: u32,
    #[serde(default = "default_approval_timeout_seconds")]
    pub approval_timeout_seconds: u32,
    #[serde(default)]
    pub confirm_reads: bool,
    /// Recently opened project roots (most recent first).
    #[serde(default)]
    pub recent_roots: Vec<String>,
    /// Max parallel agent runs across all threads (`1..=16`).
    #[serde(default = "default_concurrency_limit")]
    pub concurrency_limit: u32,
    /// Global automation kill-switch (the scheduler no-ops when false).
    #[serde(default = "default_automations_enabled")]
    pub automations_enabled: bool,
    #[serde(default = "default_completion_sound")]
    pub completion_sound: bool,
    #[serde(default)]
    pub completion_haptic: bool,
    /// A desktop notification when a goal is reached or pauses while the app is not focused.
    #[serde(default = "default_goal_notifications")]
    pub goal_notifications: bool,
    /// Editor Settings › Permissions opens `runtime.jsonc` with (`""` = system text editor).
    #[serde(default)]
    pub config_editor: String,
    /// The approval mode a new chat starts in (Settings › Permissions); each chat can change it.
    #[serde(default)]
    pub default_approval_mode: themis_core::configuration::ApprovalMode,
    /// The Settings › Secrets secret OpenCode Go uses (Settings › Models › Connections).
    #[serde(default = "default_go_secret")]
    pub go_secret: String,
    /// True once the user completed onboarding (defaults to false so settings
    /// files written before the field existed load as not onboarded).
    #[serde(default)]
    pub onboarded: bool,
}

pub fn default_projects_directory() -> String {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(|home| {
            std::path::PathBuf::from(home)
                .join("ThemisOS/Projects")
                .to_string_lossy()
                .into_owned()
        })
        .unwrap_or_default()
}

fn default_appearance_choice() -> String {
    "system".to_owned()
}

pub(crate) const fn default_text_size() -> u32 {
    12
}

const fn default_completion_sound() -> bool {
    false
}

const fn default_goal_notifications() -> bool {
    true
}

pub const fn default_max_total_turns() -> u32 {
    200
}

pub const fn default_approval_timeout_seconds() -> u32 {
    300
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            projects_directory: default_projects_directory(),
            project_names: std::collections::HashMap::new(),
            text_size: default_text_size(),
            text_size_chosen: false,
            theme_palette: default_appearance_choice(),
            font_family: default_appearance_choice(),
            sidebar_hover: true,
            check_sources_for_updates: true,
            theme: ThemeMode::Dark,
            default_provider: ProviderKind::Go,
            default_model: themis_core::providers::GO_DEFAULT_MODEL.to_owned(),
            max_total_turns: default_max_total_turns(),
            context_token_cap: 0,
            approval_timeout_seconds: default_approval_timeout_seconds(),
            confirm_reads: false,
            recent_roots: Vec::new(),
            concurrency_limit: default_concurrency_limit(),
            automations_enabled: default_automations_enabled(),
            completion_sound: default_completion_sound(),
            completion_haptic: false,
            goal_notifications: default_goal_notifications(),
            config_editor: String::new(),
            default_approval_mode: Default::default(),
            go_secret: default_go_secret(),
            onboarded: false,
        }
    }
}

fn default_go_secret() -> String {
    crate::secrets::GO_KEY_NAME.to_owned()
}

/// Partial settings update (`Partial<Settings>` in types.ts).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SettingsPatch {
    pub projects_directory: Option<String>,
    pub text_size: Option<u32>,
    pub theme_palette: Option<String>,
    pub font_family: Option<String>,
    pub sidebar_hover: Option<bool>,
    pub check_sources_for_updates: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<ThemeMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_provider: Option<ProviderKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_total_turns: Option<i64>,
    /// `0` is Auto; otherwise a cap in tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_token_cap: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_timeout_seconds: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirm_reads: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recent_roots: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub concurrency_limit: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub automations_enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_sound: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_haptic: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal_notifications: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_editor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_approval_mode: Option<themis_core::configuration::ApprovalMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub go_secret: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub onboarded: Option<bool>,
}

/// Which providers have a key stored (`SecretStatus` in types.ts).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretStatus {
    pub go: bool,
}

/// One recorded command failure (`DiagnosticsError` in types.ts).
///
/// `at` is an RFC3339 timestamp; `command` is the Tauri command name
/// (`snake_case`, as invoked); `message` is the error text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticsError {
    pub at: String,
    pub command: String,
    pub message: String,
}

/// App diagnostics snapshot (`Diagnostics` in types.ts).
///
/// `settings` is a plain settings clone, which by construction never holds
/// secret values. `recent_errors` is oldest-first (newest last).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostics {
    pub app_version: String,
    pub os: String,
    pub settings: Settings,
    pub recent_errors: Vec<DiagnosticsError>,
    #[serde(default)]
    pub persistence: Vec<crate::doctor::Check>,
    #[serde(default)]
    pub recovery_notes: usize,
}

/// Updater outcome (`UpdateState` in types.ts).
///
/// Kebab-case on the wire: `disabled|up-to-date|available|error`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UpdateState {
    Disabled,
    UpToDate,
    Available,
    Error,
}

/// Result of `check_for_updates` (`UpdateStatus` in types.ts).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateStatus {
    pub state: UpdateState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl UpdateStatus {
    /// Updater not configured (no endpoint+pubkey in `plugins.updater`).
    #[must_use]
    pub fn disabled(message: impl Into<String>) -> Self {
        Self {
            state: UpdateState::Disabled,
            version: None,
            notes: None,
            message: Some(message.into()),
        }
    }
}
