//! Opening files and links in the person's own apps.

use super::*;

/// Opens `target` as plain text: `open -t` on macOS (the default text
/// editor, whatever app `.jsonc` is bound to), `xdg-open` on Linux and
/// Notepad on Windows.
pub(crate) fn spawn_text_editor(target: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = std::process::Command::new("open");
        command.arg("-t");
        command
    };
    #[cfg(target_os = "linux")]
    let mut command = std::process::Command::new("xdg-open");
    #[cfg(target_os = "windows")]
    let mut command = std::process::Command::new("notepad.exe");
    command
        .arg(target)
        .spawn()
        .map(|_| ())
        .map_err(|err| format!("failed to open '{}': {err}", target.display()))
}

/// Opens an http(s) URL in the person's default browser (no shell parses it).
/// What opens a web address in the person's browser.
pub type UrlOpener = Arc<dyn Fn(&str) -> Result<(), String> + Send + Sync>;

impl AppState {
    /// Sends sign-in pages to `opener` instead of the browser: a test's fake person.
    pub fn use_url_opener(&self, opener: UrlOpener) {
        *self
            .inner
            .url_opener
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(opener);
    }

    pub(crate) fn open_url(&self, url: &str) -> Result<(), String> {
        let opener = self
            .inner
            .url_opener
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        match opener {
            Some(opener) => opener(url),
            None => spawn_opener_url(url),
        }
    }
}

pub(crate) fn spawn_opener_url(url: &str) -> Result<(), String> {
    let web = url.starts_with("https://") || url.starts_with("http://");
    if !web || url.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("Only web addresses can be opened".into());
    }
    #[cfg(target_os = "macos")]
    let mut command = std::process::Command::new("open");
    #[cfg(target_os = "linux")]
    let mut command = std::process::Command::new("xdg-open");
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = std::process::Command::new("rundll32");
        command.arg("url.dll,FileProtocolHandler");
        command
    };
    command
        .arg(url)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("failed to open the browser: {e}"))
}
