//! App settings persisted as JSON (`settings.json` under the app data dir).

use std::path::{Path, PathBuf};

use crate::types::{Settings, SettingsPatch, ThemeMode};

pub const THEME_PALETTES: &[&str] = &[
    "system",
    "warm",
    "violet",
    "github-dark",
    "github-dimmed",
    "midnight",
    "nordic",
    "graphite",
    "ink",
    "forest",
    "copper",
    "plum",
    "ocean",
    "rosewood",
    "olive",
    "amethyst",
    "terminal",
    "sandstone",
    "carbon",
];
pub const FONT_FAMILIES: &[&str] = &[
    "system",
    "sans",
    "serif",
    "mono",
    "rounded",
    "avenir",
    "helvetica",
    "verdana",
    "trebuchet",
    "palatino",
    "charter",
    "menlo",
];

/// The context cap before Auto existed defaulted to this; a saved value equal
/// to it migrates to Auto (which is the same budget at the current ceiling).
pub const LEGACY_DEFAULT_CONTEXT_BUDGET: u64 = 200_000;
pub const APPROVAL_TIMEOUT_MIN: i64 = 30;
pub const APPROVAL_TIMEOUT_MAX: i64 = 600;

/// Inclusive bounds for `concurrency_limit` updates.
pub const CONCURRENCY_MIN: i64 = 1;
/// Inclusive bounds for `concurrency_limit` updates.
pub const CONCURRENCY_MAX: i64 = 16;

/// Maximum project roots remembered in `recent_roots`.
pub const MAX_RECENT_ROOTS: usize = 10;

/// Loads, caches, and saves [`Settings`].
///
/// A missing or corrupt file yields [`Settings::default`]; every successful
/// update is written back immediately.
pub struct SettingsStore {
    path: PathBuf,
    projects_root: PathBuf,
    current: tokio::sync::Mutex<Settings>,
}

/// Inclusive bounds of a numeric context cap.
/// The caps Settings offer: 200,000 up (each model is clamped to its window, so a smaller
/// model gets its whole window). Smaller budgets are for evals, in `runtime.jsonc`.
fn context_cap_bounds() -> (i64, i64) {
    (
        themis_core::context_window::MIN_SETTINGS_CONTEXT_CAP as i64,
        themis_core::context_window::MAX_CONTEXT_TOKENS as i64,
    )
}

/// The context cap a settings file from before Auto implies, when it has no
/// `context_token_cap` yet: its old `context_token_budget` default becomes
/// Auto (`0`); any other saved number stays as a cap.
fn legacy_context_cap(value: &serde_json::Value) -> Option<u32> {
    if value.get("context_token_cap").is_some() {
        return None;
    }
    let budget = value.get("context_token_budget")?.as_u64()?;
    if budget == LEGACY_DEFAULT_CONTEXT_BUDGET {
        return Some(0);
    }
    let (_low, high) = context_cap_bounds();
    u32::try_from(budget.min(high as u64)).ok()
}

impl SettingsStore {
    /// Loads settings from `path`, falling back to defaults.
    #[must_use]
    pub fn load(path: PathBuf) -> Self {
        Self::load_with_projects_root(
            path,
            PathBuf::from(crate::types::default_projects_directory()),
        )
    }

