//! The Workspace Browser tab's side of the built-in browser: the same chat
//! browser the agent's tools drive, plus the person's address bar, input,
//! take over and DevTools. None of it touches project files.

use std::sync::Arc;

use serde_json::{json, Value};
use themis_core::browser::{BrowserState, BrowserUpdate, ChatBrowser, UserInput};

use super::AppState;

impl AppState {
    /// The chat's browser (created, not started, on first use).
    pub(super) async fn chat_browser(&self, thread_id: &str) -> Result<Arc<ChatBrowser>, String> {
        let root = self
            .inner
            .threads
            .read()
            .await
            .get(thread_id)
            .map(|record| record.project_root.clone())
            .ok_or_else(|| format!("unknown thread '{thread_id}'"))?;
        Ok(self.browser_of(thread_id, &root))
    }

    /// The chat's browser for `root`; the first time, its controller and
    /// recording changes start reaching every window as `browser_changed`.
    pub(super) fn browser_of(&self, thread_id: &str, root: &std::path::Path) -> Arc<ChatBrowser> {
        let existed = self.inner.browsers.existing(thread_id).is_some();
        let chat = self.inner.browsers.chat(thread_id, root);
        if !existed {
            let mut updates = chat.subscribe();
            let state = self.clone();
            let thread_id = thread_id.to_owned();
            tokio::spawn(async move {
                let mut last = None;
                loop {
                    let update = match updates.recv().await {
                        Ok(update) => update,
                        // Frames crowded out a state change: catch up from the current state.
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                            match state.browser_state_update(&thread_id).await {
                                Some(update) => update,
                                None => continue,
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    };
                    let BrowserUpdate::State(browser) = update else {
                        continue;
                    };
                    let key = (
                        browser.controller.clone(),
                        browser.recording.as_ref().map(|r| (r.by, r.max_seconds)),
                    );
                    if last.as_ref() == Some(&key) {
                        continue;
                    }
                    last = Some(key);
                    if let Some(sink) = state.scheduler_sink() {
                        sink.emit_thread_event(&crate::types::ThreadEventEnvelope {
                            thread_id: thread_id.clone(),
                            run_id: String::new(),
                            event: crate::types::ThreadEvent::BrowserChanged {
                                controller: browser.controller,
                                recording: browser.recording,
                            },
                        });
                    }
                }
            });
        }
        chat
    }

    /// The chat's current browser state as an update, if it has a browser.
    pub async fn browser_state_update(&self, thread_id: &str) -> Option<BrowserUpdate> {
        let chat = self.inner.browsers.existing(thread_id)?;
        Some(BrowserUpdate::State(chat.state().await))
    }

    /// The app's browser hub (for run tools).
    pub(super) fn browser_hub(&self) -> Arc<themis_core::browser::BrowserHub> {
        Arc::clone(&self.inner.browsers)
    }

    /// What the Browser tab shows, plus whether a browser is available at all.
    pub async fn browser_state(&self, thread_id: &str) -> Result<Value, String> {
        let state = match self.inner.browsers.existing(thread_id) {
            Some(chat) => chat.state().await,
            None => {
                // Ensure the thread exists before answering for it.
                self.chat_browser(thread_id).await?;
                BrowserState::default()
            }
        };
        let mut value = serde_json::to_value(state).map_err(|e| e.to_string())?;
        value["available"] = json!(self.inner.browsers.executable().is_some());
        Ok(value)
    }

    /// Starts streaming the chat's current tab at the view's size.
    pub async fn browser_screencast(
        &self,
        thread_id: &str,
        width: u32,
        height: u32,
        scale: f64,
    ) -> Result<(), String> {
        self.chat_browser(thread_id)
            .await?
            .start_screencast(width, height, scale)
            .await
    }

    pub async fn browser_screencast_stop(&self, thread_id: &str) -> Result<(), String> {
        if let Some(chat) = self.inner.browsers.existing(thread_id) {
            chat.stop_screencast().await;
        }
        Ok(())
    }

    /// The person's input on the view (takes over the browser).
    pub async fn browser_input(&self, thread_id: &str, input: Value) -> Result<(), String> {
        let input: UserInput =
            serde_json::from_value(input).map_err(|e| format!("invalid browser input: {e}"))?;
        self.chat_browser(thread_id).await?.user_input(&input).await
    }

    /// The person typed an address. Bare hosts get `https://`.
    pub async fn browser_open_url(&self, thread_id: &str, address: &str) -> Result<(), String> {
        let address = address.trim();
        let url = if address.contains("://") {
            address.to_owned()
        } else if address.starts_with("localhost") || address.starts_with("127.0.0.1") {
            format!("http://{address}")
        } else {
            format!("https://{address}")
        };
        let url = themis_core::tools::web::checked_url(&url)?;
        self.chat_browser(thread_id)
            .await?
            .open_for_user(url.as_str())
            .await
    }

    pub async fn browser_navigate(&self, thread_id: &str, action: &str) -> Result<(), String> {
        self.chat_browser(thread_id)
            .await?
            .navigate_for_user(action)
            .await
    }

    /// The person's Capture: a screenshot of the visible page saved with the
    /// project's attachments, ready to add to the message being written.
    pub async fn browser_capture(&self, thread_id: &str) -> Result<Value, String> {
        use themis_core::browser::media::{media_name, MediaSink};
        let chat = self.chat_browser(thread_id).await?;
        let (png, _, _) = chat.screenshot(None, false).await?;
        let path = self
            .project_media(thread_id)
            .await?
            .save(&media_name("screenshot", "png"), &png)?;
        Ok(attachment_value(&path, png.len()))
    }

    /// The person's Record: `start`, or `stop` to save the video with the
    /// project's attachments. The person can stop the agent's recording too.
    pub async fn browser_record(&self, thread_id: &str, action: &str) -> Result<Value, String> {
        use themis_core::browser::media::{media_name, MediaSink, Recorder, RECORD_MAX};
        let chat = self.chat_browser(thread_id).await?;
        match action {
            "start" => {
                chat.start_recording(Recorder::User, RECORD_MAX).await?;
                Ok(Value::Null)
            }
            "stop" => {
                let (video, length) = chat.stop_recording(Recorder::User).await?;
                let path = self
                    .project_media(thread_id)
                    .await?
                    .save(&media_name("recording", "mp4"), &video)?;
                let mut value = attachment_value(&path, video.len());
                value["seconds"] = json!(length.as_secs());
                Ok(value)
            }
            _ => Err("action must be start or stop".into()),
        }
    }

    /// Saves a recording the agent left running when its run ended.
    pub(super) async fn stop_agent_recording(&self, thread_id: &str) {
        use themis_core::browser::media::{media_name, MediaSink, Recorder};
        let Some(chat) = self.inner.browsers.existing(thread_id) else {
            return;
        };
        if chat.recorder() != Some(Recorder::Agent) {
            return;
        }
        if let Ok((video, _)) = chat.stop_recording(Recorder::Agent).await {
            if let Ok(media) = self.project_media(thread_id).await {
                let _ = media.save(&media_name("recording", "mp4"), &video);
            }
        }
    }

    async fn project_media(&self, thread_id: &str) -> Result<ProjectMedia, String> {
        let root = self
            .inner
            .threads
            .read()
            .await
            .get(thread_id)
            .map(|record| record.project_root.clone())
            .ok_or_else(|| format!("unknown thread '{thread_id}'"))?;
        Ok(ProjectMedia { root })
    }

    /// The person switched to or closed a tab (1-based).
    pub async fn browser_tab(
        &self,
        thread_id: &str,
        action: &str,
        tab: usize,
    ) -> Result<(), String> {
        if !matches!(action, "switch" | "close") {
            return Err("action must be switch or close".into());
        }
        self.chat_browser(thread_id)
            .await?
            .tabs(action, Some(tab))
            .await
            .map(|_| ())
    }

    /// The DevTools page for the current tab, if one is open.
    pub async fn browser_devtools_url(&self, thread_id: &str) -> Result<Option<String>, String> {
        Ok(match self.inner.browsers.existing(thread_id) {
            Some(chat) => chat.devtools_url().await,
            None => None,
        })
    }

    /// The chat's browser updates, for the Browser tab's stream.
    pub async fn browser_updates(
        &self,
        thread_id: &str,
    ) -> Result<tokio::sync::broadcast::Receiver<BrowserUpdate>, String> {
        Ok(self.chat_browser(thread_id).await?.subscribe())
    }

    /// Closes the browsers of discarded chats.
    pub(super) async fn forget_browsers(&self, thread_ids: &[String]) {
        for thread_id in thread_ids {
            self.inner.browsers.close_chat(thread_id).await;
        }
    }
}

/// Saves screenshots and recordings with the project's attachments
/// (`.themis/attachments/<id>/<name>`), where replies and the composer can show them.
pub(super) struct ProjectMedia {
    pub(super) root: std::path::PathBuf,
}

impl themis_core::browser::media::MediaSink for ProjectMedia {
    fn save(&self, name: &str, bytes: &[u8]) -> Result<String, String> {
        let directory =
            super::attachments::project_storage_directory(&self.root, &["attachments"])?;
        let folder = directory.join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir(&folder).map_err(|e| e.to_string())?;
        let path = folder.join(name);
        std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
        Ok(path.to_string_lossy().into_owned())
    }
}

/// A saved file as the composer's attachments list holds it.
fn attachment_value(path: &str, size: usize) -> Value {
    let name = std::path::Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    json!({"path": path, "name": name, "size": size})
}

/// One update as the Browser tab receives it.
pub fn update_payload(update: &BrowserUpdate) -> Value {
    match update {
        BrowserUpdate::State(state) => json!({"kind": "state", "state": state}),
        BrowserUpdate::Frame {
            data,
            width,
            height,
        } => json!({"kind": "frame", "data": data, "width": width, "height": height}),
    }
}
