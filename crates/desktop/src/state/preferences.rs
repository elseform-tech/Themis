use super::*;

impl AppState {
    /// Test-only Go endpoint override for synthetic provider responses.
    pub fn set_go_base_url_override(&self, url: Option<String>) {
        *self
            .inner
            .go_base_url_override
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = url;
    }

    /// Returns the current settings.
    pub async fn get_settings(&self) -> Settings {
        self.inner.settings.get().await
    }

    /// Returns the diagnostics snapshot without exposing secret values.
    pub async fn get_diagnostics(&self) -> Diagnostics {
        self.get_diagnostics_with_depth(false).await
    }

    pub async fn get_diagnostics_with_depth(&self, deep: bool) -> Diagnostics {
        let threads = self.inner.threads.read().await.keys().cloned().collect();
        Diagnostics {
            persistence: self.inner.transcript.diagnostics(deep, &threads),
            recovery_notes: self.reconcile_report().len(),
            app_version: env!("CARGO_PKG_VERSION").to_owned(),
            os: std::env::consts::OS.to_owned(),
            settings: self.inner.settings.get().await,
            recent_errors: self.recent_errors(),
        }
    }

    /// Applies a settings patch (validated) and returns the new settings.
    pub async fn update_settings(&self, patch: SettingsPatch) -> Result<Settings, String> {
        self.inner.settings.update(patch).await
    }

    /// Reports whether the Go key is available (the value never leaves the store).
    pub async fn get_secret_status(&self) -> Result<SecretStatus, String> {
        Ok(SecretStatus {
            go: self.read_secret("go").await?.is_some(),
        })
    }

    /// The Settings › Secrets secret OpenCode Go uses (`OPENCODE_KEY` unless chosen).
    pub(super) async fn go_secret_name(&self) -> String {
        self.inner.settings.get().await.go_secret
    }

    /// Stores the API key for `provider` in the secret it uses.
    pub async fn set_secret(&self, provider: String, value: String) -> Result<(), String> {
        let value = zeroize::Zeroizing::new(value);
        known_provider(&provider)?;
        crate::secrets::validate_provider_key(&value)?;
        let name = self.go_secret_name().await;
        self.inner
            .secrets
            .set(&themis_core::plugins::keys::store_key(&name), &value)?;
        self.record_key_name(&name)
    }

    /// Deletes the API key for `provider` (missing keys are a no-op success).
    pub async fn clear_secret(&self, provider: String) -> Result<(), String> {
        known_provider(&provider)?;
        clear_secret_named(self.inner.secrets.as_ref(), &self.go_secret_name().await)
    }

    /// Checks the OpenCode Go key with Go (without generating anything): `works`,
    /// `rejected`, `unavailable`, or `missing` when the chosen secret has no value. Logs the
    /// outcome and timing as metadata, never the key.
    pub async fn check_go_key(&self) -> Result<serde_json::Value, String> {
        let secret = self.go_secret_name().await;
        let started = std::time::Instant::now();
        let state = match self.read_secret("go").await? {
            None => "missing".to_owned(),
            Some(key) => {
                let base_url = self
                    .inner
                    .go_base_url_override
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .clone()
                    .unwrap_or_else(|| themis_core::providers::GO_BASE_URL.to_owned());
                let checked = themis_core::providers::check_go_key(&base_url, &key).await;
                serde_json::to_value(checked)
                    .ok()
                    .and_then(|value| value.as_str().map(str::to_owned))
                    .unwrap_or_default()
            }
        };
        let _ = self.inner.diagnostics.append(
            "providers",
            "key_checked",
            if state == "works" { "info" } else { "warn" },
            serde_json::json!({"provider": "go", "result": state, "duration_ms": started.elapsed().as_millis() as u64}),
        );
        Ok(serde_json::json!({"state": state, "secret": secret}))
    }

    /// Looks up the Go API key.
    pub(super) async fn api_key_for(&self, provider: ProviderKind) -> Result<String, String> {
        let name = provider.as_str();
        self.read_secret(name).await?.ok_or_else(|| {
            format!("no API key stored for provider '{name}': run themis providers or add one in Settings, then retry")
        })
    }

