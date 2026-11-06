//! The app-wide hub: one shared Chromium per project profile.

use super::*;

/// The built-in engine downloaded into app-data, if any.
pub fn downloaded_browser(root: &Path) -> Option<PathBuf> {
    crate::browser::engine::installed(root)
}

impl BrowserHub {
    pub fn new(config: BrowserConfig) -> Arc<Self> {
        Arc::new(Self {
            config: Mutex::new(config),
            shared: tokio::sync::Mutex::default(),
            chats: Mutex::default(),
            download_engine: AtomicBool::new(false),
            engine_install: tokio::sync::Mutex::new(()),
        })
    }

    /// Lets the browser download its built-in engine on first use (the app does; tests
    /// keep using an installed browser).
    pub fn allow_engine_download(&self) {
        self.download_engine.store(true, Ordering::Relaxed);
    }

    /// Whether a browser can be started: one is found, or the engine can be downloaded.
    pub fn available(&self) -> bool {
        self.executable().is_some() || self.built_in()
    }

    /// Whether the automatic choice is the built-in engine (installed or downloadable).
    pub fn built_in(&self) -> bool {
        downloaded_browser(&self.root()).is_some()
            || (self.download_engine.load(Ordering::Relaxed)
                && crate::browser::engine::platform().is_some())
    }

    /// Installs the built-in engine unless it is there, one install at a time.
    pub(super) async fn install_engine(&self, root: &Path) -> Result<PathBuf, String> {
        let _one = self.engine_install.lock().await;
        if let Some(installed) = downloaded_browser(root) {
            return Ok(installed);
        }
        crate::browser::engine::install(root)
            .await
            .map_err(|error| format!("could not download the browser engine: {error:#}"))
    }

