//! Browser sessions: one Chromium per project profile, shared by that
//! project's chats; each chat owns its own tabs.
//!
//! The agent's tools and the person's Workspace view drive the same tabs. When
//! the person takes over, agent actions wait until they hand back. Each tab
//! keeps what changed since the agent last looked (console errors, failed
//! requests, dialogs) and whether the page is still loading.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{json, Value};
use tokio::sync::broadcast;

use super::cdp::{Cdp, CdpEvent};
use super::launch::{find_browser, BrowserProcess, DEFAULT_VIEWPORT};
use super::media::{Recorder, Recording};
use super::snapshot::{parse_ax_nodes, render, Outline, RefMap, RenderOptions};

/// Tabs one chat may keep open.
pub const MAX_TABS: usize = 8;
/// Longest wait for a page to finish loading after a navigation.
pub const NAVIGATION_WAIT: Duration = Duration::from_secs(30);
/// Longest wait for a page to settle after other actions.
pub const SETTLE_WAIT: Duration = Duration::from_secs(10);
/// Network quiet this long counts as settled.
const QUIET: Duration = Duration::from_millis(500);
/// Requests running longer than this (long polls, streams) do not block settling.
const LONG_REQUEST: Duration = Duration::from_secs(3);
/// A loaded page whose network never goes quiet (analytics, polling, live updates)
/// counts as settled this long after loading, so the agent is not held up on busy sites.
const BUSY_AFTER_LOAD: Duration = Duration::from_secs(2);
/// Notes kept per tab between agent looks.
const MAX_NOTES: usize = 20;
const NOTE_CHARS: usize = 300;

/// A change the UI should show.
#[derive(Debug, Clone)]
pub enum BrowserUpdate {
    /// The tab list, address, title, controller or agent activity changed.
    State(BrowserState),
    /// A screencast frame of the current tab (JPEG, base64).
    Frame {
        data: String,
        width: u32,
        height: u32,
    },
}

/// What the Workspace Browser tab shows.
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct BrowserState {
    pub open: bool,
    pub tabs: Vec<TabInfo>,
    pub current: usize,
    pub url: String,
    pub title: String,
    pub can_go_back: bool,
    pub can_go_forward: bool,
    /// `agent` while an agent action runs, otherwise `idle`.
    pub controller: String,
    /// What the agent is doing now, e.g. `clicking "Create account"`.
    pub activity: Option<String>,
    pub console_errors: usize,
    pub loading: bool,
    /// The Workspace speaker is off.
    pub muted: bool,
    /// A recording in progress.
    pub recording: Option<RecordingInfo>,
}

/// A recording in progress, for the Browser tab and the thread header.
#[derive(Debug, Clone, Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct RecordingInfo {
    pub by: Recorder,
    pub seconds: u64,
    pub max_seconds: u64,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct TabInfo {
    pub title: String,
    pub url: String,
}

/// Live state of one tab, kept current by its event watcher.
#[derive(Debug, Default)]
struct TabState {
    url: String,
    title: String,
    loading: bool,
    /// Request id -> start time.
    inflight: HashMap<String, Instant>,
    last_network: Option<Instant>,
    notes: Vec<String>,
    console_errors: usize,
    /// Bumped when the main frame commits a new document (refs reset).
    document: u64,
    /// Console messages at every level and uncaught exceptions, newest last (debugging tools).
    console: VecDeque<devtools::ConsoleEntry>,
    /// Requests the page made, newest last (debugging tools).
    requests: VecDeque<devtools::RequestEntry>,
    /// Numbers console messages and requests, so a tool can read only newer ones.
    log_seq: u64,
    /// The debugger's pause (`Debugger.paused` params) while the page is stopped.
    paused: Option<Value>,
    /// Counts pauses, so a stale auto-resume does not resume a later one.
    pauses: u64,
    /// Counts screencast frames, so a restarted stream can tell whether it is drawing.
    frames: u64,
}

struct Tab {
    target_id: String,
    session_id: String,
    state: Arc<Mutex<TabState>>,
    refs: RefMap,
    /// Accessible names of nodes in the last snapshot, for activity labels.
    names: HashMap<i64, String>,
    seen_document: u64,
    watcher: tokio::task::JoinHandle<()>,
    /// The new-document script that applies the speaker setting.
    audio_script: Option<String>,
}

impl Drop for Tab {
    fn drop(&mut self) {
        self.watcher.abort();
    }
}

/// One Chromium shared by a project's chats.
struct SharedBrowser {
    process: tokio::sync::Mutex<BrowserProcess>,
    cdp: Arc<Cdp>,
    http_address: String,
}

/// Where browsers live and which one to use.
#[derive(Debug, Clone)]
pub struct BrowserConfig {
    /// App-data folder for profiles and a downloaded browser.
    pub root: PathBuf,
    /// The user file's `browser.executable`.
    pub executable: Option<PathBuf>,
    pub headless: bool,
}

/// All chats' browsers; one per app.
pub struct BrowserHub {
    config: Mutex<BrowserConfig>,
    shared: tokio::sync::Mutex<HashMap<String, Arc<SharedBrowser>>>,
    chats: Mutex<HashMap<String, Arc<ChatBrowser>>>,
    /// Whether the built-in engine may be downloaded (the app; never tests).
    download_engine: AtomicBool,
    /// One engine install at a time.
    engine_install: tokio::sync::Mutex<()>,
}

impl std::fmt::Debug for BrowserHub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BrowserHub").finish_non_exhaustive()
    }
}

