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
        Ok(self.inner.browsers.chat(thread_id, &root))
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

    /// Take over (`user` true) or hand back.
    pub async fn browser_control(&self, thread_id: &str, user: bool) -> Result<(), String> {
        self.chat_browser(thread_id)
            .await?
            .set_user_control(user)
            .await;
        Ok(())
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
        let chat = self.chat_browser(thread_id).await?;
        // Typing an address is the person driving: the agent waits.
        chat.set_user_control(true).await;
        chat.open_for_user(url.as_str()).await
    }

    pub async fn browser_navigate(&self, thread_id: &str, action: &str) -> Result<(), String> {
        self.chat_browser(thread_id)
            .await?
            .navigate_for_user(action)
            .await
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