    async fn read_secret(&self, provider: &'static str) -> Result<Option<String>, String> {
        known_provider(provider)?;
        let name = self.go_secret_name().await;
        self.read_named_secret(name).await
    }

    /// The Settings › Secrets secret `name` (saved, else the environment), read off the async
    /// workers with the same bounded wait as the OpenCode Go key.
    pub(super) async fn read_named_secret(&self, name: String) -> Result<Option<String>, String> {
        let secrets = self.inner.secrets.clone();
        // ponytail: native calls cannot be interrupted; a timed-out worker may finish later.
        tokio::time::timeout(
            std::time::Duration::from_secs(30),
            tokio::task::spawn_blocking(move || read_secret_named(secrets.as_ref(), &name)),
        )
        .await
        .map_err(|_| {
            "Credential lookup timed out; unlock the OS credential store and retry".to_owned()
        })?
        .map_err(|_| "Credential lookup worker failed".to_owned())?
    }
}

/// The secret `name`: its saved value, else (for `OPENCODE_KEY`) a key saved before keys
/// joined Settings › Secrets, else the environment variable of that name, as MCP servers
/// read it. The old entry is copied once, the first time nothing is saved under the new one,
/// and is then never read again (each read of an entry another build made can ask for
/// Keychain access); it is cleared with the secret (see [`clear_secret_named`]).
fn read_secret_named(
    secrets: &dyn crate::secrets::SecretStore,
    name: &str,
) -> Result<Option<String>, String> {
    let nonblank = |value: &String| !value.trim().is_empty();
    let entry = themis_core::plugins::keys::store_key(name);
    if let Some(saved) = secrets.stored(&entry)?.filter(nonblank) {
        return Ok(Some(saved));
    }
    if name == crate::secrets::GO_KEY_NAME {
        if let Some(legacy) = secrets
            .stored(crate::secrets::LEGACY_GO_ENTRY)?
            .filter(nonblank)
        {
            let _ = secrets.set(&entry, &legacy);
            return Ok(Some(legacy));
        }
    }
    Ok(secrets.environment(name))
}

