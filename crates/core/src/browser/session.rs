//! Browser sessions: one Chromium per project profile, shared by that
//! project's chats; each chat owns one page.
//!
//! The agent's tools and the person's Workspace view drive the same page. When
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

/// Longest wait for a page to finish loading after a navigation.
/// Chats whose page stays open at once, app-wide. Opening one more closes the page of the
/// chat used least recently (one nobody is watching, recording or acting in); that chat
/// reopens it at the same address when it next browses. Each open page costs a renderer
/// process and its memory, even frozen.
pub const OPEN_PAGES: usize = 3;
/// After this long with no chat using it, the browser exits (hundreds of MB and a GPU process
/// otherwise stay resident); the next action starts it again and pages reopen at their address.
pub const IDLE_EXIT: Duration = Duration::from_secs(10 * 60);
/// Most time Chrome gets to say where a new window was going before it is closed unloaded.
const POPUP_WAIT: Duration = Duration::from_secs(3);
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
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct BrowserState {
    pub open: bool,
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
    /// Where the agent last left the mouse, in the page's CSS pixels.
    pub pointer: Option<(f64, f64)>,
    /// The page holds the pointer lock: the view then locks the person's mouse to it on a
    /// click and sends its movement as relative moves.
    pub pointer_locked: bool,
}

/// A recording in progress, for the Browser tab and the thread header.
#[derive(Debug, Clone, Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct RecordingInfo {
    pub by: Recorder,
    pub seconds: u64,
    pub max_seconds: u64,
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
    /// `log_seq` when the current document committed: later entries belong to this page.
    document_seq: u64,
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

impl Tab {
    /// Refs name nodes of one document; a new one starts them again.
    fn forget_refs_of_old_page(&mut self, document: u64) {
        if document != self.seen_document {
            self.refs.clear();
            self.names.clear();
            self.seen_document = document;
        }
    }
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

#[derive(Default)]
struct ChatInner {
    browser: Option<Arc<SharedBrowser>>,
    /// The chat's one page; links and pages that open a new window load here instead.
    tab: Option<Tab>,
}

/// The chat's browser connection and page as of their last change. The person's input
/// and the Workspace view read this, never `inner`: an agent action holds `inner` while
/// its page settles (seconds on busy sites), and neither side waits for the other.
#[derive(Clone, Default)]
struct View {
    cdp: Option<Arc<Cdp>>,
    /// `127.0.0.1:<port>` of the browser's debugging server (for DevTools).
    http_address: Option<String>,
    tab: Option<ViewTab>,
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
        let tab = self.tab.as_ref()?;
        Some((Arc::clone(self.cdp.as_ref()?), tab.session_id.clone()))
    }
}

#[derive(Debug, Clone, Copy)]
struct Viewport {
    width: u32,
    height: u32,
    scale: f64,
}

/// Device pixels per CSS pixel the page is drawn at: the view's own, from 1 to 3.
fn clamp_scale(scale: f64) -> f64 {
    if scale.is_finite() {
        scale.clamp(1.0, 3.0)
    } else {
        1.0
    }
}

/// The largest Browser tab frame, in device pixels: a 4K screen's worth.
const MAX_FRAME: (u32, u32) = (3840, 2400);

/// Where the screencast's frames go, which decides their size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FrameTarget {
    /// The Browser tab, whose page area is this many device pixels.
    Panel(u32, u32),
    /// The chat's small preview, frames this wide.
    Preview(u32),
}

