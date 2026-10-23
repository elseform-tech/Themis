//! Downloads in the built-in browser. Chrome saves each one into a staging folder; a download
//! that starts during or just after an agent action is checked against Settings › Browser ›
//! Downloads for the page's site (Block cancels it, Ask asks), and an allowed or the person's
//! own download is moved to the downloads folder (the system Downloads folder by default).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::{cap, BrowserHub, ChatBrowser};
use crate::browser::cdp::Cdp;

/// A download that starts this long after the person's own input in the Browser tab is
/// theirs; every other download (the agent's, or a page's own) needs the Downloads permission.
const PERSON_WINDOW: Duration = Duration::from_secs(3);
/// How long an action waits for its downloads to finish before reporting.
const SETTLE: Duration = Duration::from_secs(30);
/// Downloads remembered per chat.
const KEPT: usize = 50;

/// Decides whether a download from a site may proceed (Allow, or Ask answered yes).
pub type DownloadGate = Arc<dyn Fn(&str) -> Result<(), String> + Send + Sync>;

/// What a run lets downloads do: where they go and the agent's permission.
#[derive(Clone)]
pub struct DownloadPolicy {
    pub directory: PathBuf,
    pub gate: DownloadGate,
}

#[derive(Debug, Clone, PartialEq)]
enum Status {
    Deciding,
    Allowed,
    Refused(String),
    Saved(PathBuf),
    Failed(String),
}

#[derive(Debug, Clone)]
pub(super) struct Download {
    guid: String,
    url: String,
    name: String,
    started: Instant,
    by_agent: bool,
    status: Status,
    /// Chrome finished writing it to the staging folder.
    staged: bool,
    bytes: u64,
}

/// The system Downloads folder: `XDG_DOWNLOAD_DIR` on Linux when set, else `~/Downloads`.
pub fn default_downloads_directory() -> PathBuf {
    if cfg!(target_os = "linux") {
        if let Some(dir) = std::env::var_os("XDG_DOWNLOAD_DIR").filter(|d| !d.is_empty()) {
            return PathBuf::from(dir);
        }
    }
    crate::sandbox::home_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("Downloads")
}

/// A file name that is safe to create and does not replace one already in `directory`.
fn free_name(directory: &Path, suggested: &str) -> PathBuf {
    let cleaned: String = suggested
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '_'
            } else {
                c
            }
        })
        .collect();
    let cleaned = cleaned.trim().trim_start_matches('.').to_owned();
    let name = if cleaned.is_empty() {
        "download".to_owned()
    } else {
        cap(&cleaned, 120)
    };
    let path = directory.join(&name);
    if !path.exists() {
        return path;
    }
    let (stem, extension) = match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => (stem.to_owned(), format!(".{extension}")),
        _ => (name.clone(), String::new()),
    };
    (1..10_000)
        .map(|n| directory.join(format!("{stem} ({n}){extension}")))
        .find(|candidate| !candidate.exists())
        .unwrap_or(path)
}

/// Moves a finished download from staging into `directory`.
fn place(staging: &Path, guid: &str, name: &str, directory: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(directory)
        .map_err(|e| format!("could not create {}: {e}", directory.display()))?;
    let target = free_name(directory, name);
    let source = staging.join(guid);
    if std::fs::rename(&source, &target).is_err() {
        // Across volumes a rename fails: copy, then remove.
        std::fs::copy(&source, &target).map_err(|e| format!("could not save the download: {e}"))?;
        let _ = std::fs::remove_file(&source);
    }
    Ok(target)
}

impl BrowserHub {
    /// The chat whose tab is (or contains) `frame`.
    fn chat_of_frame(&self, frame: &str) -> Option<Arc<ChatBrowser>> {
        let chats: Vec<Arc<ChatBrowser>> = self
            .chats
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .cloned()
            .collect();
        chats.into_iter().find(|chat| {
            chat.targets
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .contains(frame)
        })
    }