/// The built-in engine downloaded into app-data, if any.
pub fn downloaded_browser(root: &Path) -> Option<PathBuf> {
    super::engine::installed(root)
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

    /// Whether pages run in a browser with no window.
    fn headless(&self) -> bool {
        self.config
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .headless
    }

    /// Whether the automatic choice is the built-in engine (installed or downloadable).
    pub fn built_in(&self) -> bool {
        downloaded_browser(&self.root()).is_some()
            || (self.download_engine.load(Ordering::Relaxed) && super::engine::platform().is_some())
    }

    /// Installs the built-in engine unless it is there, one install at a time.
    async fn install_engine(&self, root: &Path) -> Result<PathBuf, String> {
        let _one = self.engine_install.lock().await;
        if let Some(installed) = downloaded_browser(root) {
            return Ok(installed);
        }
        super::engine::install(root)
            .await
            .map_err(|error| format!("could not download the browser engine: {error:#}"))
    }

    /// The engine to use when the automatic choice applies: the downloaded one, else
    /// (with downloads allowed) it is downloaded now, so the person's own Chrome is never
    /// started and never shows in the Dock. If the download fails, an installed browser is
    /// used instead; with none, the download error is reported.
    async fn engine_for(&self, config: &BrowserConfig) -> Result<Option<PathBuf>, String> {
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
                ui_stream: AtomicBool::new(false),
                preview_width: Mutex::new(None),
                recording: Mutex::new(None),
                viewport: Mutex::new(Viewport::default()),
                muted: AtomicBool::new(true),
                downloads: Mutex::new(Vec::new()),
                person_until: Mutex::new(None),
                download_policy: Mutex::new(None),
                targets: Mutex::new(HashSet::new()),
                view: Mutex::default(),
                stream_start: tokio::sync::Mutex::new(()),
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

    /// Closes a chat's tabs (chat deleted); the shared browser exits when no
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

    /// Closes `project`'s browser (every chat's tabs on it) and waits for Chrome to let go of
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

    async fn release_unused(&self) {
        let used: HashSet<String> = {
            let chats = self.chats.lock().unwrap_or_else(|e| e.into_inner());
            chats
                .values()
                .map(|chat| chat.profile_key.clone())
                .collect()
        };
        self.shared.lock().await.retain(|key, _| used.contains(key));
    }

    async fn shared_for(self: &Arc<Self>, key: &str) -> Result<Arc<SharedBrowser>, String> {
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

fn profile_key(project: &Path) -> String {
    use sha2::{Digest, Sha256};
    let canonical = project
        .canonicalize()
        .unwrap_or_else(|_| project.to_path_buf());
    let digest = Sha256::digest(canonical.to_string_lossy().as_bytes());
    digest.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

#[derive(Default)]
struct ChatInner {
    browser: Option<Arc<SharedBrowser>>,
    tabs: Vec<Tab>,
    current: usize,
}

/// The chat's browser connection and tabs as of their last change. The person's input
/// and the Workspace view read this, never `inner`: an agent action holds `inner` while
/// its page settles (seconds on busy sites), and neither side waits for the other.
#[derive(Clone, Default)]
struct View {
    cdp: Option<Arc<Cdp>>,
    /// `127.0.0.1:<port>` of the browser's debugging server (for DevTools).
    http_address: Option<String>,
    tabs: Vec<ViewTab>,
    current: usize,
}

#[derive(Clone)]
struct ViewTab {
    target_id: String,
    session_id: String,
    state: Arc<Mutex<TabState>>,
}

impl View {
    /// The connection and session of the current tab, if a page is open.
    fn current(&self) -> Option<(Arc<Cdp>, String)> {
        let tab = self.tabs.get(self.current)?;
        Some((Arc::clone(self.cdp.as_ref()?), tab.session_id.clone()))
    }
}

#[derive(Debug, Clone, Copy)]
struct Viewport {
    width: u32,
    height: u32,
    scale: f64,
}

/// The largest frame the screencast sends: the page's full size for the
/// Browser tab, or `preview` wide (same aspect) for the chat's small preview.
fn frame_size(viewport: Viewport, preview: Option<u32>) -> (u32, u32) {
    match preview {
        Some(width) => (
            width,
            (u64::from(width) * u64::from(viewport.height) / u64::from(viewport.width.max(1)))
                as u32,
        ),
        None => (
            (f64::from(viewport.width) * viewport.scale) as u32,
            (f64::from(viewport.height) * viewport.scale) as u32,
        ),
    }
}

impl Default for Viewport {
    fn default() -> Self {
        Self {
            width: DEFAULT_VIEWPORT.0,
            height: DEFAULT_VIEWPORT.1,
            scale: 1.0,
        }
    }
}

/// One chat's view of the browser.
pub struct ChatBrowser {
    hub: std::sync::Weak<BrowserHub>,
    profile_key: String,
    inner: tokio::sync::Mutex<ChatInner>,
    activity: Mutex<Option<String>>,
    updates: broadcast::Sender<BrowserUpdate>,
    /// Its pages are frozen because nothing is using them (see [`Self::freeze_pages`]).
    frozen: AtomicBool,
    /// Session id of the tab being streamed to the Workspace view.
    screencast: Mutex<Option<String>>,
    /// The Workspace view is showing the stream (frames go to it).
    ui_stream: AtomicBool,
    /// The stream feeds the chat's small preview: frames at most this wide,
    /// the page left at its own size. `None` while the Browser tab streams.
    preview_width: Mutex<Option<u32>>,
    recording: Mutex<Option<Recording>>,
    viewport: Mutex<Viewport>,
    /// The Workspace speaker is off (the default): pages play no sound.
    muted: AtomicBool,
    /// Downloads of this chat's tabs, newest last.
    downloads: Mutex<Vec<downloads::Download>>,
    /// Until when a download that starts follows the person's own input (theirs).
    person_until: Mutex<Option<Instant>>,
    /// The current run's download folder and permission.
    download_policy: Mutex<Option<downloads::DownloadPolicy>>,
    /// Target ids of this chat's tabs, readable while an action holds `inner`.
    targets: Mutex<HashSet<String>>,
    /// See [`View`].
    view: Mutex<View>,
    /// One screencast start at a time (Chrome refuses a second start on a tab).
    stream_start: tokio::sync::Mutex<()>,
}

impl std::fmt::Debug for ChatBrowser {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChatBrowser")
            .field("profile", &self.profile_key)
            .finish_non_exhaustive()
    }
}

/// The outcome of one agent action: a fresh snapshot plus what happened.
#[derive(Debug, Clone, Serialize)]
pub struct ActionReport {
    pub url: String,
    pub title: String,
    /// `"1 of 2"` when the chat has more than one tab.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tab: Option<String>,
    pub snapshot: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub new_since_last_action: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot_note: Option<String>,
}

/// A key press, parsed from `Enter`, `Control+a`, `Meta+Shift+k`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyPress {
    pub key: String,
    pub code: String,
    pub key_code: u32,
    pub text: Option<String>,
    pub modifiers: u32,
}

/// Parses a key description; `None` for unknown keys.
pub fn parse_key(description: &str) -> Option<KeyPress> {
    let parts: Vec<&str> = description.split('+').map(str::trim).collect();
    let (key, modifiers) = match parts.split_last() {
        // "Control++" means the plus key ("Control+" names no key).
        Some((&"", rest)) => match rest.split_last() {
            Some((&"", modifiers)) => ("+", modifiers),
            _ => return None,
        },
        Some((last, rest)) => (*last, rest),
        None => return None,
    };
    let mut mask = 0;
    for modifier in modifiers {
        mask |= match modifier.to_ascii_lowercase().as_str() {
            "alt" | "option" => 1,
            "control" | "ctrl" => 2,
            "meta" | "cmd" | "command" => 4,
            "shift" => 8,
            _ => return None,
        };
    }
    let named = |key: &str, code: &str, key_code: u32, text: Option<&str>| KeyPress {
        key: key.to_owned(),
        code: code.to_owned(),
        key_code,
        text: text.map(str::to_owned),
        modifiers: mask,
    };
    let press = match key.to_ascii_lowercase().as_str() {
        "enter" | "return" => named("Enter", "Enter", 13, Some("\r")),
        "tab" => named("Tab", "Tab", 9, None),
        "escape" | "esc" => named("Escape", "Escape", 27, None),
        "backspace" => named("Backspace", "Backspace", 8, None),
        "delete" => named("Delete", "Delete", 46, None),
        "space" | " " => named(" ", "Space", 32, Some(" ")),
        "arrowup" | "up" => named("ArrowUp", "ArrowUp", 38, None),
        "arrowdown" | "down" => named("ArrowDown", "ArrowDown", 40, None),
        "arrowleft" | "left" => named("ArrowLeft", "ArrowLeft", 37, None),
        "arrowright" | "right" => named("ArrowRight", "ArrowRight", 39, None),
        "home" => named("Home", "Home", 36, None),
        "end" => named("End", "End", 35, None),
        "pageup" => named("PageUp", "PageUp", 33, None),
        "pagedown" => named("PageDown", "PageDown", 34, None),
        lower
            if lower.len() > 1
                && lower.starts_with('f')
                && lower[1..]
                    .parse::<u32>()
                    .is_ok_and(|n| (1..=12).contains(&n)) =>
        {
            let number: u32 = lower[1..].parse().unwrap_or(1);
            named(
                &format!("F{number}"),
                &format!("F{number}"),
                111 + number,
                None,
            )
        }
        _ => {
            let mut chars = key.chars();
            let c = chars.next()?;
            if chars.next().is_some() {
                return None;
            }
            let shifted = mask & 8 != 0;
            let shown = if shifted { c.to_ascii_uppercase() } else { c };
            let code = if c.is_ascii_alphabetic() {
                format!("Key{}", c.to_ascii_uppercase())
            } else if c.is_ascii_digit() {
                format!("Digit{c}")
            } else {
                String::new()
            };
            let key_code = if c.is_ascii_alphanumeric() {
                u32::from(c.to_ascii_uppercase())
            } else {
                0
            };
            // Shortcuts (Control/Meta/Alt) type nothing.
            let text = (mask & 7 == 0).then(|| shown.to_string());
            KeyPress {
                key: shown.to_string(),
                code,
                key_code,
                text,
                modifiers: mask,
            }
        }
    };
    Some(press)
}

fn cap(text: &str, limit: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = flat.chars();
    let mut out: String = chars.by_ref().take(limit).collect();
    if chars.next().is_some() {
        out.push('…');
    }
    out
}

/// Applies the speaker setting to the tab's current and future documents;
/// returns the new-document script's id.
async fn install_audio(cdp: &Cdp, session: &str, muted: bool) -> Result<String, String> {
    let source = format!(
        "{}\nwindow.__themisAudio({muted});",
        include_str!("audio.js")
    );
    let added = cdp
        .call(
            Some(session),
            "Page.addScriptToEvaluateOnNewDocument",
            json!({"source": source}),
        )
        .await?;
    let identifier = added["identifier"]
        .as_str()
        .ok_or("the browser did not accept the sound setting")?
        .to_owned();
    let ran = cdp
        .call(
            Some(session),
            "Runtime.evaluate",
            json!({"expression": source}),
        )
        .await?;
    if ran.get("exceptionDetails").is_some() {
        return Err("the page did not take the sound setting".into());
    }
    Ok(identifier)
}

fn push_note(state: &Mutex<TabState>, note: String) {
    let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
    if state.notes.len() < MAX_NOTES {
        state.notes.push(cap(&note, NOTE_CHARS));
    }
}

/// URL without its query and fragment, for notes (queries can carry tokens).
fn short_url(url: &str) -> String {
    let end = url.find(['?', '#']).unwrap_or(url.len());
    cap(&url[..end], 150)
}

/// Updates a tab's state from its events. Runs until the browser closes.
/// The shortest gap between streamed frames: about 30 a second, smooth to
/// watch at half the JPEG work of 60. Recordings sample 10 a second.
const FRAME_GAP: Duration = Duration::from_millis(33);

/// Totals since the app started, for diagnostics (see [`stream_stats`]).
static STREAM_STARTS: AtomicU64 = AtomicU64::new(0);
static STREAM_STOPS: AtomicU64 = AtomicU64::new(0);
static STREAM_RESTARTS_AFTER_LAG: AtomicU64 = AtomicU64::new(0);
static FRAMES: AtomicU64 = AtomicU64::new(0);
static LAGGED_EVENTS: AtomicU64 = AtomicU64::new(0);

fn tracing_lagged(missed: u64) {
    LAGGED_EVENTS.fetch_add(missed, Ordering::Relaxed);
}

/// Workspace stream totals since the app started.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct StreamStats {
    pub starts: u64,
    pub stops: u64,
    /// Restarts after a tab's protocol events overflowed (see `FRAME_GAP`).
    pub restarts_after_lag: u64,
    pub frames: u64,
    pub lagged_events: u64,
}

pub fn stream_stats() -> StreamStats {
    StreamStats {
        starts: STREAM_STARTS.load(Ordering::Relaxed),
        stops: STREAM_STOPS.load(Ordering::Relaxed),
        restarts_after_lag: STREAM_RESTARTS_AFTER_LAG.load(Ordering::Relaxed),
        frames: FRAMES.load(Ordering::Relaxed),
        lagged_events: LAGGED_EVENTS.load(Ordering::Relaxed),
    }
}

fn spawn_watcher(
    cdp: Arc<Cdp>,
    target_id: String,
    session_id: String,
    state: Arc<Mutex<TabState>>,
    chat: std::sync::Weak<ChatBrowser>,
) -> tokio::task::JoinHandle<()> {
    let mut events = cdp.subscribe();
    tokio::spawn(async move {
        // When the next frame may be acknowledged (see FRAME_GAP).
        let mut next_ack = std::time::Instant::now();
        loop {
            let event = match events.recv().await {
                Ok(event) => event,
                Err(broadcast::error::RecvError::Lagged(missed)) => {
                    // A dropped frame is never acknowledged, and Chrome sends
                    // no more until it is: restart this tab's stream.
                    tracing_lagged(missed);
                    if let Some(chat) = chat.upgrade() {
                        let streaming = chat
                            .screencast
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .as_deref()
                            == Some(session_id.as_str());
                        if streaming {
                            STREAM_RESTARTS_AFTER_LAG.fetch_add(1, Ordering::Relaxed);
                            tokio::spawn(async move { chat.restart_screencast().await });
                        }
                    }
                    continue;
                }
                Err(broadcast::error::RecvError::Closed) => break,
            };
            if event.session.as_deref() != Some(session_id.as_str()) {
                if event.method == "Target.targetInfoChanged"
                    && event.params["targetInfo"]["targetId"] == target_id.as_str()
                {
                    {
                        let mut tab = state.lock().unwrap_or_else(|e| e.into_inner());
                        let info = &event.params["targetInfo"];
                        let url = info["url"].as_str().unwrap_or(&tab.url).to_owned();
                        let title = info["title"].as_str().unwrap_or_default();
                        // Until a page has a title, Chrome reports its address without the
                        // scheme; that never replaces the page's real title (read on load).
                        let placeholder =
                            url.split_once("://").map(|(_, rest)| rest) == Some(title);
                        if url != tab.url || !placeholder {
                            tab.title = cap(title, 200);
                        }
                        tab.url = url;
                    }
                    if let Some(chat) = chat.upgrade() {
                        chat.publish_state().await;
                    }
                }
                continue;
            }
            handle_tab_event(&cdp, &target_id, &session_id, &state, &event);
            let committed = event.method == "Page.frameNavigated"
                && event.params["frame"]["parentId"].is_null();
            if committed || event.method == "Page.loadEventFired" {
                // A navigation to another site moves the page to a new renderer, which ends
                // its screencast: start it again once the new page is committed. Chrome
                // reports the title only when the address commits (the address itself
                // then), so read the page's own title at commit (a page restored by
                // Back/Forward has it already and fires no load) and again on load.
                if let Some(chat) = chat.upgrade() {
                    let restart = committed
                        && chat
                            .screencast
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .as_deref()
                            == Some(session_id.as_str());
                    let (cdp, session_id, state) =
                        (Arc::clone(&cdp), session_id.clone(), Arc::clone(&state));
                    tokio::spawn(async move {
                        let (document, frames) = {
                            let tab = state.lock().unwrap_or_else(|e| e.into_inner());
                            (tab.document, tab.frames)
                        };
                        if restart {
                            chat.restart_screencast().await;
                        }
                        refresh_title(&cdp, &session_id, &state).await;
                        // After the restart, so the page (and the end of loading) reaches
                        // the view with its stream.
                        chat.publish_state().await;
                        if committed {
                            settle_view(&chat, &cdp, &session_id, &state, document, frames).await;
                        }
                    });
                }
            }
            if event.method == "Debugger.paused" {
                devtools::resume_later(Arc::clone(&cdp), session_id.clone(), Arc::clone(&state));
            }
            if event.method == "Page.screencastFrame" {
                // Chrome sends the next frame only after this acknowledgement;
                // spacing them caps the stream without skipping a still page's
                // only frame.
                FRAMES.fetch_add(1, Ordering::Relaxed);
                state.lock().unwrap_or_else(|e| e.into_inner()).frames += 1;
                let ack = json!({"sessionId": event.params["sessionId"]});
                let now = std::time::Instant::now();
                let wait = next_ack.saturating_duration_since(now);
                next_ack = now.max(next_ack) + FRAME_GAP;
                if wait.is_zero() {
                    cdp.send(Some(&session_id), "Page.screencastFrameAck", ack);
                } else {
                    let (cdp, session_id) = (Arc::clone(&cdp), session_id.clone());
                    tokio::spawn(async move {
                        tokio::time::sleep(wait).await;
                        cdp.send(Some(&session_id), "Page.screencastFrameAck", ack);
                    });
                }
                if let Some(chat) = chat.upgrade() {
                    let streaming = chat
                        .screencast
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .as_deref()
                        == Some(session_id.as_str());
                    let data = event.params["data"].as_str().unwrap_or_default();
                    if streaming {
                        chat.record_frame(data);
                    }
                    if streaming && chat.ui_stream.load(Ordering::SeqCst) {
                        let _ = chat.updates.send(BrowserUpdate::Frame {
                            data: data.to_owned(),
                            width: event.params["metadata"]["deviceWidth"]
                                .as_f64()
                                .unwrap_or(0.0) as u32,
                            height: event.params["metadata"]["deviceHeight"]
                                .as_f64()
                                .unwrap_or(0.0) as u32,
                        });
                    }
                }
            } else if matches!(
                event.method.as_str(),
                "Page.frameNavigated"
                    | "Page.loadEventFired"
                    | "Page.navigatedWithinDocument"
                    | "Runtime.exceptionThrown"
                    | "Page.frameStartedLoading"
                    | "Page.frameStoppedLoading"
            ) {
                if let Some(chat) = chat.upgrade() {
                    chat.publish_state().await;
                }
            }
        }
    })
}

/// After a commit, makes sure the view catches up with the new page. A page restored by
/// Back/Forward fires no load, and Chrome may report its title before the commit (which
/// clears it), so the read at commit can be the only one and may find no title yet; a
/// restarted stream may also not draw. Until the next navigation, the title is read again
/// while it is missing and the stream is restarted when no frame has come for a second.
async fn settle_view(
    chat: &ChatBrowser,
    cdp: &Cdp,
    session_id: &str,
    state: &Mutex<TabState>,
    document: u64,
    frames_before: u64,
) {
    for attempt in 1..=12 {
        tokio::time::sleep(Duration::from_millis(250)).await;
        let (moved_on, titled, drawn) = {
            let tab = state.lock().unwrap_or_else(|e| e.into_inner());
            (
                tab.document != document,
                !tab.title.is_empty(),
                tab.frames > frames_before,
            )
        };
        let streaming = chat
            .screencast
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_deref()
            == Some(session_id);
        if moved_on || (titled && (drawn || !streaming)) {
            return;
        }
        if streaming && !drawn && attempt % 4 == 0 {
            chat.restart_screencast().await;
        }
        if !titled {
            refresh_title(cdp, session_id, state).await;
        }
        chat.publish_state().await;
    }
}

/// Reads the page's own title into the tab's state (kept when the page has none yet).
async fn refresh_title(cdp: &Cdp, session_id: &str, state: &Mutex<TabState>) {
    let Ok(read) = cdp
        .call(
            Some(session_id),
            "Runtime.evaluate",
            json!({"expression": "document.title", "returnByValue": true}),
        )
        .await
    else {
        return;
    };
    if let Some(title) = read["result"]["value"].as_str().filter(|t| !t.is_empty()) {
        state.lock().unwrap_or_else(|e| e.into_inner()).title = cap(title, 200);
    }
}

/// Applies one event of a tab to its state (and answers dialogs).
fn handle_tab_event(
    cdp: &Cdp,
    target_id: &str,
    session_id: &str,
    state: &Mutex<TabState>,
    event: &CdpEvent,
) {
    let params = &event.params;
    let main_frame = |frame: &Value| frame.as_str() == Some(target_id);
    devtools::record(state, event);
    match event.method.as_str() {
        "Page.frameStartedLoading" if main_frame(&params["frameId"]) => {
            state.lock().unwrap_or_else(|e| e.into_inner()).loading = true;
        }
        "Page.frameStoppedLoading" if main_frame(&params["frameId"]) => {
            state.lock().unwrap_or_else(|e| e.into_inner()).loading = false;
        }
        "Page.loadEventFired" => {
            state.lock().unwrap_or_else(|e| e.into_inner()).loading = false;
        }
        "Page.frameNavigated" if params["frame"]["parentId"].is_null() => {
            let mut tab = state.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(url) = params["frame"]["url"].as_str() {
                tab.url = url.to_owned();
            }
            // A new document: the old page's title no longer applies (read again on load).
            tab.title.clear();
            tab.document += 1;
            tab.console_errors = 0;
        }
        "Page.navigatedWithinDocument" if main_frame(&params["frameId"]) => {
            if let Some(url) = params["url"].as_str() {
                state.lock().unwrap_or_else(|e| e.into_inner()).url = url.to_owned();
            }
        }
        "Network.requestWillBeSent" => {
            if let Some(id) = params["requestId"].as_str() {
                let mut tab = state.lock().unwrap_or_else(|e| e.into_inner());
                tab.inflight.insert(id.to_owned(), Instant::now());
                tab.last_network = Some(Instant::now());
            }
        }
        "Network.loadingFinished" | "Network.loadingFailed" => {
            if let Some(id) = params["requestId"].as_str() {
                let mut tab = state.lock().unwrap_or_else(|e| e.into_inner());
                tab.inflight.remove(id);
                tab.last_network = Some(Instant::now());
            }
            if event.method == "Network.loadingFailed"
                && !params["canceled"].as_bool().unwrap_or(false)
                && matches!(params["type"].as_str(), Some("Document" | "XHR" | "Fetch"))
            {
                push_note(
                    state,
                    format!(
                        "request failed: {}",
                        params["errorText"].as_str().unwrap_or("network error")
                    ),
                );
            }
        }
        "Network.responseReceived" => {
            let status = params["response"]["status"].as_u64().unwrap_or(0);
            if status >= 400
                && matches!(params["type"].as_str(), Some("Document" | "XHR" | "Fetch"))
            {
                push_note(
                    state,
                    format!(
                        "HTTP {status} for {}",
                        short_url(params["response"]["url"].as_str().unwrap_or_default())
                    ),
                );
            }
        }
        "Runtime.exceptionThrown" => {
            let details = &params["exceptionDetails"];
            let text = details["exception"]["description"]
                .as_str()
                .or_else(|| details["text"].as_str())
                .unwrap_or("error");
            let first_line = text.lines().next().unwrap_or(text);
            state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .console_errors += 1;
            push_note(state, format!("uncaught {first_line}"));
        }
        "Runtime.consoleAPICalled" if params["type"] == "error" => {
            let text: Vec<String> = params["args"]
                .as_array()
                .into_iter()
                .flatten()
                .take(4)
                .map(|arg| {
                    arg["value"]
                        .as_str()
                        .map(str::to_owned)
                        .or_else(|| arg["description"].as_str().map(str::to_owned))
                        .unwrap_or_else(|| arg["value"].to_string())
                })
                .collect();
            state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .console_errors += 1;
            push_note(state, format!("console.error: {}", text.join(" ")));
        }
        "Page.javascriptDialogOpening" => {
            let kind = params["type"].as_str().unwrap_or("alert");
            // Alerts and leave-page prompts are accepted; confirm and prompt are
            // dismissed, so a page cannot get a "yes" the agent never gave.
            let accept = matches!(kind, "alert" | "beforeunload");
            cdp.send(
                Some(session_id),
                "Page.handleJavaScriptDialog",
                json!({"accept": accept}),
            );
            push_note(
                state,
                format!(
                    "{kind} dialog \"{}\" was {}",
                    cap(params["message"].as_str().unwrap_or_default(), 150),
                    if accept { "accepted" } else { "dismissed" }
                ),
            );
        }
        _ => {}
    }
}

impl ChatBrowser {
    /// Stops the chat's pages running while nothing uses them: a 3D page left open takes
    /// several cores here, since this browser draws on the CPU. Screenshots and scripts still
    /// work on a frozen page. Not while the person watches the Browser tab or a recording
    /// runs. The next action, input or view brings them back ([`Self::thaw_pages`]).
    pub async fn freeze_pages(&self) {
        let watching = self.ui_stream.load(Ordering::SeqCst)
            && self
                .preview_width
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_none();
        let recording = self
            .recording
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some();
        if watching || recording || self.frozen.swap(true, Ordering::SeqCst) {
            return;
        }
        self.set_page_state("frozen").await;
        crate::diagnostics::emit("browser", "pages_frozen", "info", json!({}));
    }

    /// Brings frozen pages back; an agent must never read a page that has stopped.
    pub async fn thaw_pages(&self) {
        if self.frozen.swap(false, Ordering::SeqCst) {
            self.set_page_state("active").await;
            crate::diagnostics::emit("browser", "pages_thawed", "info", json!({}));
        }
    }

    async fn set_page_state(&self, state: &str) {
        let view = self.view();
        let Some(cdp) = view.cdp else { return };
        for tab in &view.tabs {
            let session = Some(tab.session_id.as_str());
            let _ = cdp
                .call(
                    session,
                    "Page.setWebLifecycleState",
                    json!({"state": state}),
                )
                .await;
            // A thawed page stays hidden and never draws again until its focus state is set
            // afresh (tested against the real engine): switched off first, since setting
            // the same value again changes nothing.
            if state == "active" {
                for enabled in [false, true] {
                    let _ = cdp
                        .call(
                            session,
                            "Emulation.setFocusEmulationEnabled",
                            json!({"enabled": enabled}),
                        )
                        .await;
                }
            }
        }
    }

    /// Updates for the Workspace view.
    pub fn subscribe(&self) -> broadcast::Receiver<BrowserUpdate> {
        self.updates.subscribe()
    }

    /// Shows what the agent is doing (or nothing).
    pub async fn set_activity(&self, activity: Option<String>) {
        let done = activity.is_none();
        *self.activity.lock().unwrap_or_else(|e| e.into_inner()) = activity;
        if done {
            // The action is over, so waiting is safe; a dropped update would
            // leave "Agent is …" showing after the run.
            self.publish_state().await;
        } else {
            self.publish_state().await;
        }
    }

    /// Turns the Workspace speaker off (`muted`) or on for every tab.
    /// The setting is reported only once every tab took it; on failure the
    /// previous setting stays (and is shown).
    pub async fn set_muted(&self, muted: bool) -> Result<(), String> {
        let previous = self.muted.swap(muted, Ordering::SeqCst);
        let result = self.apply_muted(muted).await;
        if result.is_err() {
            self.muted.store(previous, Ordering::SeqCst);
            let _ = self.apply_muted(previous).await;
        }
        self.publish_state().await;
        result
    }

    async fn apply_muted(&self, muted: bool) -> Result<(), String> {
        let mut inner = self.inner.lock().await;
        let Some(browser) = inner.browser.clone() else {
            return Ok(());
        };
        for tab in &mut inner.tabs {
            // The new script goes in before the old one comes out.
            let new = install_audio(&browser.cdp, &tab.session_id, muted).await?;
            if let Some(old) = tab.audio_script.replace(new) {
                let _ = browser
                    .cdp
                    .call(
                        Some(&tab.session_id),
                        "Page.removeScriptToEvaluateOnNewDocument",
                        json!({"identifier": old}),
                    )
                    .await;
            }
        }
        Ok(())
    }

    /// The current state, for the Workspace view.
    pub async fn state(&self) -> BrowserState {
        self.state_of().await
    }

    /// Records the browser and tabs for [`View`] readers; called after every change.
    fn remember(&self, inner: &ChatInner) {
        *self.view.lock().unwrap_or_else(|e| e.into_inner()) = View {
            cdp: inner
                .browser
                .as_ref()
                .map(|browser| Arc::clone(&browser.cdp)),
            http_address: inner
                .browser
                .as_ref()
                .map(|browser| browser.http_address.clone()),
            tabs: inner
                .tabs
                .iter()
                .map(|tab| ViewTab {
                    target_id: tab.target_id.clone(),
                    session_id: tab.session_id.clone(),
                    state: Arc::clone(&tab.state),
                })
                .collect(),
            current: inner.current,
        };
    }

    fn view(&self) -> View {
        self.view.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    async fn state_of(&self) -> BrowserState {
        let view = self.view();
        let tabs: Vec<TabInfo> = view
            .tabs
            .iter()
            .map(|tab| {
                let state = tab.state.lock().unwrap_or_else(|e| e.into_inner());
                TabInfo {
                    title: state.title.clone(),
                    url: state.url.clone(),
                }
            })
            .collect();
        let current = view.tabs.get(view.current);
        let (url, title, errors, loading) = current
            .map(|tab| {
                let state = tab.state.lock().unwrap_or_else(|e| e.into_inner());
                (
                    state.url.clone(),
                    state.title.clone(),
                    state.console_errors,
                    state.loading,
                )
            })
            .unwrap_or_default();
        let (back, forward) = match (current, &view.cdp) {
            (Some(tab), Some(cdp)) => cdp
                .call_with_timeout(
                    Some(&tab.session_id),
                    "Page.getNavigationHistory",
                    json!({}),
                    Duration::from_secs(2),
                )
                .await
                .map(|history| {
                    let index = history["currentIndex"].as_u64().unwrap_or(0) as usize;
                    let count = history["entries"].as_array().map_or(0, Vec::len);
                    (index > 0, index + 1 < count)
                })
                .unwrap_or_default(),
            _ => (false, false),
        };
        let activity = self
            .activity
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        BrowserState {
            open: !view.tabs.is_empty(),
            current: view.current,
            tabs,
            url,
            title,
            can_go_back: back,
            can_go_forward: forward,
            controller: if activity.is_some() {
                "agent".into()
            } else {
                "idle".into()
            },
            activity,
            console_errors: errors,
            loading,
            muted: self.muted.load(Ordering::SeqCst),
            recording: self
                .recording
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_ref()
                .map(|recording| RecordingInfo {
                    by: recording.by,
                    seconds: recording.elapsed().as_secs(),
                    max_seconds: recording.max.as_secs(),
                }),
        }
    }

    /// Sends the current state to the Workspace view (never waits on an action).
    async fn publish_state(&self) {
        if self.updates.receiver_count() == 0 {
            return;
        }
        let state = self.state_of().await;
        let _ = self.updates.send(BrowserUpdate::State(state));
    }

    /// The host of the current page (for approvals), if a page is open.
    pub async fn current_site(&self) -> Option<String> {
        let inner = self.inner.lock().await;
        let tab = inner.tabs.get(inner.current)?;
        let url = tab
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .url
            .clone();
        site_of(&url)
    }

    /// DevTools for the current tab, served by the browser itself: the plain DevTools front
    /// end (`inspector.html` adds a second, smaller live copy of the page, which the
    /// Browser tab already shows above it).
    pub async fn devtools_url(&self) -> Option<String> {
        let view = self.view();
        let tab = view.tabs.get(view.current)?;
        Some(format!(
            "http://{0}/devtools/devtools_app.html?ws={0}/devtools/page/{1}",
            view.http_address.as_deref()?,
            tab.target_id
        ))
    }

    async fn browser(&self, inner: &mut ChatInner) -> Result<Arc<SharedBrowser>, String> {
        if let Some(browser) = &inner.browser {
            if !browser.cdp.is_closed() {
                return Ok(Arc::clone(browser));
            }
            // The browser exited: its tabs are gone too.
            inner.tabs.clear();
            self.targets
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clear();
            inner.current = 0;
        }
        let hub = self.hub.upgrade().ok_or("the app is closing")?;
        let browser = hub.shared_for(&self.profile_key).await?;
        inner.browser = Some(Arc::clone(&browser));
        self.remember(inner);
        Ok(browser)
    }

    async fn new_tab(self: &Arc<Self>, inner: &mut ChatInner) -> Result<(), String> {
        if inner.tabs.len() >= MAX_TABS {
            return Err(format!(
                "this chat already has {MAX_TABS} tabs open; close one with browser_tabs"
            ));
        }
        let browser = self.browser(inner).await?;
        let cdp = &browser.cdp;
        let created = cdp
            .call(None, "Target.createTarget", json!({"url": "about:blank"}))
            .await?;
        let target_id = created["targetId"].as_str().ok_or("no tab id")?.to_owned();
        let attached = cdp
            .call(
                None,
                "Target.attachToTarget",
                json!({"targetId": target_id, "flatten": true}),
            )
            .await?;
        let session_id = attached["sessionId"]
            .as_str()
            .ok_or("no session id")?
            .to_owned();
        let state = Arc::new(Mutex::new(TabState {
            url: "about:blank".into(),
            ..Default::default()
        }));
        let watcher = spawn_watcher(
            Arc::clone(cdp),
            target_id.clone(),
            session_id.clone(),
            Arc::clone(&state),
            Arc::downgrade(self),
        );
        let _ = cdp
            .call(None, "Target.setDiscoverTargets", json!({"discover": true}))
            .await;
        for method in [
            "Page.enable",
            "Network.enable",
            "Runtime.enable",
            "DOM.enable",
            "Accessibility.enable",
            "Log.enable",
        ] {
            cdp.call(Some(&session_id), method, json!({})).await?;
        }
        let viewport = *self.viewport.lock().unwrap_or_else(|e| e.into_inner());
        let _ = cdp
            .call(
                Some(&session_id),
                "Emulation.setDeviceMetricsOverride",
                json!({"width": viewport.width, "height": viewport.height, "deviceScaleFactor": viewport.scale, "mobile": false}),
            )
            .await;
        // The stand-in is a convenience: a page that refuses it just has no pointer lock.
        if self.hub.upgrade().is_some_and(|hub| hub.headless()) {
            let _ = cdp
                .call(
                    Some(&session_id),
                    "Page.addScriptToEvaluateOnNewDocument",
                    json!({"source": include_str!("pointer_lock.js")}),
                )
                .await;
        }
        // A tab that cannot take the sound setting is not opened.
        let audio_script =
            Some(install_audio(cdp, &session_id, self.muted.load(Ordering::SeqCst)).await?);
        self.targets
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(target_id.clone());
        inner.tabs.push(Tab {
            target_id,
            session_id,
            state,
            refs: RefMap::default(),
            names: HashMap::new(),
            seen_document: 0,
            watcher,
            audio_script,
        });
        inner.current = inner.tabs.len() - 1;
        self.remember(inner);
        Ok(())
    }

    /// Ensures a current tab exists; returns the browser connection and its session.
    async fn current(
        self: &Arc<Self>,
        inner: &mut ChatInner,
    ) -> Result<(Arc<Cdp>, String), String> {
        self.thaw_pages().await;
        let browser = self.browser(inner).await?;
        if inner.tabs.is_empty() {
            self.new_tab(inner).await?;
        }
        let tab = &inner.tabs[inner.current];
        Ok((Arc::clone(&browser.cdp), tab.session_id.clone()))
    }

    /// Loads `url` in the current tab, or a new one.
    pub async fn open(self: &Arc<Self>, url: &str, new_tab: bool) -> Result<ActionReport, String> {
        let mut inner = self.inner.lock().await;
        if new_tab && !inner.tabs.is_empty() {
            self.new_tab(&mut inner).await?;
        }
        let (cdp, session) = self.current(&mut inner).await?;
        let navigated = cdp
            .call_with_timeout(
                Some(&session),
                "Page.navigate",
                json!({"url": url}),
                NAVIGATION_WAIT,
            )
            .await?;
        let mut note = None;
        if let Some(error) = navigated["errorText"].as_str().filter(|e| !e.is_empty()) {
            note = Some(format!("could not open {}: {error}", short_url(url)));
        }
        self.settle(&inner, true).await;
        let mut report = self.report(&mut inner, None).await?;
        if let Some(note) = note {
            report.new_since_last_action.insert(0, note);
        }
        drop(inner);
        self.publish_state().await;
        self.restart_screencast().await;
        Ok(report)
    }

    /// The person typed an address in the Workspace view.
    pub async fn open_for_user(self: &Arc<Self>, url: &str) -> Result<(), String> {
        self.person_acted();
        // With a page open, go there at once, even while an agent action runs; the
        // stream follows when the new page commits.
        if let Some((cdp, session)) = self.view().current() {
            cdp.call(Some(&session), "Page.navigate", json!({"url": url}))
                .await?;
            self.publish_state().await;
            return Ok(());
        }
        let mut inner = self.inner.lock().await;
        let (cdp, session) = self.current(&mut inner).await?;
        cdp.call(Some(&session), "Page.navigate", json!({"url": url}))
            .await?;
        drop(inner);
        self.publish_state().await;
        self.restart_screencast().await;
        Ok(())
    }

    /// Back, forward or reload.
    pub async fn navigate(self: &Arc<Self>, action: &str) -> Result<ActionReport, String> {
        let mut inner = self.inner.lock().await;
        if inner.tabs.is_empty() {
            return Err("no page is open; use browser_open first".into());
        }
        let (cdp, session) = self.current(&mut inner).await?;
        history_step(&cdp, &session, action).await?;
        self.settle(&inner, true).await;
        let report = self.report(&mut inner, None).await;
        drop(inner);
        self.publish_state().await;
        report
    }

    /// Back/forward/reload for the person's toolbar.
    pub async fn navigate_for_user(&self, action: &str) -> Result<(), String> {
        self.thaw_pages().await;
        self.person_acted();
        let Some((cdp, session)) = self.view().current() else {
            return Ok(());
        };
        history_step(&cdp, &session, action).await
    }

    /// A fresh snapshot (of one element's subtree with `reference`).
    pub async fn snapshot(
        self: &Arc<Self>,
        reference: Option<&str>,
    ) -> Result<ActionReport, String> {
        let mut inner = self.inner.lock().await;
        if inner.tabs.is_empty() {
            return Err("no page is open; use browser_open first".into());
        }
        self.report(&mut inner, reference).await
    }

    /// The accessible name of a ref from the last snapshot, for labels.
    pub async fn name_of(&self, reference: &str) -> Option<String> {
        let inner = self.inner.lock().await;
        let tab = inner.tabs.get(inner.current)?;
        let backend = tab.refs.resolve(reference)?;
        tab.names.get(&backend).cloned()
    }

    async fn element(
        &self,
        inner: &ChatInner,
        reference: &str,
    ) -> Result<(Arc<Cdp>, String, i64), String> {
        self.thaw_pages().await;
        let browser = inner
            .browser
            .as_ref()
            .ok_or("no page is open; use browser_open first")?;
        let tab = inner
            .tabs
            .get(inner.current)
            .ok_or("no page is open; use browser_open first")?;
        let backend = tab.refs.resolve(reference).ok_or_else(|| {
            format!("no element {reference} on this page; take a new browser_snapshot and use a ref from it")
        })?;
        Ok((Arc::clone(&browser.cdp), tab.session_id.clone(), backend))
    }

    /// Clicks an element (twice with `double`).
    pub async fn click(
        self: &Arc<Self>,
        reference: &str,
        double: bool,
    ) -> Result<ActionReport, String> {
        let mut inner = self.inner.lock().await;
        let (cdp, session, backend) = self.element(&inner, reference).await?;
        let (x, y) = element_center(&cdp, &session, backend).await?;
        click_at(&cdp, &session, x, y, if double { 2 } else { 1 }).await?;
        self.settle(&inner, false).await;
        let report = self.report(&mut inner, None).await;
        drop(inner);
        self.publish_state().await;
        report
    }

    /// Clicks a point of the visible page in CSS pixels, as a viewport
    /// screenshot shows it: for canvas, maps and games, which have no refs.
    pub async fn click_point(
        self: &Arc<Self>,
        x: f64,
        y: f64,
        double: bool,
    ) -> Result<ActionReport, String> {
        let mut inner = self.inner.lock().await;
        let browser = inner
            .browser
            .clone()
            .ok_or("no page is open; use browser_open first")?;
        let session = inner
            .tabs
            .get(inner.current)
            .ok_or("no page is open; use browser_open first")?
            .session_id
            .clone();
        let metrics = browser
            .cdp
            .call(Some(&session), "Page.getLayoutMetrics", json!({}))
            .await?;
        let view = &metrics["cssVisualViewport"];
        let width = view["clientWidth"].as_f64().unwrap_or(0.0);
        let height = view["clientHeight"].as_f64().unwrap_or(0.0);
        if !(x.is_finite()
            && y.is_finite()
            && (0.0..width).contains(&x)
            && (0.0..height).contains(&y))
        {
            return Err(format!(
                "the point ({x}, {y}) is outside the visible page ({}×{} CSS pixels); take a browser_screenshot and use a point inside it",
                width.round(),
                height.round()
            ));
        }
        click_at(&browser.cdp, &session, x, y, if double { 2 } else { 1 }).await?;
        self.settle(&inner, false).await;
        let report = self.report(&mut inner, None).await;
        drop(inner);
        self.publish_state().await;
        report
    }

    /// Types into an element, optionally clearing it first and pressing Enter after.
    pub async fn type_text(
        self: &Arc<Self>,
        reference: &str,
        text: &str,
        submit: bool,
        clear: bool,
    ) -> Result<ActionReport, String> {
        let mut inner = self.inner.lock().await;
        let (cdp, session, backend) = self.element(&inner, reference).await?;
        let object = resolve_object(&cdp, &session, backend).await?;
        cdp.call(
            Some(&session),
            "DOM.focus",
            json!({"backendNodeId": backend}),
        )
        .await?;
        if clear {
            cdp.call(
                Some(&session),
                "Runtime.callFunctionOn",
                json!({"objectId": object, "functionDeclaration": SELECT_ALL_JS, "awaitPromise": false}),
            )
            .await?;
            press_key(&cdp, &session, &parse_key("Backspace").expect("known key")).await?;
        }
        cdp.call(Some(&session), "Input.insertText", json!({"text": text}))
            .await?;
        if submit {
            press_key(&cdp, &session, &parse_key("Enter").expect("known key")).await?;
        }
        self.settle(&inner, submit).await;
        let report = self.report(&mut inner, None).await;
        drop(inner);
        self.publish_state().await;
        report
    }

    /// Chooses options of a `<select>` by value or label.
    pub async fn select(
        self: &Arc<Self>,
        reference: &str,
        values: &[String],
    ) -> Result<ActionReport, String> {
        let mut inner = self.inner.lock().await;
        let (cdp, session, backend) = self.element(&inner, reference).await?;
        let object = resolve_object(&cdp, &session, backend).await?;
        let result = cdp
            .call(
                Some(&session),
                "Runtime.callFunctionOn",
                json!({"objectId": object, "functionDeclaration": SELECT_JS, "arguments": [{"value": values}], "returnByValue": true}),
            )
            .await?;
        if let Some(error) = result["exceptionDetails"]["exception"]["description"].as_str() {
            return Err(format!(
                "{} is not a dropdown ({}); click it and pick the option instead",
                reference,
                cap(error.lines().next().unwrap_or(error), 100)
            ));
        }
        if result["result"]["value"].as_u64() == Some(0) {
            return Err(format!("no option of {reference} matches {values:?}; check the option labels in a snapshot"));
        }
        self.settle(&inner, false).await;
        let report = self.report(&mut inner, None).await;
        drop(inner);
        self.publish_state().await;
        report
    }

    /// Presses a key (`Enter`, `Escape`, `Control+a`) in the focused element.
    pub async fn press(self: &Arc<Self>, key: &str) -> Result<ActionReport, String> {
        let press = parse_key(key).ok_or_else(|| {
            format!(
                "unknown key '{}'; use names like Enter, Escape, Tab, ArrowDown or Control+a",
                cap(key, 30)
            )
        })?;
        let mut inner = self.inner.lock().await;
        if inner.tabs.is_empty() {
            return Err("no page is open; use browser_open first".into());
        }
        let (cdp, session) = self.current(&mut inner).await?;
        press_key(&cdp, &session, &press).await?;
        self.settle(&inner, press.key == "Enter").await;
        let report = self.report(&mut inner, None).await;
        drop(inner);
        self.publish_state().await;
        report
    }

    /// Scrolls the page (or the element `reference`) up or down.
    pub async fn scroll(
        self: &Arc<Self>,
        direction: &str,
        reference: Option<&str>,
    ) -> Result<ActionReport, String> {
        let mut inner = self.inner.lock().await;
        if inner.tabs.is_empty() {
            return Err("no page is open; use browser_open first".into());
        }
        let (cdp, session) = self.current(&mut inner).await?;
        let metrics = cdp
            .call(Some(&session), "Page.getLayoutMetrics", json!({}))
            .await?;
        let width = metrics["cssVisualViewport"]["clientWidth"]
            .as_f64()
            .unwrap_or(1280.0);
        let height = metrics["cssVisualViewport"]["clientHeight"]
            .as_f64()
            .unwrap_or(800.0);
        let (x, y) = match reference {
            Some(reference) => {
                let (_, _, backend) = self.element(&inner, reference).await?;
                element_center(&cdp, &session, backend).await?
            }
            None => (width / 2.0, height / 2.0),
        };
        let step = height * 0.8;
        let (dx, dy) = match direction.trim().to_ascii_lowercase().as_str() {
            "down" => (0.0, step),
            "up" => (0.0, -step),
            "right" => (width * 0.8, 0.0),
            "left" => (-width * 0.8, 0.0),
            other => {
                return Err(format!(
                    "direction must be up, down, left or right, not '{}'",
                    cap(other, 20)
                ))
            }
        };
        cdp.call(
            Some(&session),
            "Input.dispatchMouseEvent",
            json!({"type": "mouseWheel", "x": x, "y": y, "deltaX": dx, "deltaY": dy}),
        )
        .await?;
        self.settle(&inner, false).await;
        let report = self.report(&mut inner, None).await;
        drop(inner);
        self.publish_state().await;
        report
    }

    /// Lists, switches to or closes tabs (1-based numbers).
    pub async fn tabs(self: &Arc<Self>, action: &str, tab: Option<usize>) -> Result<Value, String> {
        let mut inner = self.inner.lock().await;
        let pick = |count: usize| -> Result<usize, String> {
            let number = tab.ok_or("say which tab (its number from list)")?;
            (1..=count)
                .contains(&number)
                .then(|| number - 1)
                .ok_or_else(|| format!("there is no tab {number}; this chat has {count}"))
        };
        match action.trim().to_ascii_lowercase().as_str() {
            "list" => {}
            "switch" => {
                inner.current = pick(inner.tabs.len())?;
                let target = inner.tabs[inner.current].target_id.clone();
                if let Some(browser) = &inner.browser {
                    let _ = browser
                        .cdp
                        .call(None, "Target.activateTarget", json!({"targetId": target}))
                        .await;
                }
            }
            "close" => {
                let index = pick(inner.tabs.len())?;
                let tab = inner.tabs.remove(index);
                self.targets
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&tab.target_id);
                if let Some(browser) = &inner.browser {
                    let _ = browser
                        .cdp
                        .call(
                            None,
                            "Target.closeTarget",
                            json!({"targetId": tab.target_id}),
                        )
                        .await;
                }
                if inner.current >= inner.tabs.len() {
                    inner.current = inner.tabs.len().saturating_sub(1);
                }
            }
            other => {
                return Err(format!(
                    "action must be list, switch or close, not '{}'",
                    cap(other, 20)
                ))
            }
        }
        self.remember(&inner);
        let state = self.state_of().await;
        drop(inner);
        self.publish_state().await;
        self.restart_screencast().await;
        Ok(json!({
            "tabs": state.tabs.iter().enumerate().map(|(i, tab)| json!({
                "tab": i + 1, "title": tab.title, "url": tab.url, "current": i == state.current && state.open,
            })).collect::<Vec<_>>(),
        }))
    }

    async fn close_all(&self) {
        self.stop_screencast().await;
        let mut inner = self.inner.lock().await;
        if let Some(browser) = &inner.browser {
            for tab in &inner.tabs {
                let _ = browser
                    .cdp
                    .call(
                        None,
                        "Target.closeTarget",
                        json!({"targetId": tab.target_id}),
                    )
                    .await;
            }
        }
        inner.tabs.clear();
        self.targets
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        inner.browser = None;
        self.remember(&inner);
    }

    /// Waits until the page has settled: loaded (after a navigation) and the
    /// network quiet, bounded by [`NAVIGATION_WAIT`] / [`SETTLE_WAIT`].
    async fn settle(&self, inner: &ChatInner, navigation: bool) {
        let Some(tab) = inner.tabs.get(inner.current) else {
            return;
        };
        let started = Instant::now();
        let limit = if navigation {
            NAVIGATION_WAIT
        } else {
            SETTLE_WAIT
        };
        tokio::time::sleep(Duration::from_millis(150)).await;
        let mut loaded_at: Option<Instant> = None;
        loop {
            let (loading, busy) = {
                let state = tab.state.lock().unwrap_or_else(|e| e.into_inner());
                let busy = state
                    .inflight
                    .values()
                    .any(|start| start.elapsed() < LONG_REQUEST)
                    || state
                        .last_network
                        .is_some_and(|last| last.elapsed() < QUIET);
                (state.loading, busy)
            };
            if (!loading && !busy) || started.elapsed() > limit {
                break;
            }
            if loading {
                loaded_at = None;
            } else if loaded_at.get_or_insert_with(Instant::now).elapsed() > BUSY_AFTER_LOAD {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// Snapshot of the current tab plus notes since the last report.
    async fn report(
        &self,
        inner: &mut ChatInner,
        reference: Option<&str>,
    ) -> Result<ActionReport, String> {
        let browser = inner.browser.clone().ok_or("no page is open")?;
        let count = inner.tabs.len();
        let current = inner.current;
        let tab = inner.tabs.get_mut(current).ok_or("no page is open")?;
        let (document, url, title, notes) = {
            let mut state = tab.state.lock().unwrap_or_else(|e| e.into_inner());
            (
                state.document,
                state.url.clone(),
                state.title.clone(),
                std::mem::take(&mut state.notes),
            )
        };
        if document != tab.seen_document {
            tab.refs.clear();
            tab.names.clear();
            tab.seen_document = document;
        }
        let root = match reference {
            Some(reference) => Some(tab.refs.resolve(reference).ok_or_else(|| {
                format!("no element {reference} on this page; take a new browser_snapshot")
            })?),
            None => None,
        };
        let cdp = &browser.cdp;
        let tree = cdp
            .call(
                Some(&tab.session_id),
                "Accessibility.getFullAXTree",
                json!({}),
            )
            .await?;
        let nodes = parse_ax_nodes(&tree);
        let passwords = password_fields(cdp, &tab.session_id, &nodes).await;
        let Outline {
            text,
            truncated,
            omitted_lines,
        } = render(
            &nodes,
            &mut tab.refs,
            &RenderOptions {
                passwords,
                root,
                limit: 0,
            },
        );
        for node in &nodes {
            if let Some(backend) = node.backend_id {
                if !node.name.is_empty() && tab.refs.of(backend).is_some() {
                    tab.names.insert(backend, cap(&node.name, 60));
                }
            }
        }
        // The tab's own title briefly holds the URL while a page loads.
        let title = cdp
            .call(
                Some(&tab.session_id),
                "Runtime.evaluate",
                json!({"expression": "document.title", "returnByValue": true}),
            )
            .await
            .ok()
            .and_then(|value| value["result"]["value"].as_str().map(|t| cap(t, 200)))
            .unwrap_or(title);
        Ok(ActionReport {
            url,
            title,
            tab: (count > 1).then(|| format!("{} of {count}", current + 1)),
            snapshot: if text.is_empty() { "(empty page)".to_owned() } else { text },
            new_since_last_action: notes,
            snapshot_note: truncated.then(|| {
                format!("{omitted_lines} more lines not shown; use browser_snapshot with a ref to read one part, or browser_scroll")
            }),
        })
    }

    /// Streams the current tab to the Workspace view at the view's size.
    pub async fn start_screencast(
        &self,
        width: u32,
        height: u32,
        scale: f64,
    ) -> Result<(), String> {
        self.ui_stream.store(true, Ordering::SeqCst);
        *self.preview_width.lock().unwrap_or_else(|e| e.into_inner()) = None;
        self.thaw_pages().await;
        self.run_screencast(width, height, scale).await
    }

    /// Streams small frames for the chat's preview without resizing the page:
    /// the agent keeps the viewport it had, only the frames are scaled down.
    pub async fn start_preview(&self, max_width: u32) -> Result<(), String> {
        self.ui_stream.store(true, Ordering::SeqCst);
        *self.preview_width.lock().unwrap_or_else(|e| e.into_inner()) =
            Some(max_width.clamp(160, 1280));
        let viewport = *self.viewport.lock().unwrap_or_else(|e| e.into_inner());
        self.run_screencast(viewport.width, viewport.height, viewport.scale)
            .await
    }

    /// Starts (or moves) the protocol screencast to the current tab.
    async fn run_screencast(&self, width: u32, height: u32, scale: f64) -> Result<(), String> {
        {
            let mut viewport = self.viewport.lock().unwrap_or_else(|e| e.into_inner());
            *viewport = Viewport {
                width: width.clamp(320, 3840),
                height: height.clamp(240, 2400),
                scale: if scale.is_finite() {
                    scale.clamp(1.0, 3.0)
                } else {
                    1.0
                },
            };
        }
        let _one = self.stream_start.lock().await;
        let Some((cdp, session_id)) = self.view().current() else {
            *self.screencast.lock().unwrap_or_else(|e| e.into_inner()) = Some(String::new());
            return Ok(());
        };
        let viewport = *self.viewport.lock().unwrap_or_else(|e| e.into_inner());
        let _ = cdp
            .call(
                Some(&session_id),
                "Emulation.setDeviceMetricsOverride",
                json!({"width": viewport.width, "height": viewport.height, "deviceScaleFactor": viewport.scale, "mobile": false}),
            )
            .await;
        let previous = self
            .screencast
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .replace(session_id.clone());
        // Chrome refuses a second start on a streaming tab, and a restart is
        // what applies a new size, so stop whichever tab is streaming first.
        if let Some(previous) = previous.filter(|p| !p.is_empty()) {
            let _ = cdp
                .call(Some(&previous), "Page.stopScreencast", json!({}))
                .await;
        }
        STREAM_STARTS.fetch_add(1, Ordering::Relaxed);
        let (max_width, max_height) = frame_size(
            viewport,
            *self.preview_width.lock().unwrap_or_else(|e| e.into_inner()),
        );
        cdp.call(
            Some(&session_id),
            "Page.startScreencast",
            json!({"format": "jpeg", "quality": 85, "maxWidth": max_width, "maxHeight": max_height, "everyNthFrame": 1}),
        )
        .await?;
        Ok(())
    }

    /// Stops streaming (the Browser tab was closed or hidden). A recording
    /// keeps the protocol screencast running.
    pub async fn stop_screencast(&self) {
        self.ui_stream.store(false, Ordering::SeqCst);
        if self
            .recording
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some()
        {
            return;
        }
        self.end_screencast().await;
        // Nobody is looking; an agent's next action brings the pages back.
        self.freeze_pages().await;
    }

    async fn end_screencast(&self) {
        let session = self
            .screencast
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        let inner = self.inner.lock().await;
        if let (Some(session), Some(browser)) = (session.filter(|s| !s.is_empty()), &inner.browser)
        {
            STREAM_STOPS.fetch_add(1, Ordering::Relaxed);
            let _ = browser
                .cdp
                .call(Some(&session), "Page.stopScreencast", json!({}))
                .await;
        }
    }

    async fn restart_screencast(&self) {
        let streaming = self
            .screencast
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some();
        if streaming {
            let viewport = *self.viewport.lock().unwrap_or_else(|e| e.into_inner());
            let _ = self
                .run_screencast(viewport.width, viewport.height, viewport.scale)
                .await;
        }
    }

    /// Adds a screencast frame (base64 JPEG) to a recording in progress.
    fn record_frame(&self, data: &str) {
        use base64::Engine;
        let mut recording = self.recording.lock().unwrap_or_else(|e| e.into_inner());
        let Some(recording) = recording.as_mut() else {
            return;
        };
        if let Ok(jpeg) = base64::engine::general_purpose::STANDARD.decode(data) {
            recording.push(jpeg);
        }
    }

    /// Starts recording the current tab for up to `max`.
    pub async fn start_recording(&self, by: Recorder, max: Duration) -> Result<(), String> {
        {
            let mut recording = self.recording.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(current) = recording.as_ref() {
                let seconds = current.elapsed().as_secs();
                return Err(format!(
                    "already recording since {}:{:02}; stop it first",
                    seconds / 60,
                    seconds % 60
                ));
            }
            if self
                .inner
                .try_lock()
                .is_ok_and(|inner| inner.tabs.is_empty())
            {
                return Err("no page is open to record; use browser_open first".into());
            }
            *recording = Some(Recording::new(by, max));
        }
        let viewport = *self.viewport.lock().unwrap_or_else(|e| e.into_inner());
        if let Err(error) = self
            .run_screencast(viewport.width, viewport.height, viewport.scale)
            .await
        {
            *self.recording.lock().unwrap_or_else(|e| e.into_inner()) = None;
            return Err(error);
        }
        self.publish_state().await;
        Ok(())
    }

    /// Who is recording, if anyone.
    pub fn recorder(&self) -> Option<Recorder> {
        self.recording
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map(|recording| recording.by)
    }

    /// Stops the recording and returns it as an MP4 with its length. The agent
    /// may stop only its own recordings (`by` is who asks).
    pub async fn stop_recording(&self, by: Recorder) -> Result<(Vec<u8>, Duration), String> {
        let recording = {
            let mut recording = self.recording.lock().unwrap_or_else(|e| e.into_inner());
            match recording.as_ref() {
                None => return Err("nothing is being recorded".into()),
                Some(current) if by == Recorder::Agent && current.by == Recorder::User => {
                    return Err("the user started this recording; only they can stop it".into());
                }
                Some(_) => recording.take().expect("checked above"),
            }
        };
        if !self.ui_stream.load(Ordering::SeqCst) {
            self.end_screencast().await;
        }
        self.publish_state().await;
        let length = recording.elapsed();
        let frames = recording.frames;
        let css = {
            let viewport = self.viewport.lock().unwrap_or_else(|e| e.into_inner());
            (viewport.width as usize, viewport.height as usize)
        };
        if frames.is_empty() {
            return Err(
                "the page did not paint while recording, so there is nothing to save".into(),
            );
        }
        // Dropping this future (the run was stopped) cancels the encode, which
        // otherwise keeps a core busy to the end.
        struct CancelOnDrop(Arc<AtomicBool>);
        impl Drop for CancelOnDrop {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Relaxed);
            }
        }
        let cancel = CancelOnDrop(Arc::new(AtomicBool::new(false)));
        let cancelled = cancel.0.clone();
        let video = tokio::task::spawn_blocking(move || {
            super::media::encode_mp4(&frames, length, css, &cancelled)
        })
        .await
        .map_err(|e| e.to_string())??;
        drop(cancel);
        Ok((video, length))
    }

    /// A PNG of the current page (the visible part, the whole page, or one
    /// element) with its size in CSS pixels.
    pub async fn screenshot(
        &self,
        reference: Option<&str>,
        full_page: bool,
    ) -> Result<(Vec<u8>, u32, u32), String> {
        use base64::Engine;
        let inner = self.inner.lock().await;
        let browser = inner
            .browser
            .clone()
            .ok_or("no page is open; use browser_open first")?;
        let tab = inner
            .tabs
            .get(inner.current)
            .ok_or("no page is open; use browser_open first")?;
        let cdp = &browser.cdp;
        let session = tab.session_id.clone();
        let metrics = cdp
            .call(Some(&session), "Page.getLayoutMetrics", json!({}))
            .await?;
        let view = &metrics["cssVisualViewport"];
        let (page_x, page_y) = (
            view["pageX"].as_f64().unwrap_or(0.0),
            view["pageY"].as_f64().unwrap_or(0.0),
        );
        let mut params = json!({"format": "png", "fromSurface": true});
        // The Workspace view emulates its screen's device scale; dividing it
        // out of the clip gives the model CSS pixels.
        let scale = 1.0
            / self
                .viewport
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .scale;
        let (width, height) = match reference {
            Some(reference) => {
                let backend = tab.refs.resolve(reference).ok_or_else(|| {
                    format!("no element {reference} on this page; take a new browser_snapshot")
                })?;
                let _ = cdp
                    .call(
                        Some(&session),
                        "DOM.scrollIntoViewIfNeeded",
                        json!({"backendNodeId": backend}),
                    )
                    .await;
                let model = cdp
                    .call(
                        Some(&session),
                        "DOM.getBoxModel",
                        json!({"backendNodeId": backend}),
                    )
                    .await
                    .map_err(|_| "that element is not visible on the page".to_owned())?;
                let quad: Vec<f64> = model["model"]["border"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_f64)
                    .collect();
                if quad.len() != 8 {
                    return Err("that element has no size on the page".into());
                }
                let xs = [quad[0], quad[2], quad[4], quad[6]];
                let ys = [quad[1], quad[3], quad[5], quad[7]];
                let (x, y) = (
                    xs.iter().copied().fold(f64::MAX, f64::min),
                    ys.iter().copied().fold(f64::MAX, f64::min),
                );
                let w = xs.iter().copied().fold(f64::MIN, f64::max) - x;
                let h = ys.iter().copied().fold(f64::MIN, f64::max) - y;
                // After scrolling into view the box is in viewport coordinates.
                let metrics = cdp
                    .call(Some(&session), "Page.getLayoutMetrics", json!({}))
                    .await?;
                let view = &metrics["cssVisualViewport"];
                let (scroll_x, scroll_y) = (
                    view["pageX"].as_f64().unwrap_or(page_x),
                    view["pageY"].as_f64().unwrap_or(page_y),
                );
                params["clip"] = json!({"x": x + scroll_x, "y": y + scroll_y, "width": w.max(1.0), "height": h.max(1.0), "scale": scale});
                params["captureBeyondViewport"] = json!(true);
                (w, h)
            }
            None if full_page => {
                let content = &metrics["cssContentSize"];
                let w = content["width"].as_f64().unwrap_or(1280.0);
                let h = content["height"].as_f64().unwrap_or(800.0).min(16_384.0);
                params["clip"] = json!({"x": 0, "y": 0, "width": w, "height": h, "scale": scale});
                params["captureBeyondViewport"] = json!(true);
                (w, h)
            }
            None => {
                let w = view["clientWidth"].as_f64().unwrap_or(1280.0);
                let h = view["clientHeight"].as_f64().unwrap_or(800.0);
                params["clip"] =
                    json!({"x": page_x, "y": page_y, "width": w, "height": h, "scale": scale});
                (w, h)
            }
        };
        let shot = cdp
            .call_with_timeout(
                Some(&session),
                "Page.captureScreenshot",
                params,
                NAVIGATION_WAIT,
            )
            .await?;
        let png = base64::engine::general_purpose::STANDARD
            .decode(shot["data"].as_str().unwrap_or_default())
            .map_err(|_| "the browser returned an unreadable screenshot".to_owned())?;
        Ok((png, width.round() as u32, height.round() as u32))
    }

    /// The person's mouse, wheel or key input on the Workspace view. The
    /// person and the agent use the browser at the same time; neither waits.
    pub async fn user_input(&self, input: &UserInput) -> Result<(), String> {
        self.person_acted();
        self.thaw_pages().await;
        let Some((cdp, session_id)) = self.view().current() else {
            return Ok(());
        };
        let session = Some(session_id.as_str());
        let cdp = &cdp;
        match input {
            UserInput::Mouse {
                kind,
                x,
                y,
                button,
                clicks,
                modifiers,
                delta_x,
                delta_y,
            } => {
                let kind = match kind.as_str() {
                    "down" => "mousePressed",
                    "up" => "mouseReleased",
                    "move" => "mouseMoved",
                    "wheel" => "mouseWheel",
                    _ => return Err("unknown mouse event".into()),
                };
                let button = match button.as_str() {
                    "left" | "right" | "middle" => button.as_str(),
                    _ => "none",
                };
                cdp.send(
                    session,
                    "Input.dispatchMouseEvent",
                    json!({"type": kind, "x": x, "y": y, "button": button, "clickCount": clicks.min(&3), "modifiers": modifiers & 15, "deltaX": delta_x, "deltaY": delta_y}),
                );
            }
            UserInput::Key {
                kind,
                key,
                code,
                text,
                key_code,
                modifiers,
            } => {
                let kind = match kind.as_str() {
                    "down" if text.is_some() => "keyDown",
                    "down" => "rawKeyDown",
                    "up" => "keyUp",
                    _ => return Err("unknown key event".into()),
                };
                let mut params = json!({"type": kind, "key": cap(key, 20), "code": cap(code, 30), "windowsVirtualKeyCode": key_code, "modifiers": modifiers & 15});
                if let Some(text) = text.as_deref().filter(|_| kind == "keyDown") {
                    params["text"] = json!(cap(text, 8));
                }
                cdp.send(session, "Input.dispatchKeyEvent", params);
            }
            UserInput::Text { text } => {
                cdp.send(
                    session,
                    "Input.insertText",
                    json!({"text": text.chars().take(10_000).collect::<String>()}),
                );
            }
        }
        Ok(())
    }
}

/// Input from the Workspace view, in page CSS pixels.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UserInput {
    Mouse {
        kind: String,
        x: f64,
        y: f64,
        #[serde(default)]
        button: String,
        #[serde(default)]
        clicks: u32,
        #[serde(default)]
        modifiers: u32,
        #[serde(default)]
        delta_x: f64,
        #[serde(default)]
        delta_y: f64,
    },
    Key {
        kind: String,
        key: String,
        #[serde(default)]
        code: String,
        #[serde(default)]
        text: Option<String>,
        #[serde(default)]
        key_code: u32,
        #[serde(default)]
        modifiers: u32,
    },
    Text {
        text: String,
    },
}

/// The host of a URL for approvals; `None` for pages without one (about:blank).
pub fn site_of(url: &str) -> Option<String> {
    reqwest::Url::parse(url)
        .ok()?
        .host_str()
        .filter(|host| !host.is_empty())
        .map(str::to_ascii_lowercase)
}

const SELECT_ALL_JS: &str = "function() { this.focus(); if (typeof this.select === 'function') { this.select(); } else { const range = document.createRange(); range.selectNodeContents(this); const selection = window.getSelection(); selection.removeAllRanges(); selection.addRange(range); } }";

const SELECT_JS: &str = "function(values) { if (this.tagName !== 'SELECT') { throw new Error('element is ' + this.tagName.toLowerCase()); } const wanted = new Set(values); let found = 0; for (const option of this.options) { const match = wanted.has(option.value) || wanted.has(option.label) || wanted.has(option.textContent.trim()); option.selected = match && (this.multiple || found === 0); if (match) { found += 1; } } this.dispatchEvent(new Event('input', { bubbles: true })); this.dispatchEvent(new Event('change', { bubbles: true })); return found; }";

async fn history_step(cdp: &Cdp, session: &str, action: &str) -> Result<(), String> {
    match action.trim().to_ascii_lowercase().as_str() {
        "reload" => {
            cdp.call(Some(session), "Page.reload", json!({})).await?;
        }
        step @ ("back" | "forward") => {
            let history = cdp
                .call(Some(session), "Page.getNavigationHistory", json!({}))
                .await?;
            let index = history["currentIndex"].as_i64().unwrap_or(0);
            let target = if step == "back" { index - 1 } else { index + 1 };
            let entry = usize::try_from(target)
                .ok()
                .and_then(|target| history["entries"].as_array()?.get(target).cloned())
                .ok_or_else(|| format!("there is no page to go {step} to"))?;
            cdp.call(
                Some(session),
                "Page.navigateToHistoryEntry",
                json!({"entryId": entry["id"]}),
            )
            .await?;
        }
        other => {
            return Err(format!(
                "action must be back, forward or reload, not '{}'",
                cap(other, 20)
            ))
        }
    }
    Ok(())
}

async fn resolve_object(cdp: &Cdp, session: &str, backend: i64) -> Result<String, String> {
    let resolved = cdp
        .call(
            Some(session),
            "DOM.resolveNode",
            json!({"backendNodeId": backend}),
        )
        .await
        .map_err(|_| {
            "that element is gone from the page; take a new browser_snapshot".to_owned()
        })?;
    resolved["object"]["objectId"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| "that element is gone from the page; take a new browser_snapshot".to_owned())
}

/// Scrolls an element into view and returns its center in CSS pixels.
async fn element_center(cdp: &Cdp, session: &str, backend: i64) -> Result<(f64, f64), String> {
    let _ = cdp
        .call(
            Some(session),
            "DOM.scrollIntoViewIfNeeded",
            json!({"backendNodeId": backend}),
        )
        .await;
    let quads = cdp
        .call(
            Some(session),
            "DOM.getContentQuads",
            json!({"backendNodeId": backend}),
        )
        .await
        .map_err(|_| {
            "that element is gone or not visible; take a new browser_snapshot".to_owned()
        })?;
    let quad: Vec<f64> = quads["quads"][0]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_f64)
        .collect();
    if quad.len() != 8 {
        return Err("that element is not visible on the page (no size); scroll or open what contains it first".into());
    }
    let x = (quad[0] + quad[2] + quad[4] + quad[6]) / 4.0;
    let y = (quad[1] + quad[3] + quad[5] + quad[7]) / 4.0;
    Ok((x, y))
}

