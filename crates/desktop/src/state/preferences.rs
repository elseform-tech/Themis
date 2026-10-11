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
            go: self
                .read_secret("go")
                .await?
                .is_some_and(|key| !key.trim().is_empty()),
        })
    }

    /// Stores the API key for `provider`.
    pub async fn set_secret(&self, provider: String, value: String) -> Result<(), String> {
        let value = zeroize::Zeroizing::new(value);
        let key = secret_key(&provider)?;
        crate::secrets::validate_provider_key(&value)?;
        self.inner.secrets.set(key, &value)
    }

    /// Deletes the API key for `provider` (missing keys are a no-op success).
    pub async fn clear_secret(&self, provider: String) -> Result<(), String> {
        let key = secret_key(&provider)?;
        self.inner.secrets.clear(key)
    }

    /// Looks up the Go API key.
    pub(super) async fn api_key_for(&self, provider: ProviderKind) -> Result<String, String> {
        let name = provider.as_str();
        self.read_secret(name)
            .await?
            .filter(|key| !key.trim().is_empty())
            .ok_or_else(|| {
                format!("no API key stored for provider '{name}': run themis providers or add one in Settings, then retry")
            })
    }

    async fn read_secret(&self, provider: &'static str) -> Result<Option<String>, String> {
        let secrets = self.inner.secrets.clone();
        // ponytail: native calls cannot be interrupted; a timed-out worker may finish later.
        tokio::time::timeout(
            std::time::Duration::from_secs(30),
            tokio::task::spawn_blocking(move || secrets.get(provider)),
        )
        .await
        .map_err(|_| {
            "Credential lookup timed out; unlock the OS credential store and retry".to_owned()
        })?
        .map_err(|_| "Credential lookup worker failed".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct SlowReadStore(std::time::Duration);
    impl crate::secrets::SecretStore for SlowReadStore {
        fn get(&self, _: &str) -> Option<String> {
            std::thread::sleep(self.0);
            Some("synthetic-key".to_owned())
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
        let crate::sink::TestEvent::Thread(started) =
            tokio::time::timeout(std::time::Duration::from_secs(2), events.recv())
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