    /// Lets a browser save downloads into `staging` and routes their events to the chats.
    pub(super) async fn watch_downloads(self: &Arc<Self>, cdp: &Arc<Cdp>, staging: PathBuf) {
        let _ = std::fs::create_dir_all(&staging);
        let _ = cdp
            .call(
                None,
                "Browser.setDownloadBehavior",
                json!({"behavior": "allowAndName", "downloadPath": staging.to_string_lossy(), "eventsEnabled": true}),
            )
            .await;
        let mut events = cdp.subscribe();
        let hub = Arc::downgrade(self);
        let cdp = Arc::clone(cdp);
        tokio::spawn(async move {
            let mut owners: std::collections::HashMap<String, std::sync::Weak<ChatBrowser>> =
                Default::default();
            loop {
                let event = match events.recv().await {
                    Ok(event) => event,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => break,
                };
                let params = &event.params;
                match event.method.as_str() {
                    "Browser.downloadWillBegin" => {
                        let Some(hub) = hub.upgrade() else { break };
                        let guid = params["guid"].as_str().unwrap_or_default().to_owned();
                        let frame = params["frameId"].as_str().unwrap_or_default();
                        let Some(chat) = hub.chat_of_frame(frame) else {
                            // Not a chat's tab: nothing may download there.
                            cdp.send(None, "Browser.cancelDownload", json!({"guid": guid}));
                            continue;
                        };
                        owners.insert(guid.clone(), Arc::downgrade(&chat));
                        chat.download_began(
                            Arc::clone(&cdp),
                            staging.clone(),
                            guid,
                            params["url"].as_str().unwrap_or_default(),
                            params["suggestedFilename"].as_str().unwrap_or_default(),
                        )
                        .await;
                    }
                    "Browser.downloadProgress" => {
                        let guid = params["guid"].as_str().unwrap_or_default();
                        let Some(chat) = owners.get(guid).and_then(std::sync::Weak::upgrade) else {
                            continue;
                        };
                        match params["state"].as_str() {
                            Some("completed") => {
                                chat.download_staged(
                                    &staging,
                                    guid,
                                    params["receivedBytes"].as_f64().unwrap_or(0.0) as u64,
                                );
                                owners.remove(guid);
                            }
                            Some("canceled") => {
                                chat.download_failed(guid, "canceled");
                                owners.remove(guid);
                            }
                            _ => {}
                        }
                    }
                    _ => {}
                }
            }
        });
    }
}

impl ChatBrowser {
    /// The run's download settings (folder and the agent's permission).
    pub fn set_download_policy(&self, policy: Option<DownloadPolicy>) {
        *self
            .download_policy
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = policy;
    }

    /// Marks the person's own input: a download starting soon after is theirs.
    pub(crate) fn person_acted(&self) {
        *self.person_until.lock().unwrap_or_else(|e| e.into_inner()) =
            Some(Instant::now() + PERSON_WINDOW);
    }

    fn directory(&self) -> PathBuf {
        self.download_policy
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map(|policy| policy.directory.clone())
            .unwrap_or_else(default_downloads_directory)
    }

    async fn download_began(
        self: &Arc<Self>,
        cdp: Arc<Cdp>,
        staging: PathBuf,
        guid: String,
        url: &str,
        name: &str,
    ) {
        let by_person = self
            .person_until
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some_and(|until| Instant::now() <= until);
        let gate = self
            .download_policy
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map(|policy| Arc::clone(&policy.gate));
        // Fails closed: anything but the person's own download needs the permission, and
        // with no run's policy to decide it, it is cancelled.
        let status = match (by_person, &gate) {
            (true, _) => Status::Allowed,
            (false, Some(_)) => Status::Deciding,
            (false, None) => Status::Refused("no run allows downloads now".into()),
        };
        {
            let mut downloads = self.downloads.lock().unwrap_or_else(|e| e.into_inner());
            downloads.push(Download {
                guid: guid.clone(),
                url: cap(url, 500),
                name: name.to_owned(),
                started: Instant::now(),
                by_agent: !by_person,
                status: status.clone(),
                staged: false,
                bytes: 0,
            });
            let excess = downloads.len().saturating_sub(KEPT);
            downloads.drain(..excess);
        }
        if matches!(status, Status::Refused(_)) {
            cdp.send(None, "Browser.cancelDownload", json!({"guid": guid}));
            let _ = std::fs::remove_file(staging.join(&guid));
            return;
        }
        let (Some(gate), false) = (gate, by_person) else {
            return;
        };
        let site = self.current_site().await.unwrap_or_default();
        let chat = Arc::clone(self);
        // The gate may wait for the person's answer: keep it off the event loop.
        tokio::task::spawn_blocking(move || {
            let decision = gate(&site);
            if let Err(reason) = &decision {
                cdp.send(None, "Browser.cancelDownload", json!({"guid": guid}));
                let _ = std::fs::remove_file(staging.join(&guid));
                chat.set_status(&guid, Status::Refused(reason.clone()));
            } else {
                chat.set_status(&guid, Status::Allowed);
                chat.finish_if_ready(&staging, &guid);
            }
        });
    }