/// The largest frame the screencast sends. Chrome scales a frame down to fit both
/// limits and keeps its shape. The Browser tab gets at most its page area in device
/// pixels (a page laid out larger than the panel comes at the panel's density, not
/// its own), the preview `preview` wide, and a recording the page's full size: it
/// is encoded from these frames at their whole-number ratio to the page.
fn frame_size(viewport: Viewport, target: Option<FrameTarget>, recording: bool) -> (u32, u32) {
    let full = (
        (f64::from(viewport.width) * viewport.scale) as u32,
        (f64::from(viewport.height) * viewport.scale) as u32,
    );
    match target {
        _ if recording => full,
        Some(FrameTarget::Preview(width)) => (
            width,
            (u64::from(width) * u64::from(viewport.height) / u64::from(viewport.width.max(1)))
                as u32,
        ),
        Some(FrameTarget::Panel(width, height)) => (
            full.0.min(width.clamp(1, MAX_FRAME.0)),
            full.1.min(height.clamp(1, MAX_FRAME.1)),
        ),
        None => full,
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
    /// Where the mouse last went on the visible page, for moves relative to it.
    pointer: Mutex<Option<(f64, f64)>>,
    /// The page held the pointer lock when last checked (after the person's clicks and keys).
    page_locked: AtomicBool,
    /// The mouse button held down by the `press` action.
    held: Mutex<Option<HeldMouse>>,
    /// Session id of the tab being streamed to the Workspace view.
    screencast: Mutex<Option<String>>,
    /// The Workspace view is showing the stream (frames go to it).
    ui_stream: AtomicBool,
    /// Who the stream's frames are for: the Browser tab (frames at its pixel density) or
    /// the chat's small preview (small frames; the page left at its own size).
    frame_target: Mutex<Option<FrameTarget>>,
    recording: Mutex<Option<Recording>>,
    viewport: Mutex<Viewport>,
    /// The Workspace speaker is off (the default): pages play no sound.
    muted: AtomicBool,
    /// Downloads of this chat's page, newest last.
    downloads: Mutex<Vec<downloads::Download>>,
    /// Until when a download that starts follows the person's own input (theirs).
    person_until: Mutex<Option<Instant>>,
    /// The current run's download folder and permission.
    download_policy: Mutex<Option<downloads::DownloadPolicy>>,
    /// Target ids of this chat's page, readable while an action holds `inner`.
    targets: Mutex<HashSet<String>>,
    /// See [`View`].
    view: Mutex<View>,
    /// One screencast start at a time (Chrome refuses a second start on a tab).
    stream_start: tokio::sync::Mutex<()>,
    /// The address of the page closed to make room (see [`OPEN_PAGES`]), reopened on next use.
    reopen: Mutex<Option<String>>,
    /// When an agent action or the person last used the page; the oldest page closes first.
    last_used: Mutex<Instant>,
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
    pub snapshot: String,
    /// The size x and y are measured against, as `1280x720`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visible_page: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub new_since_last_action: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot_note: Option<String>,
    /// What the mouse is over after moving: the agent has no pointer to look at.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub under_mouse: Option<String>,
    /// What a drag pressed on, whether anything was dragged, and where it let go.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub drag: Option<String>,
    /// The page's structure, when asked for: containers, positions and selectors.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub layout: Option<String>,
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

impl ChatBrowser {
    /// Stops the chat's pages running while nothing uses them: a 3D page left open takes
    /// several cores here, since this browser draws on the CPU. Screenshots and scripts still
    /// work on a frozen page. Not while the person watches the Browser tab or a recording
    /// runs. The next action, input or view brings them back ([`Self::thaw_pages`]).
    /// Returns whether this froze them.
    pub async fn freeze_pages(&self) -> bool {
        let watching = self.ui_stream.load(Ordering::SeqCst)
            && !matches!(
                *self.frame_target.lock().unwrap_or_else(|e| e.into_inner()),
                Some(FrameTarget::Preview(_))
            );
        let recording = self
            .recording
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some();
        if watching || recording || self.frozen.swap(true, Ordering::SeqCst) {
            return false;
        }
        self.set_page_state("frozen").await;
        true
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
        let freezing = state == "frozen";
        if let Some(tab) = &view.tab {
            let session = Some(tab.session_id.as_str());
            // Focus emulation decides both ways (tested against the real engine): left on, a page
            // ignores the freeze; left off, a thawed page stays hidden and never draws again.
            if freezing {
                let _ = cdp
                    .call(
                        session,
                        "Emulation.setFocusEmulationEnabled",
                        json!({"enabled": false}),
                    )
                    .await;
            }
            let _ = cdp
                .call(
                    session,
                    "Page.setWebLifecycleState",
                    json!({"state": state}),
                )
                .await;
            if !freezing {
                let _ = cdp
                    .call(
                        session,
                        "Emulation.setFocusEmulationEnabled",
                        json!({"enabled": true}),
                    )
                    .await;
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
        if let Some(tab) = &mut inner.tab {
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

    /// Records the browser and page for [`View`] readers; called after every change.
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
            tab: inner.tab.as_ref().map(|tab| ViewTab {
                target_id: tab.target_id.clone(),
                session_id: tab.session_id.clone(),
                state: Arc::clone(&tab.state),
            }),
        };
    }

    fn view(&self) -> View {
        self.view.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    async fn state_of(&self) -> BrowserState {
        let view = self.view();
        let current = view.tab.as_ref();
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
            open: view.tab.is_some(),
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
            pointer: *self.pointer.lock().unwrap_or_else(|e| e.into_inner()),
            pointer_locked: self.page_locked.load(Ordering::SeqCst),
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

    /// Moves the agent's pointer to `to` the way a hand would: the view is told where it goes
    /// and animates the cursor over [`GLIDE`], while the page gets the moves along an eased
    /// path. From nowhere (no move yet) it goes straight there.
    async fn glide(&self, cdp: &Cdp, session: &str, to: (f64, f64)) -> Result<(), String> {
        let from = self
            .pointer
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .replace(to);
        let path = from.map_or_else(Vec::new, |from| glide_path(from, to));
        if path.is_empty() {
            return mouse(
                cdp,
                session,
                json!({"type": "mouseMoved", "x": to.0, "y": to.1}),
            )
            .await;
        }
        self.publish_state().await;
        let pause = GLIDE / path.len() as u32;
        for (x, y) in path {
            mouse(cdp, session, json!({"type": "mouseMoved", "x": x, "y": y})).await?;
            tokio::time::sleep(pause).await;
        }
        Ok(())
    }

    /// Reads whether the page holds the pointer lock and tells the view when that changed.
    async fn check_lock(&self, cdp: &Cdp, session: &str) {
        let locked = pointer_locked(cdp, session).await;
        if self.page_locked.swap(locked, Ordering::SeqCst) != locked {
            self.publish_state().await;
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
        let tab = inner.tab.as_ref()?;
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
        let tab = view.tab.as_ref()?;
        Some(format!(
            "http://{0}/devtools/devtools_app.html?ws={0}/devtools/page/{1}",
            view.http_address.as_deref()?,
            tab.target_id
        ))
    }
}

/// The host of a URL for approvals; `None` for pages without one (about:blank).
pub fn site_of(url: &str) -> Option<String> {
    reqwest::Url::parse(url)
        .ok()?
        .host_str()
        .filter(|host| !host.is_empty())
        .map(str::to_ascii_lowercase)
}

/// How long the agent's pointer takes to travel to where it acts: the view animates the
/// cursor over the same time, and the page gets the moves on the way.
pub const GLIDE: Duration = Duration::from_millis(240);

#[cfg(test)]
mod tests {
    /// A glide eases in and out along the straight line, ends exactly on the target, takes a
    /// bounded number of steps whatever the distance, and is nothing when already there.
    #[test]
    fn a_glide_eases_to_the_target_in_bounded_steps() {
        assert!(super::glide_path((10.0, 10.0), (11.0, 10.5)).is_empty());
        let path = super::glide_path((0.0, 0.0), (400.0, 0.0));
        assert_eq!(path.len(), 10);
        assert_eq!(*path.last().unwrap(), (400.0, 0.0));
        let steps: Vec<f64> = path.windows(2).map(|pair| pair[1].0 - pair[0].0).collect();
        assert!(steps.iter().all(|step| *step > 0.0), "{path:?}");
        // Slow at both ends, fastest in the middle.
        assert!(
            path[0].0 < steps[4] && steps[steps.len() - 1] < steps[4],
            "{path:?}"
        );
        assert!(path.iter().all(|point| point.1 == 0.0));
        assert_eq!(super::glide_path((0.0, 0.0), (30.0, 0.0)).len(), 4);
        assert_eq!(super::glide_path((0.0, 0.0), (5000.0, 5000.0)).len(), 12);
    }

    #[test]
    fn a_relative_move_reports_its_distance_from_a_start_outside_the_page() {
        const PAGE: (f64, f64) = (1280.0, 800.0);
        let inside = super::relative_path((640.0, 400.0), (100.0, -50.0), PAGE).unwrap();
        assert_eq!(inside.start, None);
        assert_eq!(inside.end, (740.0, 350.0));
        for from in [(0.0, 0.0), (640.0, 400.0), (1279.0, 799.0), (1279.0, 0.0)] {
            for delta in [
                (5000.0, 0.0),
                (-5000.0, 0.0),
                (0.0, 5000.0),
                (0.0, -5000.0),
                (1.0, 0.0),
                (-1.0, 0.0),
                (0.0, 2.0),
                (0.0, -2.0),
                (900.0, 900.0),
                (-1300.0, 20.0),
                (3.0, -2000.0),
            ] {
                let path = super::relative_path(from, delta, PAGE).unwrap();
                let on_page =
                    |(x, y): (f64, f64)| (0.0..PAGE.0).contains(&x) && (0.0..PAGE.1).contains(&y);
                assert!(on_page(path.end), "{from:?} {delta:?} {path:?}");
                match path.start {
                    None => assert!(on_page((from.0 + delta.0, from.1 + delta.1))),
                    Some(start) => {
                        assert!(!on_page(start), "{from:?} {delta:?} {path:?}");
                        assert_eq!(path.end.0 - start.0, delta.0, "{from:?} {delta:?}");
                        assert_eq!(path.end.1 - start.1, delta.1, "{from:?} {delta:?}");
                    }
                }
            }
        }
        assert!(super::relative_path((0.0, 0.0), (f64::NAN, 0.0), PAGE).is_err());
        // The browser counts whole pixels, so half a pixel anywhere would lose one every move.
        let odd = super::relative_path((640.5, 400.5), (2000.0, -3.0), (1281.0, 801.0)).unwrap();
        let start = odd.start.unwrap();
        for value in [start.0, start.1, odd.end.0, odd.end.1] {
            assert_eq!(value, value.round(), "{odd:?}");
        }
        assert_eq!(odd.end.0 - start.0, 2000.0);
    }

    #[test]
    fn a_preview_scales_frames_down_and_keeps_the_page_size() {
        let page = super::Viewport {
            width: 1280,
            height: 800,
            scale: 2.0,
        };
        assert_eq!(super::frame_size(page, None, false), (2560, 1600));
        let preview = Some(super::FrameTarget::Preview(440));
        assert_eq!(super::frame_size(page, preview, false), (440, 275));
        // A recording needs the page's full size, whoever watches.
        assert_eq!(super::frame_size(page, preview, true), (2560, 1600));
    }

    #[test]
    fn the_browser_tab_gets_frames_at_its_own_pixel_density_never_more() {
        let page = super::Viewport {
            width: 1280,
            height: 800,
            scale: 2.0,
        };
        // A page laid out larger than a 744 × 560 panel on a Retina screen: at most the
        // panel's device pixels (Chrome fits the frame inside both and keeps its shape).
        let panel = Some(super::FrameTarget::Panel(1488, 1120));
        assert_eq!(super::frame_size(page, panel, false), (1488, 1120));
        // A panel larger than the page never asks for more than the page has.
        let wide = Some(super::FrameTarget::Panel(4000, 3000));
        assert_eq!(super::frame_size(page, wide, false), (2560, 1600));
        // Bounded on a huge screen.
        let huge = super::Viewport {
            width: 3840,
            height: 2400,
            scale: 3.0,
        };
        let all = Some(super::FrameTarget::Panel(11_520, 7_200));
        assert_eq!(super::frame_size(huge, all, false), super::MAX_FRAME);
        assert_eq!(super::clamp_scale(f64::NAN), 1.0);
        assert_eq!(super::clamp_scale(0.5), 1.0);
        assert_eq!(super::clamp_scale(4.0), 3.0);
        assert_eq!(super::clamp_scale(1.5), 1.5);
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
        assert_eq!(parse_key("KeyW").unwrap(), parse_key("w").unwrap());
        assert_eq!(parse_key("Digit1").unwrap(), parse_key("1").unwrap());
        assert!(parse_key("Keyboard").is_none());
        assert!(parse_key("Digita").is_none());
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

mod actions;
mod gestures;
mod hub;
mod keys;
mod pages;
mod person;
mod stream;
mod watcher;
pub use gestures::DragEnd;
use gestures::*;
pub use hub::{downloaded_browser, NO_BROWSER};
pub use keys::{parse_key, KeyPress};
pub use person::UserInput;
use watcher::*;
mod devtools;
mod downloads;
mod looks;
pub use devtools::{ConsoleEntry, RequestEntry};
pub use downloads::{default_downloads_directory, DownloadGate, DownloadPolicy};