    pub(crate) fn load_with_projects_root(path: PathBuf, projects_root: PathBuf) -> Self {
        let saved: Option<serde_json::Value> = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok());
        // A default model saved with a retired provider means nothing to OpenCode Go.
        let retired_provider = saved
            .as_ref()
            .and_then(|value| value.get("default_provider")?.as_str())
            .is_some_and(|provider| provider != crate::types::ProviderKind::Go.as_str());
        let cap = saved.as_ref().and_then(legacy_context_cap);
        let mut settings: Settings = saved
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_default();
        if let Some(cap) = cap {
            settings.context_token_cap = cap;
        }
        // A cap saved before the 200,000 floor (or a legacy budget below it) becomes Auto.
        let (low, _high) = context_cap_bounds();
        if settings.context_token_cap != 0 && i64::from(settings.context_token_cap) < low {
            settings.context_token_cap = 0;
        }
        settings.theme = ThemeMode::Dark;
        settings.projects_directory = projects_root.to_string_lossy().into_owned();
        if retired_provider {
            settings.default_model.clear();
        }
        Self {
            path,
            projects_root,
            current: tokio::sync::Mutex::new(settings),
        }
    }

    /// Returns the file this store persists to.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns a snapshot of the current settings.
    pub async fn get(&self) -> Settings {
        self.current.lock().await.clone()
    }

    /// Applies `patch`, validates, persists, and returns the new settings.
    pub async fn update(&self, patch: SettingsPatch) -> Result<Settings, String> {
        let mut guard = self.current.lock().await;
        let mut current = guard.clone();
        let snapshot = {
            if let Some(directory) = patch.projects_directory {
                if Path::new(&directory) != self.projects_root {
                    return Err("Projects root is fixed".to_owned());
                }
            }
            if let Some(size) = patch.text_size {
                if !(12..=22).contains(&size) {
                    return Err("Text size must be between 12 and 22".to_owned());
                }
                current.text_size = size;
            }
            if let Some(palette) = patch.theme_palette {
                if !THEME_PALETTES.contains(&palette.as_str()) {
                    return Err("Unknown theme palette".to_owned());
                }
                current.theme_palette = palette;
            }
            if let Some(font) = patch.font_family {
                if !FONT_FAMILIES.contains(&font.as_str()) {
                    return Err("Unknown font family".to_owned());
                }
                current.font_family = font;
            }
            if let Some(hover) = patch.sidebar_hover {
                current.sidebar_hover = hover;
            }
            if let Some(total) = patch.max_total_turns {
                if !(1..=2000).contains(&total) {
                    return Err("max_total_turns must be between 1 and 2000".to_owned());
                }
                current.max_total_turns = total as u32;
            }
            if let Some(cap) = patch.context_token_cap {
                let (low, high) = context_cap_bounds();
                if cap != 0 && !(low..=high).contains(&cap) {
                    return Err(format!(
                        "context_token_cap must be Auto or between {low} and {high}"
                    ));
                }
                current.context_token_cap = cap as u32;
            }
            if let Some(seconds) = patch.approval_timeout_seconds {
                if !(APPROVAL_TIMEOUT_MIN..=APPROVAL_TIMEOUT_MAX).contains(&seconds) {
                    return Err(format!("approval_timeout_seconds must be between {APPROVAL_TIMEOUT_MIN} and {APPROVAL_TIMEOUT_MAX}"));
                }
                current.approval_timeout_seconds = seconds as u32;
            }
            if let Some(confirm) = patch.confirm_reads {
                current.confirm_reads = confirm;
            }
            // Accept old clients' light/system requests, always return dark.
            current.theme = ThemeMode::Dark;
            if let Some(provider) = patch.default_provider {
                current.default_provider = provider;
            }
            if let Some(model) = patch.default_model {
                current.default_model = model;
            }
            if let Some(limit) = patch.concurrency_limit {
                if !(CONCURRENCY_MIN..=CONCURRENCY_MAX).contains(&limit) {
                    return Err(format!(
                        "concurrency_limit must be between {CONCURRENCY_MIN} and {CONCURRENCY_MAX} \
                         (got {limit})"
                    ));
                }
                current.concurrency_limit = u32::try_from(limit).map_err(|_| {
                    format!(
                        "concurrency_limit must be between {CONCURRENCY_MIN} and {CONCURRENCY_MAX}"
                    )
                })?;
            }
            if let Some(roots) = patch.recent_roots {
                current.recent_roots = roots.into_iter().take(MAX_RECENT_ROOTS).collect();
            }
            if let Some(enabled) = patch.automations_enabled {
                current.automations_enabled = enabled;
            }
            if let Some(enabled) = patch.completion_sound {
                current.completion_sound = enabled;
            }
            if let Some(enabled) = patch.completion_haptic {
                current.completion_haptic = enabled;
            }
            if let Some(enabled) = patch.goal_notifications {
                current.goal_notifications = enabled;
            }
            if let Some(mode) = patch.default_approval_mode {
                current.default_approval_mode = mode;
            }
            if let Some(editor) = patch.config_editor {
                if !crate::state::editors::is_known(&editor) {
                    return Err("Unknown editor".to_owned());
                }
                current.config_editor = editor;
            }
            if let Some(secret) = patch.go_secret {
                if !themis_core::plugins::keys::valid_name(&secret) {
                    return Err("Secret names use letters, digits and _ (up to 128)".to_owned());
                }
                current.go_secret = secret;
            }
            if let Some(onboarded) = patch.onboarded {
                current.onboarded = onboarded;
            }
            current.clone()
        };
        self.save(&snapshot)?;
        *guard = snapshot.clone();
        Ok(snapshot)
    }

    pub(crate) async fn rename_project(&self, root: String, name: String) -> Result<(), String> {
        let mut guard = self.current.lock().await;
        let mut next = guard.clone();
        next.project_names.insert(root, name);
        self.save(&next)?;
        *guard = next;
        Ok(())
    }

    /// Drops a deleted project from recents and display names.
    pub(crate) async fn forget_project(&self, root: &str) -> Result<(), String> {
        let mut guard = self.current.lock().await;
        let mut next = guard.clone();
        next.recent_roots.retain(|recent| recent != root);
        next.project_names.remove(root);
        self.save(&next)?;
        *guard = next;
        Ok(())
    }

    fn save(&self, settings: &Settings) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|err| format!("failed to create settings dir: {err}"))?;
        }
        let text = serde_json::to_string_pretty(settings)
            .map_err(|err| format!("failed to encode settings: {err}"))?;
        let temporary = self.path.with_extension("json.tmp");
        std::fs::write(&temporary, text)
            .map_err(|err| format!("failed to save settings: {err}"))?;
        std::fs::rename(&temporary, &self.path)
            .map_err(|err| format!("failed to replace settings: {err}"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ProviderKind, ThemeMode};

    #[test]
    fn missing_file_yields_defaults() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = SettingsStore::load(dir.path().join("settings.json"));
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        assert_eq!(runtime.block_on(store.get()), Settings::default());
    }

    #[tokio::test]
    async fn save_load_roundtrip() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nested").join("settings.json");
        let store = SettingsStore::load(path.clone());
        let updated = store
            .update(SettingsPatch {
                theme: Some(ThemeMode::Dark),
                default_provider: Some(ProviderKind::Go),
                default_model: Some("gpt-x".to_owned()),
                recent_roots: Some(vec!["/tmp/a".to_owned()]),
                concurrency_limit: Some(5),
                automations_enabled: Some(false),
                completion_sound: Some(true),
                completion_haptic: Some(true),
                goal_notifications: Some(false),
                onboarded: Some(true),
                ..SettingsPatch::default()
            })
            .await
            .expect("update");
        assert_eq!(updated.theme, ThemeMode::Dark);
        assert_eq!(updated.default_provider, ProviderKind::Go);
        assert_eq!(updated.default_model, "gpt-x");
        assert_eq!(updated.recent_roots, vec!["/tmp/a".to_owned()]);
        assert_eq!(updated.concurrency_limit, 5);
        assert!(!updated.automations_enabled);
        assert!(updated.completion_sound);
        assert!(updated.completion_haptic);
        assert!(!updated.goal_notifications);
        assert!(updated.onboarded);
        assert!(path.exists(), "settings file was written");

        // A fresh store over the same path sees the saved values.
        let reopened = SettingsStore::load(path);
        assert_eq!(reopened.get().await, updated);
    }

    #[tokio::test]
    async fn context_and_approval_limits_validate_and_survive_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let store = SettingsStore::load(path.clone());
        for bad in [1, 1999, 64_000, 199_999, 1_000_001, -5] {
            let error = store
                .update(SettingsPatch {
                    context_token_cap: Some(bad),
                    ..Default::default()
                })
                .await
                .unwrap_err();
            assert!(error.contains("context_token_cap"), "{error}");
        }
        assert!(store
            .update(SettingsPatch {
                approval_timeout_seconds: Some(0),
                ..Default::default()
            })
            .await
            .is_err());
        let saved = store
            .update(SettingsPatch {
                context_token_cap: Some(1_000_000),
                approval_timeout_seconds: Some(90),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(saved.context_token_cap, 1_000_000);
        assert_eq!(saved.approval_timeout_seconds, 90);
        assert_eq!(SettingsStore::load(path.clone()).get().await, saved);
        let auto = store
            .update(SettingsPatch {
                context_token_cap: Some(0),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(auto.context_token_cap, 0);
        assert_eq!(SettingsStore::load(path).get().await, auto);
    }

    #[test]
    fn saved_context_budget_migrates_to_auto_or_a_cap() {
        let dir = tempfile::tempdir().unwrap();
        let load = |text: &str| {
            let path = dir.path().join("settings.json");
            std::fs::write(&path, text).unwrap();
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .unwrap();
            runtime.block_on(SettingsStore::load(path).get())
        };
        let base = r#""theme":"dark","default_provider":"go","default_model":"m""#;
        // The old default (and an old file with no budget at all) is Auto.
        assert_eq!(load(&format!("{{{base},\"max_turns\":20,\"context_token_budget\":200000,\"context_messages\":20}}")).context_token_cap, 0);
        assert_eq!(load(&format!("{{{base}}}")).context_token_cap, 0);
        // A deliberately different number stays a cap, from the 200,000 floor up; below it is Auto.
        assert_eq!(
            load(&format!("{{{base},\"context_token_budget\":64000}}")).context_token_cap,
            0
        );
        assert_eq!(
            load(&format!("{{{base},\"context_token_cap\":64000}}")).context_token_cap,
            0
        );
        assert_eq!(
            load(&format!("{{{base},\"context_token_budget\":256000}}")).context_token_cap,
            256000
        );
        // Once the new field exists, the legacy one no longer applies.
        assert_eq!(
            load(&format!(
                "{{{base},\"context_token_budget\":64000,\"context_token_cap\":0}}"
            ))
            .context_token_cap,
            0
        );
    }

    #[tokio::test]
    async fn corrupt_file_yields_defaults() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("settings.json");
        std::fs::write(&path, "{ not json").expect("write");
        let store = SettingsStore::load(path);
        assert_eq!(store.get().await, Settings::default());
    }

    #[tokio::test]
    async fn legacy_file_without_new_fields_loads_defaults() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("settings.json");
        std::fs::write(
            &path,
            r#"{"theme":"dark","default_provider":"openai","default_model":"gpt-x","max_turns":7}"#,
        )
        .expect("write");
        let store = SettingsStore::load(path);
        let settings = store.get().await;
        assert_eq!(settings.theme, ThemeMode::Dark);
        assert_eq!(settings.default_provider, ProviderKind::Go);
        assert!(settings.default_model.is_empty());
        assert!(settings.recent_roots.is_empty());
        assert_eq!(settings.concurrency_limit, 3);
        assert!(settings.automations_enabled);
        assert!(!settings.completion_sound);
        assert!(!settings.completion_haptic);
        assert!(settings.goal_notifications);
        assert!(!settings.onboarded);
    }

    #[tokio::test]
    async fn concurrency_limit_is_validated() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = SettingsStore::load(dir.path().join("settings.json"));
        for bad in [0, -2, 17, 100] {
            let err = store
                .update(SettingsPatch {
                    concurrency_limit: Some(bad),
                    ..SettingsPatch::default()
                })
                .await
                .expect_err("rejected");
            assert!(err.contains("concurrency_limit"), "{err}");
        }
        assert_eq!(store.get().await.concurrency_limit, 3);
        for good in [1, 16] {
            let ok = store
                .update(SettingsPatch {
                    concurrency_limit: Some(good),
                    ..SettingsPatch::default()
                })
                .await
                .expect("accepted");
            assert_eq!(ok.concurrency_limit, u32::try_from(good).expect("u32"));
        }
    }
}

