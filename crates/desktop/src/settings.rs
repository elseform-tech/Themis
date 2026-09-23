//! App settings persisted as JSON (`settings.json` under the app data dir).

use std::path::{Path, PathBuf};

use crate::types::{Settings, SettingsPatch};

/// Inclusive bounds for `max_turns` updates.
pub const MAX_TURNS_MIN: i64 = 1;
/// Inclusive bounds for `max_turns` updates.
pub const MAX_TURNS_MAX: i64 = 200;

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
    current: tokio::sync::Mutex<Settings>,
}

impl SettingsStore {
    /// Loads settings from `path`, falling back to defaults.
    #[must_use]
    pub fn load(path: PathBuf) -> Self {
        let settings = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        Self {
            path,
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
                let path = Path::new(directory.trim());
                if !path.is_absolute()
                    || path
                        .components()
                        .any(|c| matches!(c, std::path::Component::ParentDir))
                    || (path.exists() && !path.is_dir())
                {
                    return Err(
                        "Projects folder must be an absolute directory path without '..'"
                            .to_owned(),
                    );
                }
                current.projects_directory = path.to_string_lossy().into_owned();
            }
            if let Some(size) = patch.text_size {
                if !(12..=18).contains(&size) {
                    return Err("Text size must be between 12 and 18".to_owned());
                }
                current.text_size = size;
            }
            if let Some(hover) = patch.sidebar_hover {
                current.sidebar_hover = hover;
            }
            if let Some(max_turns) = patch.max_turns {
                if !(MAX_TURNS_MIN..=MAX_TURNS_MAX).contains(&max_turns) {
                    return Err(format!(
                        "max_turns must be between {MAX_TURNS_MIN} and {MAX_TURNS_MAX} \
                         (got {max_turns})"
                    ));
                }
                current.max_turns = u32::try_from(max_turns).map_err(|_| {
                    format!("max_turns must be between {MAX_TURNS_MIN} and {MAX_TURNS_MAX}")
                })?;
            }
            if let Some(theme) = patch.theme {
                current.theme = theme;
            }
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
            if let Some(onboarded) = patch.onboarded {
                current.onboarded = onboarded;
            }
            current.clone()
        };
        self.save(&snapshot)?;
        *guard = snapshot.clone();
        Ok(snapshot)
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
                default_provider: Some(ProviderKind::OpenAI),
                default_model: Some("gpt-x".to_owned()),
                max_turns: Some(42),
                recent_roots: Some(vec!["/tmp/a".to_owned()]),
                concurrency_limit: Some(5),
                automations_enabled: Some(false),
                onboarded: Some(true),
                ..SettingsPatch::default()
            })
            .await
            .expect("update");
        assert_eq!(updated.theme, ThemeMode::Dark);
        assert_eq!(updated.default_provider, ProviderKind::OpenAI);
        assert_eq!(updated.default_model, "gpt-x");
        assert_eq!(updated.max_turns, 42);
        assert_eq!(updated.recent_roots, vec!["/tmp/a".to_owned()]);
        assert_eq!(updated.concurrency_limit, 5);
        assert!(!updated.automations_enabled);
        assert!(updated.onboarded);
        assert!(path.exists(), "settings file was written");

        // A fresh store over the same path sees the saved values.
        let reopened = SettingsStore::load(path);
        assert_eq!(reopened.get().await, updated);
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
        assert_eq!(settings.default_provider, ProviderKind::OpenAI);
        assert!(settings.recent_roots.is_empty());
        assert_eq!(settings.concurrency_limit, 3);
        assert!(settings.automations_enabled);
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

    #[tokio::test]
    async fn max_turns_is_validated() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = SettingsStore::load(dir.path().join("settings.json"));
        for bad in [0, -1, 201, 10_000] {
            let err = store
                .update(SettingsPatch {
                    max_turns: Some(bad),
                    ..SettingsPatch::default()
                })
                .await
                .expect_err("rejected");
            assert!(err.contains("max_turns"), "{err}");
        }
        // A rejected update leaves the previous value untouched.
        assert_eq!(store.get().await.max_turns, 20);
        for good in [1, 200] {
            let ok = store
                .update(SettingsPatch {
                    max_turns: Some(good),
                    ..SettingsPatch::default()
                })
                .await
                .expect("accepted");
            assert_eq!(ok.max_turns, u32::try_from(good).expect("u32"));
        }
    }
}

#[cfg(test)]
mod preference_tests {
    use super::*;
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