async fn click_at(cdp: &Cdp, session: &str, x: f64, y: f64, clicks: u32) -> Result<(), String> {
    cdp.call(
        Some(session),
        "Input.dispatchMouseEvent",
        json!({"type": "mouseMoved", "x": x, "y": y}),
    )
    .await?;
    for count in 1..=clicks {
        for kind in ["mousePressed", "mouseReleased"] {
            cdp.call(
                Some(session),
                "Input.dispatchMouseEvent",
                json!({"type": kind, "x": x, "y": y, "button": "left", "buttons": 1, "clickCount": count}),
            )
            .await?;
        }
    }
    Ok(())
}

async fn press_key(cdp: &Cdp, session: &str, press: &KeyPress) -> Result<(), String> {
    let mut down = json!({
        "type": if press.text.is_some() { "keyDown" } else { "rawKeyDown" },
        "key": press.key, "code": press.code,
        "windowsVirtualKeyCode": press.key_code, "nativeVirtualKeyCode": press.key_code,
        "modifiers": press.modifiers,
    });
    if let Some(text) = &press.text {
        down["text"] = json!(text);
        down["unmodifiedText"] = json!(text);
    }
    cdp.call(Some(session), "Input.dispatchKeyEvent", down)
        .await?;
    cdp.call(
        Some(session),
        "Input.dispatchKeyEvent",
        json!({"type": "keyUp", "key": press.key, "code": press.code, "windowsVirtualKeyCode": press.key_code, "modifiers": press.modifiers}),
    )
    .await?;
    Ok(())
}