#[cfg(test)]
mod preference_tests {
    use super::*;
    #[tokio::test]
    async fn every_preset_and_font_survives_reload() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let store = SettingsStore::load(path.clone());
        for palette in THEME_PALETTES {
            store
                .update(SettingsPatch {
                    theme_palette: Some((*palette).to_owned()),
                    ..Default::default()
                })
                .await
                .unwrap();
            assert_eq!(
                SettingsStore::load(path.clone()).get().await.theme_palette,
                *palette
            );
        }
        for font in FONT_FAMILIES {
            store
                .update(SettingsPatch {
                    font_family: Some((*font).to_owned()),
                    ..Default::default()
                })
                .await
                .unwrap();
            assert_eq!(
                SettingsStore::load(path.clone()).get().await.font_family,
                *font
            );
        }
    }
    #[tokio::test]
    async fn rejected_or_unwritable_updates_preserve_memory_and_disk() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("settings.json");
        let store = SettingsStore::load(path.clone());
        let saved = store
            .update(SettingsPatch {
                text_size: Some(16),
                sidebar_hover: Some(false),
                ..Default::default()
            })
            .await
            .unwrap();
        assert!(store
            .update(SettingsPatch {
                text_size: Some(10),
                theme: Some(crate::types::ThemeMode::Dark),
                ..Default::default()
            })
            .await
            .is_err());
        assert!(store
            .update(SettingsPatch {
                projects_directory: Some("../escape".to_owned()),
                ..Default::default()
            })
            .await
            .is_err());
        assert_eq!(store.get().await, saved);
        assert_eq!(SettingsStore::load(path.clone()).get().await, saved);
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(store
            .update(SettingsPatch {
                text_size: Some(18),
                ..Default::default()
            })
            .await
            .is_err());
        assert_eq!(store.get().await, saved);
    }
}