    fn set_status(&self, guid: &str, status: Status) {
        let mut downloads = self.downloads.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(download) = downloads.iter_mut().find(|d| d.guid == guid) {
            download.status = status;
        }
    }

    fn download_staged(&self, staging: &Path, guid: &str, bytes: u64) {
        {
            let mut downloads = self.downloads.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(download) = downloads.iter_mut().find(|d| d.guid == guid) {
                download.staged = true;
                download.bytes = bytes;
            }
        }
        self.finish_if_ready(staging, guid);
    }

    fn download_failed(&self, guid: &str, reason: &str) {
        let mut downloads = self.downloads.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(download) = downloads.iter_mut().find(|d| d.guid == guid) {
            if !matches!(download.status, Status::Refused(_)) {
                download.status = Status::Failed(reason.to_owned());
            }
        }
    }

    /// Moves an allowed, finished download to the downloads folder.
    fn finish_if_ready(&self, staging: &Path, guid: &str) {
        let ready = {
            let downloads = self.downloads.lock().unwrap_or_else(|e| e.into_inner());
            downloads
                .iter()
                .find(|d| d.guid == guid)
                .filter(|d| d.staged && d.status == Status::Allowed)
                .map(|d| d.name.clone())
        };
        let Some(name) = ready else { return };
        let status = match place(staging, guid, &name, &self.directory()) {
            Ok(path) => Status::Saved(path),
            Err(error) => Status::Failed(error),
        };
        self.set_status(guid, status);
    }

    /// Downloads the agent started since `since`, once each has finished or was refused
    /// (waiting up to [`SETTLE`]), for the action's report.
    pub(crate) async fn agent_downloads_since(&self, since: Instant) -> Vec<Value> {
        let deadline = Instant::now() + SETTLE;
        loop {
            let (pending, done): (Vec<Download>, Vec<Download>) = self
                .downloads
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .filter(|d| d.by_agent && d.started >= since)
                .cloned()
                .partition(|d| matches!(d.status, Status::Deciding | Status::Allowed));
            if pending.is_empty() || Instant::now() >= deadline {
                return done
                    .iter()
                    .chain(pending.iter())
                    .map(|d| match &d.status {
                        Status::Saved(path) => json!({"file": d.name, "saved_to": path, "bytes": d.bytes, "from": d.url}),
                        Status::Refused(reason) => json!({"file": d.name, "refused": cap(reason, 300), "from": d.url}),
                        Status::Failed(reason) => json!({"file": d.name, "failed": reason, "from": d.url}),
                        Status::Deciding | Status::Allowed => json!({"file": d.name, "still_downloading": true, "from": d.url}),
                    })
                    .collect();
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }
}

impl ChatBrowser {
    /// Test support: runs `expression` in the current tab as page script (no person input).
    #[doc(hidden)]
    pub async fn evaluate_for_test(&self, expression: &str) -> Result<(), String> {
        self.dev_console(Some(expression), 0, false)
            .await
            .map(|_| ())
    }

    /// Test support: the downloads started since `since` that were not the person's.
    #[doc(hidden)]
    pub async fn agent_downloads_for_test(&self, since: Instant) -> Vec<Value> {
        self.agent_downloads_since(since).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn download_names_are_safe_and_never_replace_a_file() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            free_name(dir.path(), "report.pdf"),
            dir.path().join("report.pdf")
        );
        std::fs::write(dir.path().join("report.pdf"), b"x").unwrap();
        assert_eq!(
            free_name(dir.path(), "report.pdf"),
            dir.path().join("report (1).pdf")
        );
        assert_eq!(
            free_name(dir.path(), "../../etc/passwd"),
            dir.path().join("_.._etc_passwd")
        );
        assert_eq!(free_name(dir.path(), ""), dir.path().join("download"));
        assert_eq!(free_name(dir.path(), ".hidden"), dir.path().join("hidden"));
    }
}