/// DOM nodes of `<input type=password>` among the snapshot's text fields.
async fn password_fields(
    cdp: &Cdp,
    session: &str,
    nodes: &[super::snapshot::AxNode],
) -> HashSet<i64> {
    let fields: Vec<i64> = nodes
        .iter()
        .filter(|node| matches!(node.role.as_str(), "textbox" | "textfield" | "searchbox"))
        .filter_map(|node| node.backend_id)
        .take(64)
        .collect();
    let mut passwords = HashSet::new();
    for backend in fields {
        let described = cdp
            .call_with_timeout(
                Some(session),
                "DOM.describeNode",
                json!({"backendNodeId": backend}),
                Duration::from_secs(5),
            )
            .await;
        // When a field cannot be described, mask it: never risk showing a password.
        let password = described.map_or(true, |node| {
            let attributes: Vec<&str> = node["node"]["attributes"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            attributes.chunks(2).any(|pair| {
                pair.len() == 2
                    && pair[0].eq_ignore_ascii_case("type")
                    && pair[1].eq_ignore_ascii_case("password")
            })
        });
        if password {
            passwords.insert(backend);
        }
    }
    passwords
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_preview_scales_frames_down_and_keeps_the_page_size() {
        let page = super::Viewport {
            width: 1280,
            height: 800,
            scale: 2.0,
        };
        assert_eq!(super::frame_size(page, None), (2560, 1600));
        assert_eq!(super::frame_size(page, Some(440)), (440, 275));
    }

    use super::*;

    #[test]
    fn keys_parse_with_modifiers_and_typing_text() {
        let enter = parse_key("Enter").unwrap();
        assert_eq!(
            (enter.key.as_str(), enter.key_code, enter.text.as_deref()),
            ("Enter", 13, Some("\r"))
        );
        let select_all = parse_key("Control+a").unwrap();
        assert_eq!(
            (
                select_all.code.as_str(),
                select_all.modifiers,
                select_all.text
            ),
            ("KeyA", 2, None)
        );
        let shifted = parse_key("Shift+k").unwrap();
        assert_eq!(
            (
                shifted.key.as_str(),
                shifted.text.as_deref(),
                shifted.modifiers
            ),
            ("K", Some("K"), 8)
        );
        assert_eq!(parse_key("Meta+Shift+ArrowDown").unwrap().modifiers, 12);
        assert_eq!(parse_key("F5").unwrap().key_code, 116);
        assert_eq!(parse_key("Control++").unwrap().key, "+");
        for bad in ["", "Hyper+a", "NotAKey", "F13", "Control+"] {
            assert!(parse_key(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn sites_and_short_urls_drop_what_approvals_and_notes_must_not_carry() {
        assert_eq!(
            site_of("https://Login.Example.com:8443/a?b=c").as_deref(),
            Some("login.example.com")
        );
        assert_eq!(site_of("about:blank"), None);
        assert_eq!(site_of("not a url"), None);
        assert_eq!(
            short_url("https://x.example/a/b?token=secret#frag"),
            "https://x.example/a/b"
        );
    }

    #[test]
    fn dialogs_errors_and_failures_become_notes_and_navigation_resets_refs() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        drop(tx);
        let state = Mutex::new(TabState::default());
        let note = |method: &str, params: Value| CdpEvent {
            method: method.into(),
            params,
            session: Some("S".into()),
        };
        let events = [
            note(
                "Runtime.exceptionThrown",
                json!({"exceptionDetails": {"exception": {"description": "TypeError: x is undefined\n    at f (a.js:1)"}}}),
            ),
            note(
                "Network.responseReceived",
                json!({"type": "Fetch", "response": {"status": 422, "url": "http://localhost:5173/api/signup?token=abc"}}),
            ),
            note(
                "Network.loadingFailed",
                json!({"type": "Image", "errorText": "net::ERR_FAILED"}),
            ),
            note(
                "Network.loadingFailed",
                json!({"type": "XHR", "errorText": "net::ERR_CONNECTION_REFUSED"}),
            ),
            note(
                "Page.frameNavigated",
                json!({"frame": {"id": "T", "url": "http://localhost:5173/next"}}),
            ),
        ];
        // Dialog handling needs a connection; everything else is pure state.
        for event in &events {
            apply_without_connection(&state, event);
        }
        let state = state.into_inner().unwrap();
        assert_eq!(
            state.notes,
            [
                "uncaught TypeError: x is undefined",
                "HTTP 422 for http://localhost:5173/api/signup",
                "request failed: net::ERR_CONNECTION_REFUSED",
            ]
        );
        assert_eq!(state.url, "http://localhost:5173/next");
        assert_eq!(state.document, 1);
        assert_eq!(
            state.console_errors, 0,
            "a new document starts with no errors"
        );
    }

    /// [`handle_tab_event`] for events that never use the connection.
    fn apply_without_connection(state: &Mutex<TabState>, event: &CdpEvent) {
        assert_ne!(event.method, "Page.javascriptDialogOpening");
        // A closed connection: `send` is a no-op for these events anyway.
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let accept = tokio::spawn(async move {
                let (stream, _) = listener.accept().await.unwrap();
                tokio_tungstenite::accept_async(stream).await.unwrap()
            });
            let cdp = Cdp::connect(&format!("ws://{address}/devtools/browser/x"))
                .await
                .unwrap();
            let _server = accept.await.unwrap();
            handle_tab_event(&cdp, "T", "S", state, event);
        });
    }
}

mod devtools;
mod downloads;
pub use devtools::{ConsoleEntry, RequestEntry};
pub use downloads::{default_downloads_directory, DownloadGate, DownloadPolicy};