    /// The engine to use when the automatic choice applies: the downloaded one, else
    /// (with downloads allowed) it is downloaded now, so the person's own Chrome is never
    /// started and never shows in the Dock. If the download fails, an installed browser is
    /// used instead; with none, the download error is reported.
    pub(super) async fn engine_for(
        &self,
        config: &BrowserConfig,
    ) -> Result<Option<PathBuf>, String> {
        if let Some(installed) = downloaded_browser(&config.root) {
            return Ok(Some(installed));
        }
        if config.executable.is_some() || !self.download_engine.load(Ordering::Relaxed) {
            return Ok(None);
        }
        match self.install_engine(&config.root).await {
            Ok(engine) => Ok(Some(engine)),
            Err(_) if find_browser(None, None).is_some() => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Applies a run's captured `browser.executable` for browsers started later.
    pub fn set_executable(&self, executable: Option<PathBuf>) {
        self.config
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .executable = executable;
    }

    /// The executable that would be used, or `None` when no browser is found.
    pub fn executable(&self) -> Option<PathBuf> {
        let config = self
            .config
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        find_browser(
            config.executable.as_deref(),
            downloaded_browser(&config.root).as_deref(),
        )
    }

    /// The app-data folder browsers live in.
    pub fn root(&self) -> PathBuf {
        self.config
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .root
            .clone()
    }

    /// The chat's browser, created (not started) on first use.
    pub fn chat(self: &Arc<Self>, chat_id: &str, project: &Path) -> Arc<ChatBrowser> {
        let mut chats = self.chats.lock().unwrap_or_else(|e| e.into_inner());
        Arc::clone(chats.entry(chat_id.to_owned()).or_insert_with(|| {
            let (updates, _) = broadcast::channel(64);
            Arc::new(ChatBrowser {
                hub: Arc::downgrade(self),
                profile_key: profile_key(project),
                inner: tokio::sync::Mutex::new(ChatInner::default()),
                activity: Mutex::new(None),
                updates,
                screencast: Mutex::new(None),
                frozen: AtomicBool::new(false),
                pointer: Mutex::new(None),
                page_locked: AtomicBool::new(false),
                held: Mutex::new(None),
                ui_stream: AtomicBool::new(false),
                frame_target: Mutex::new(None),
                recording: Mutex::new(None),
                viewport: Mutex::new(Viewport::default()),
                muted: AtomicBool::new(true),
                downloads: Mutex::new(Vec::new()),
                person_until: Mutex::new(None),
                download_policy: Mutex::new(None),
                targets: Mutex::new(HashSet::new()),
                view: Mutex::default(),
                stream_start: tokio::sync::Mutex::new(()),
                reopen: Mutex::new(None),
                last_used: Mutex::new(Instant::now()),
            })
        }))
    }

    /// The chat's browser if it has one already.
    pub fn existing(&self, chat_id: &str) -> Option<Arc<ChatBrowser>> {
        self.chats
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(chat_id)
            .cloned()
    }

    /// Closes a chat's page (chat deleted); the shared browser exits when no
    /// chat uses it.
    pub async fn close_chat(&self, chat_id: &str) {
        let chat = self
            .chats
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(chat_id);
        if let Some(chat) = chat {
            chat.close_all().await;
        }
        self.release_unused().await;
    }

    /// The folder of `project`'s browser profile (history, cookies, site data, cache).
    pub fn profile_dir(&self, project: &Path) -> PathBuf {
        self.root().join("profiles").join(profile_key(project))
    }

    /// Closes `project`'s browser (every chat's page on it) and waits for Chrome to let go of
    /// its profile, so its files can be changed. Chats reopen it when they next browse.
    pub async fn close_project(&self, project: &Path) {
        let key = profile_key(project);
        let chats: Vec<Arc<ChatBrowser>> = self
            .chats
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .filter(|chat| chat.profile_key == key)
            .cloned()
            .collect();
        for chat in chats {
            chat.close_all().await;
        }
        let browser = self.shared.lock().await.remove(&key);
        if let Some(browser) = browser {
            let _ = browser.cdp.call(None, "Browser.close", json!({})).await;
        }
        let lock = self
            .root()
            .join("profiles")
            .join(&key)
            .join("SingletonLock");
        for _ in 0..50 {
            if std::fs::symlink_metadata(&lock).is_err() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// Keeps at most [`OPEN_PAGES`] chats' pages open: closes the least recently used idle
    /// ones until `opening` fits. A page in use stays open, so the limit can be passed then.
    pub(super) async fn make_room(&self, opening: &Arc<ChatBrowser>) {
        let mut others: Vec<Arc<ChatBrowser>> = self
            .chats
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .filter(|chat| !Arc::ptr_eq(chat, opening))
            .filter(|chat| {
                chat.view
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .tab
                    .is_some()
            })
            .cloned()
            .collect();
        others.sort_by_key(|chat| *chat.last_used.lock().unwrap_or_else(|e| e.into_inner()));
        let mut open = others.len();
        for chat in others {
            if open < OPEN_PAGES {
                break;
            }
            if chat.idle_page() && chat.close_for_room().await {
                open -= 1;
            }
        }
    }

    /// Ends the browser when every chat has left it idle for `idle_for`: nobody watching a
    /// Browser tab, no agent action, no recording. Each open page is closed with its address
    /// remembered (the next action reopens it there), every handle dropped, so Chromium and its
    /// helpers exit. Returns whether it did.
    pub async fn exit_when_idle(&self, idle_for: Duration) -> bool {
        let chats: Vec<Arc<ChatBrowser>> = self
            .chats
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .cloned()
            .collect();
        if self.shared.lock().await.is_empty() {
            return false;
        }
        // A chat whose page was already closed to make room is as quiet as an idle one: with
        // more chats than open pages, waiting for it to have a page would keep the browser forever.
        let quiet = chats.iter().all(|chat| {
            (chat.idle_page() || chat.page_closed_and_quiet())
                && chat
                    .last_used
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .elapsed()
                    >= idle_for
        });
        if !quiet {
            return false;
        }
        for chat in &chats {
            if !chat.release_browser().await {
                return false;
            }
        }
        self.shared.lock().await.clear();
        crate::diagnostics::emit(
            "browser",
            "exited_idle",
            "info",
            json!({"chats": chats.len(), "idle_seconds": idle_for.as_secs()}),
        );
        true
    }

    pub(super) async fn release_unused(&self) {
        let used: HashSet<String> = {
            let chats = self.chats.lock().unwrap_or_else(|e| e.into_inner());
            chats
                .values()
                .map(|chat| chat.profile_key.clone())
                .collect()
        };
        self.shared.lock().await.retain(|key, _| used.contains(key));
    }

    pub(super) async fn shared_for(
        self: &Arc<Self>,
        key: &str,
    ) -> Result<Arc<SharedBrowser>, String> {
        let mut shared = self.shared.lock().await;
        if let Some(existing) = shared.get(key) {
            if !existing.cdp.is_closed() && existing.process.lock().await.is_running() {
                return Ok(Arc::clone(existing));
            }
            shared.remove(key);
        }
        let config = self
            .config
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let engine = self.engine_for(&config).await?;
        let executable =
            find_browser(config.executable.as_deref(), engine.as_deref()).ok_or_else(|| {
                match &config.executable {
                    Some(path) => format!("browser.executable {} does not exist", path.display()),
                    None => NO_BROWSER.to_owned(),
                }
            })?;
        let profile = config.root.join("profiles").join(key);
        let process = BrowserProcess::launch(&executable, &profile, config.headless)
            .await
            .map_err(|error| format!("{error:#}"))?;
        // The page that starts with the browser is not any chat's tab.
        let cdp = Arc::clone(&process.cdp);
        self.watch_downloads(&cdp, config.root.join("downloads-staging").join(key))
            .await;
        // Pages never get the camera, microphone, location or notifications.
        for name in ["camera", "microphone", "geolocation", "notifications"] {
            let _ = cdp
                .call(
                    None,
                    "Browser.setPermission",
                    json!({"permission": {"name": name}, "setting": "denied"}),
                )
                .await;
        }
        if let Ok(targets) = cdp.call(None, "Target.getTargets", json!({})).await {
            for target in targets["targetInfos"].as_array().into_iter().flatten() {
                if target["type"] == "page" {
                    let _ = cdp
                        .call(
                            None,
                            "Target.closeTarget",
                            json!({"targetId": target["targetId"]}),
                        )
                        .await;
                }
            }
        }
        let browser = Arc::new(SharedBrowser {
            http_address: process.http_address.clone(),
            cdp,
            process: tokio::sync::Mutex::new(process),
        });
        shared.insert(key.to_owned(), Arc::clone(&browser));
        Ok(browser)
    }
}

/// Error text when no Chromium browser is installed.
pub const NO_BROWSER: &str = "No Chromium browser found. Install Chrome, Chromium or Edge, or set browser.executable in runtime.jsonc.";

pub(super) fn profile_key(project: &Path) -> String {
    use sha2::{Digest, Sha256};
    let canonical = project
        .canonicalize()
        .unwrap_or_else(|_| project.to_path_buf());
    let digest = Sha256::digest(canonical.to_string_lossy().as_bytes());
    digest.iter().take(8).map(|b| format!("{b:02x}")).collect()
}