/// Forgets the secret `name` (for `OPENCODE_KEY`, also a copy under its old entry).
pub(super) fn clear_secret_named(
    secrets: &dyn crate::secrets::SecretStore,
    name: &str,
) -> Result<(), String> {
    secrets.clear(&themis_core::plugins::keys::store_key(name))?;
    if name == crate::secrets::GO_KEY_NAME {
        secrets.clear(crate::secrets::LEGACY_GO_ENTRY)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct UnavailableReadStore;
    impl crate::secrets::SecretStore for UnavailableReadStore {
        fn get(&self, _: &str) -> Option<String> {
            None
        }
        fn try_get(&self, _: &str) -> Result<Option<String>, String> {
            Err("OS credential lookup unavailable".into())
        }
        fn has(&self, _: &str) -> bool {
            false
        }
        fn set(&self, _: &str, _: &str) -> Result<(), String> {
            unreachable!()
        }
        fn clear(&self, _: &str) -> Result<(), String> {
            unreachable!()
        }
    }

    #[tokio::test]
    async fn secret_status_and_admission_preserve_store_errors() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new_with_dirs(
            dir.path().join("settings.json"),
            dir.path().join("worktrees"),
            dir.path().join("threads.json"),
            Arc::new(UnavailableReadStore),
            dir.path().join("projects"),
            dir.path().join("trash"),
        );
        let (sink, _) = crate::sink::ChannelSink::channel();
        let result = crate::server::dispatch(
            &state,
            Arc::new(sink),
            "get_secret_status",
            serde_json::json!({}),
        )
        .await;
        assert_eq!(
            result.unwrap_err(),
            "OS credential lookup unavailable",
            "an unavailable store must not report a confirmed missing key"
        );
        assert_eq!(
            state.api_key_for(ProviderKind::Go).await.unwrap_err(),
            "OS credential lookup unavailable"
        );
    }

    struct SlowReadStore(std::time::Duration);
    impl crate::secrets::SecretStore for SlowReadStore {
        fn get(&self, entry: &str) -> Option<String> {
            std::thread::sleep(self.0);
            // A store with the key already in Settings › Keys (nothing to move).
            (entry != crate::secrets::LEGACY_GO_ENTRY).then(|| "synthetic-key".to_owned())
        }
        fn has(&self, provider: &str) -> bool {
            self.get(provider).is_some()
        }
        fn set(&self, _: &str, _: &str) -> Result<(), String> {
            unreachable!()
        }
        fn clear(&self, _: &str) -> Result<(), String> {
            unreachable!()
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn secret_status_timeout_is_an_error_not_a_missing_key() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new_with_dirs(
            dir.path().join("settings.json"),
            dir.path().join("worktrees"),
            dir.path().join("threads.json"),
            Arc::new(SlowReadStore(std::time::Duration::from_secs(31))),
            dir.path().join("projects"),
            dir.path().join("trash"),
        );
        let (sink, _) = crate::sink::ChannelSink::channel();
        let result = crate::server::dispatch(
            &state,
            Arc::new(sink),
            "get_secret_status",
            serde_json::json!({}),
        )
        .await;
        assert!(
            result.is_err(),
            "an unavailable read must not report a confirmed missing key: {result:?}"
        );
        assert!(result.unwrap_err().contains("Credential lookup timed out"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn credential_reads_do_not_block_other_backend_requests() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new_with_dirs(
            dir.path().join("settings.json"),
            dir.path().join("worktrees"),
            dir.path().join("threads.json"),
            Arc::new(SlowReadStore(std::time::Duration::from_millis(150))),
            dir.path().join("projects"),
            dir.path().join("trash"),
        );
        for status in [true, false] {
            let lookup = tokio::spawn({
                let state = state.clone();
                async move {
                    if status {
                        state.get_secret_status().await.unwrap().go
                    } else {
                        state.api_key_for(ProviderKind::Go).await.unwrap() == "synthetic-key"
                    }
                }
            });
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            assert!(
                !lookup.is_finished(),
                "credential lookup blocked the async worker"
            );
            assert_eq!(state.ping().await, themis_core::version());
            assert!(lookup.await.unwrap());
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn stop_during_credential_read_prevents_run_setup() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new_with_dirs(
            dir.path().join("settings.json"),
            dir.path().join("worktrees"),
            dir.path().join("threads.json"),
            Arc::new(SlowReadStore(std::time::Duration::from_millis(150))),
            dir.path().join("projects"),
            dir.path().join("trash"),
        );
        let project = dir.path().join("project");
        std::fs::create_dir(&project).unwrap();
        let thread = state
            .create_thread(
                project.to_string_lossy().into_owned(),
                ProviderKind::Go,
                None,
            )
            .await
            .unwrap();
        let (sink, mut events) = crate::sink::ChannelSink::channel();
        let send = tokio::spawn({
            let state = state.clone();
            let id = thread.id.clone();
            async move {
                state
                    .send_message(Arc::new(sink), id, "hello".to_owned())
                    .await
            }
        });
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while !state.get_thread(&thread.id).await.unwrap().running {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let wait = std::time::Duration::from_secs(2);
        let crate::sink::TestEvent::Thread(message) = tokio::time::timeout(wait, events.recv())
            .await
            .unwrap()
            .unwrap()
        else {
            panic!("expected the user message first");
        };
        assert!(matches!(message.event, ThreadEvent::UserMessage { .. }));
        let crate::sink::TestEvent::Thread(started) = tokio::time::timeout(wait, events.recv())
            .await
            .unwrap()
            .unwrap()
        else {
            panic!("expected admission before credential setup");
        };
        assert!(matches!(started.event, ThreadEvent::Started { .. }));
        assert!(
            !send.is_finished(),
            "credential admission must remain stoppable"
        );
        state.stop_thread(thread.id.clone()).await.unwrap();
        assert!(send.await.unwrap().unwrap_err().contains("Stopped by you"));
        let crate::sink::TestEvent::Thread(failed) = events.try_recv().unwrap() else {
            panic!("expected cancelled setup terminal");
        };
        assert_eq!(started.run_id, failed.run_id);
        assert!(matches!(failed.event, ThreadEvent::Failed { .. }));
        assert!(events.try_recv().is_err());
        assert!(!project.join(".themis/skill-catalogs").exists());
        assert!(!state.get_thread(&thread.id).await.unwrap().running);
    }
}
