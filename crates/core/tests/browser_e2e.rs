//! The built-in browser against a real Chromium and a local test page: the
//! snapshot, refs, typing, clicking, dropdowns, keys, notes, password masking,
//! dialogs, history and tabs. Skipped (with a message) when no Chromium is
//! installed.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use themis_core::browser::{BrowserConfig, BrowserHub};
use themis_core::tools::{Approval, FnHook, ToolAction};
use wiremock::matchers::path;
use wiremock::{Mock, MockServer, ResponseTemplate};

const FORM: &str = r#"<!doctype html><html><head><title>Sign up · Acme</title></head><body>
<h1>Create your account</h1>
<form id="f" onsubmit="event.preventDefault(); submitIt();">
  <label for="email">Email</label><input id="email" type="email">
  <label for="pw">Password</label><input id="pw" type="password" value="hunter2-secret">
  <label for="country">Country</label>
  <select id="country"><option value="">Pick one</option><option value="de">Germany</option><option value="fr">France</option></select>
  <button type="submit">Create account</button>
</form>
<p id="out"></p>
<a href="/second">Next page</a>
<button onclick="if (confirm('Delete everything?')) { document.getElementById('out').textContent = 'deleted'; } else { document.getElementById('out').textContent = 'kept'; }">Danger</button>
<script>
function submitIt() {
  const email = document.getElementById('email').value;
  const country = document.getElementById('country').value;
  document.getElementById('out').textContent = 'Welcome ' + email + ' from ' + country;
  console.error('signup tracker failed');
}
</script></body></html>"#;

/// Saves media into a test folder.
struct TempMedia(std::path::PathBuf);

impl themis_core::browser::media::MediaSink for TempMedia {
    fn save(&self, name: &str, bytes: &[u8]) -> Result<String, String> {
        let path = self.0.join(name);
        std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
        Ok(path.to_string_lossy().into_owned())
    }
}

// One Chrome at a time: on GPU-less CI runners, Chromes started together
// starve each other and an animated page paints only a few times a second.
static ONE_BROWSER: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn hub(root: &std::path::Path) -> Option<Arc<BrowserHub>> {
    let hub = BrowserHub::new(BrowserConfig {
        root: root.to_path_buf(),
        // A chosen engine, for checking one Chrome build; otherwise whatever is installed.
        executable: std::env::var_os("THEMIS_TEST_BROWSER").map(Into::into),
        headless: true,
    });
    if hub.executable().is_none() {
        eprintln!("skipping: no Chromium browser installed");
        return None;
    }
    Some(hub)
}

/// The ref (`e12`) on the line naming `needle`; it follows the name directly.
fn ref_of(snapshot: &str, needle: &str) -> String {
    let line = snapshot
        .lines()
        .find(|line| line.contains(needle) && line.contains("[e"))
        .unwrap_or_else(|| panic!("no line with {needle} in:\n{snapshot}"));
    let after = &line[line.find(needle).unwrap() + needle.len()..];
    let reference = after
        .trim_start()
        .strip_prefix('[')
        .and_then(|rest| rest.split(']').next())
        .filter(|r| r.starts_with('e') && r[1..].chars().all(|c| c.is_ascii_digit()))
        .unwrap_or_else(|| panic!("no ref after {needle}: {line}"));
    reference.to_owned()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_agent_fills_a_form_reads_notes_and_navigates_in_a_real_browser() {
    let _browser = ONE_BROWSER.lock().await;
    let data = tempfile::tempdir().unwrap();
    let Some(hub) = hub(data.path()) else { return };
    let site = MockServer::start().await;
    Mock::given(path("/signup"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(FORM, "text/html; charset=utf-8"))
        .mount(&site)
        .await;
    Mock::given(path("/second"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            "<!doctype html><title>Second</title><h1>Second page</h1>",
            "text/html",
        ))
        .mount(&site)
        .await;
    let project = tempfile::tempdir().unwrap();
    let media_dir = tempfile::tempdir().unwrap();
    let chat = hub.chat("chat-1", project.path());
    let seen = Arc::new(Mutex::new(Vec::<ToolAction>::new()));
    let log = Arc::clone(&seen);
    let approvals = Arc::new(FnHook::new(move |action: &ToolAction| {
        log.lock().unwrap().push(action.clone());
        Approval::AllowOnce
    }));
    let tools = themis_core::browser::tools::browser_tools(
        approvals,
        themis_core::browser::tools::BrowserScope {
            permissions: themis_core::tools::site_permissions::SitePermissions::new(
                Default::default(),
                None,
            ),
            chat: Arc::clone(&chat),
            cancel: None,
            media: Some(Arc::new(TempMedia(media_dir.path().to_path_buf()))),
        },
    );
    let tool = |name: &str| {
        tools
            .iter()
            .find(|tool| tool.name() == name)
            .unwrap_or_else(|| panic!("{name}"))
    };
    let url = format!("{}/signup", site.uri());

    let opened = tool("browser_open")
        .execute(json!({"url": url}))
        .await
        .unwrap();
    let snapshot = opened["snapshot"].as_str().unwrap().to_owned();
    assert_eq!(opened["title"], "Sign up · Acme");
    assert!(
        snapshot.contains("heading \"Create your account\""),
        "{snapshot}"
    );
    assert!(
        !snapshot.contains("hunter2"),
        "password leaked:\n{snapshot}"
    );
    assert_eq!(opened["untrusted"], true);
    // Approvals name the site.
    assert!(seen
        .lock()
        .unwrap()
        .iter()
        .all(|a| a.command == ["127.0.0.1"]));

    let email = ref_of(&snapshot, "\"Email\"");
    let country = ref_of(&snapshot, "\"Country\"");
    let submit = ref_of(&snapshot, "\"Create account\"");
    tool("browser_type")
        .execute(json!({"ref": email, "text": "ada@example.com"}))
        .await
        .unwrap();
    tool("browser_select")
        .execute(json!({"ref": country, "values": ["France"]}))
        .await
        .unwrap();
    // The Workspace view watches; it is streaming, as when the tab is open.
    let mut updates = chat.subscribe();
    chat.start_screencast(800, 600, 1.0).await.unwrap();
    let clicked = tool("browser_click")
        .execute(json!({"ref": submit}))
        .await
        .unwrap();
    let after = clicked["snapshot"].as_str().unwrap();
    assert!(after.contains("Welcome ada@example.com from fr"), "{after}");
    let notes: Vec<String> =
        serde_json::from_value(clicked["new_since_last_action"].clone()).unwrap_or_default();
    assert!(
        notes
            .iter()
            .any(|note| note.contains("signup tracker failed")),
        "{notes:?}"
    );
    // Refs are stable across snapshots of one page.
    assert_eq!(ref_of(after, "\"Create account\""), submit);
    // Once the click is done the view stops saying "Agent is clicking".
    let mut last = None;
    while let Ok(update) = updates.try_recv() {
        if let themis_core::browser::BrowserUpdate::State(state) = update {
            last = Some(state);
        }
    }
    let last = last.expect("the view got state updates");
    assert_eq!(last.activity, None, "{last:?}");
    assert_ne!(last.controller, "agent");
    chat.stop_screencast().await;

    // A confirm dialog is dismissed, never answered yes.
    let danger = ref_of(after, "\"Danger\"");
    let confirmed = tool("browser_click")
        .execute(json!({"ref": danger}))
        .await
        .unwrap();
    assert!(
        confirmed["snapshot"].as_str().unwrap().contains("kept"),
        "{confirmed}"
    );
    let notes = confirmed["new_since_last_action"].to_string();
    assert!(
        notes.contains("confirm dialog") && notes.contains("dismissed"),
        "{notes}"
    );

    // Clearing and typing again, then Enter submits.
    tool("browser_type")
        .execute(json!({"ref": email, "text": "grace@example.com", "clear": true, "submit": true}))
        .await
        .unwrap();
    let state = chat.snapshot(None).await.unwrap();
    assert!(
        state.snapshot.contains("Welcome grace@example.com"),
        "{}",
        state.snapshot
    );

    // Following a link, then back.
    let next = ref_of(&state.snapshot, "\"Next page\"");
    let second = tool("browser_click")
        .execute(json!({"ref": next}))
        .await
        .unwrap();
    assert!(
        second["url"].as_str().unwrap().ends_with("/second"),
        "{second}"
    );
    assert!(second["snapshot"].as_str().unwrap().contains("Second page"));
    let back = tool("browser_navigate")
        .execute(json!({"action": "back"}))
        .await
        .unwrap();
    assert!(back["url"].as_str().unwrap().ends_with("/signup"), "{back}");

    // Keys and a second tab.
    tool("browser_press")
        .execute(json!({"key": "Tab"}))
        .await
        .unwrap();
    let error = tool("browser_press")
        .execute(json!({"key": "Hyper+x"}))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("unknown key"), "{error}");
    let other = tool("browser_open")
        .execute(json!({"url": format!("{}/second", site.uri()), "new_tab": true}))
        .await
        .unwrap();
    assert_eq!(other["tab"], "2 of 2");
    let tabs: Value = tool("browser_tabs")
        .execute(json!({"action": "list"}))
        .await
        .unwrap();
    assert_eq!(tabs["tabs"].as_array().unwrap().len(), 2, "{tabs}");
    tool("browser_tabs")
        .execute(json!({"action": "close", "tab": 2}))
        .await
        .unwrap();
    let state = chat.state().await;
    assert_eq!(state.tabs.len(), 1);
    assert!(state.url.ends_with("/signup"), "{state:?}");

    // A screenshot of the page and of one element, and a short recording.
    let shot = tool("browser_screenshot").execute(json!({})).await.unwrap();
    let png = std::fs::read(shot["path"].as_str().unwrap()).unwrap();
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    assert!(shot["width"].as_u64().unwrap() > 100, "{shot}");
    // The browser renders at Retina scale, but the model gets CSS pixels.
    let png_width = u32::from_be_bytes(png[16..20].try_into().unwrap());
    assert_eq!(u64::from(png_width), shot["width"].as_u64().unwrap());
    let state = chat.snapshot(None).await.unwrap();
    let heading_button = ref_of(&state.snapshot, "\"Create account\"");
    let part = tool("browser_screenshot")
        .execute(json!({"ref": heading_button}))
        .await
        .unwrap();
    assert!(
        part["width"].as_u64().unwrap() < shot["width"].as_u64().unwrap(),
        "{part}"
    );
    tool("browser_record")
        .execute(json!({"action": "start", "max_seconds": 5}))
        .await
        .unwrap();
    let again = tool("browser_record")
        .execute(json!({"action": "start"}))
        .await
        .unwrap_err();
    assert!(again.to_string().contains("already recording"), "{again}");
    tool("browser_type")
        .execute(json!({"ref": email, "text": " typing on camera"}))
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    let video = tool("browser_record")
        .execute(json!({"action": "stop"}))
        .await
        .unwrap();
    let mp4 = std::fs::read(video["path"].as_str().unwrap()).unwrap();
    assert_eq!(&mp4[4..8], b"ftyp", "an MP4 file");
    assert!(mp4.windows(4).any(|w| w == b"avc1"), "with an H.264 track");
    assert!(chat.recorder().is_none());

    // A stale ref explains itself.
    let stale = tool("browser_click")
        .execute(json!({"ref": "e999"}))
        .await
        .unwrap_err();
    assert!(
        stale.to_string().contains("new browser_snapshot"),
        "{stale}"
    );
    hub.close_chat("chat-1").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_workspace_stream_sends_frames_and_the_person_and_agent_act_together() {
    let _browser = ONE_BROWSER.lock().await;
    let data = tempfile::tempdir().unwrap();
    let Some(hub) = hub(data.path()) else { return };
    let project = tempfile::tempdir().unwrap();
    let chat = hub.chat("chat-3", project.path());
    chat.open("data:text/html,<h1>Hello</h1>", false)
        .await
        .unwrap();
    let mut updates = chat.subscribe();
    chat.start_screencast(800, 600, 1.0).await.unwrap();
    let frame = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            if let Ok(themis_core::browser::BrowserUpdate::Frame { data, width, .. }) =
                updates.recv().await
            {
                return (data, width);
            }
        }
    })
    .await
    .expect("a screencast frame");
    assert!(!frame.0.is_empty());
    assert_eq!(frame.1, 800);
    // The view asks again (remount, resize) while the tab is already streaming.
    chat.start_screencast(800, 600, 1.0)
        .await
        .expect("restarting on the same tab");
    chat.start_screencast(640, 480, 2.0)
        .await
        .expect("resizing on the same tab");
    // A Retina view gets frames at device pixels, not blurry CSS pixels.
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        use base64::Engine;
        loop {
            if let Ok(themis_core::browser::BrowserUpdate::Frame { data, .. }) =
                updates.recv().await
            {
                let jpeg = base64::engine::general_purpose::STANDARD
                    .decode(data)
                    .unwrap();
                let mut decoder = zune_jpeg::JpegDecoder::new(std::io::Cursor::new(jpeg));
                decoder.decode_headers().unwrap();
                if decoder.info().unwrap().width == 1280 {
                    return;
                }
            }
        }
    })
    .await
    .expect("a frame at twice the view's width");
    // The model still gets CSS pixels from a Retina view.
    let (png, width, _) = chat.screenshot(None, false).await.unwrap();
    assert_eq!(width, 640);
    assert_eq!(u32::from_be_bytes(png[16..20].try_into().unwrap()), 640);
    // A recording from a Retina view is encoded at CSS size too, so it stays quick.
    chat.start_recording(
        themis_core::browser::media::Recorder::Agent,
        std::time::Duration::from_secs(10),
    )
    .await
    .unwrap();
    chat.open("data:text/html,<h1>Recorded</h1>", false)
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    let (video, _) = chat
        .stop_recording(themis_core::browser::media::Recorder::Agent)
        .await
        .unwrap();
    let avc1 = video
        .windows(4)
        .rposition(|w| w == b"avc1")
        .expect("an H.264 track");
    let width = u16::from_be_bytes([video[avc1 + 28], video[avc1 + 29]]);
    assert_eq!(
        width,
        640,
        "recording width; avc1 at {avc1} of {}",
        video.len()
    );
    chat.user_input(&themis_core::browser::UserInput::Mouse {
        kind: "move".into(),
        x: 10.0,
        y: 10.0,
        button: String::new(),
        clicks: 0,
        modifiers: 0,
        delta_x: 0.0,
        delta_y: 0.0,
    })
    .await
    .unwrap();
    // The person's input never takes the browser from the agent.
    assert_eq!(chat.state().await.controller, "idle");
    tokio::time::timeout(std::time::Duration::from_secs(5), chat.snapshot(None))
        .await
        .expect("the agent acts right after the person")
        .unwrap();
    let devtools = chat.devtools_url().await.unwrap();
    assert!(devtools.starts_with("http://127.0.0.1:"), "{devtools}");
    chat.stop_screencast().await;
    hub.close_chat("chat-3").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_speaker_starts_off_and_turns_on_for_current_and_new_pages() {
    let _browser = ONE_BROWSER.lock().await;
    let data = tempfile::tempdir().unwrap();
    let Some(hub) = hub(data.path()) else { return };
    let project = tempfile::tempdir().unwrap();
    let chat = hub.chat("chat-sound", project.path());
    let page = "data:text/html,<audio></audio><p id=s>?</p><script>setInterval(()=>{s.textContent='sound '+(document.querySelector('audio').muted?'off':'on')},50)</script>";
    let shows = |text: &'static str| {
        let chat = Arc::clone(&chat);
        async move {
            for _ in 0..40 {
                let outline = chat.snapshot(None).await.unwrap().snapshot;
                if outline.contains(text) {
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            panic!(
                "the page never showed {text:?}: {}",
                chat.snapshot(None).await.unwrap().snapshot
            );
        }
    };
    chat.open(page, false).await.unwrap();
    assert!(chat.state().await.muted);
    shows("sound off").await;
    chat.set_muted(false).await.unwrap();
    assert!(!chat.state().await.muted);
    shows("sound on").await;
    // A page loaded later follows the speaker too.
    chat.navigate("reload").await.unwrap();
    shows("sound on").await;
    chat.set_muted(true).await.unwrap();
    shows("sound off").await;
    hub.close_chat("chat-sound").await;
}

/// The agent can aim without clicking, and opening the same page again reuses its tab.
#[tokio::test(flavor = "multi_thread")]
async fn the_agent_moves_the_mouse_and_reopening_a_page_reuses_its_tab() {
    let _browser = ONE_BROWSER.lock().await;
    let data = tempfile::tempdir().unwrap();
    let Some(hub) = hub(data.path()) else { return };
    let site = MockServer::start().await;
    Mock::given(path("/aim"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            r#"<!doctype html><title>Aim</title><body style="margin:0;height:600px"><p id="out">still</p>
<script>document.addEventListener('mousemove', e => { document.getElementById('out').textContent = 'moved to ' + e.clientX; });</script></body>"#,
            "text/html",
        ))
        .mount(&site)
        .await;
    let project = tempfile::tempdir().unwrap();
    let chat = hub.chat("chat-aim", project.path());
    let tools = themis_core::browser::tools::browser_tools(
        Arc::new(FnHook::new(|_: &ToolAction| Approval::AllowOnce)),
        themis_core::browser::tools::BrowserScope {
            permissions: themis_core::tools::site_permissions::SitePermissions::new(
                Default::default(),
                None,
            ),
            chat: Arc::clone(&chat),
            cancel: None,
            media: None,
        },
    );
    let tool = |name: &str| tools.iter().find(|tool| tool.name() == name).unwrap();
    let url = format!("{}/aim", site.uri());
    tool("browser_open")
        .execute(json!({"url": url}))
        .await
        .unwrap();
    let moved = tool("browser_move")
        .execute(json!({"x": 300, "y": 100}))
        .await
        .unwrap();
    assert!(
        moved["snapshot"].as_str().unwrap().contains("moved to 300"),
        "{moved}"
    );
    assert!(tool("browser_move")
        .execute(json!({"x": 99999, "y": 1}))
        .await
        .is_err());
    let again = tool("browser_open")
        .execute(json!({"url": url, "new_tab": true}))
        .await
        .unwrap();
    // Only with more than one tab does a result say which it is.
    assert!(again.get("tab").is_none(), "{again}");
}

/// A page left open stops drawing until something needs it, and draws again afterwards, every
/// time. The page counts the long pauses between its frames, which only a freeze can cause.
#[tokio::test(flavor = "multi_thread")]
async fn a_frozen_page_stops_drawing_until_the_next_action() {
    let _browser = ONE_BROWSER.lock().await;
    let data = tempfile::tempdir().unwrap();
    let Some(hub) = hub(data.path()) else { return };
    let site = MockServer::start().await;
    Mock::given(path("/game"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            r#"<!doctype html><title>Game</title><p id="out">none</p><script>
let frames = 0, last = Date.now(), pauses = 0;
function draw() {
  const now = Date.now();
  frames++;
  if (now - last > 1000) pauses++;
  last = now;
  document.getElementById('out').textContent = 'frames ' + frames + ' pauses ' + pauses;
  requestAnimationFrame(draw);
}
requestAnimationFrame(draw);
</script>"#,
            "text/html",
        ))
        .mount(&site)
        .await;
    let project = tempfile::tempdir().unwrap();
    let chat = hub.chat("chat-freeze", project.path());
    let tools = themis_core::browser::tools::browser_tools(
        Arc::new(FnHook::new(|_: &ToolAction| Approval::AllowOnce)),
        themis_core::browser::tools::BrowserScope {
            permissions: themis_core::tools::site_permissions::SitePermissions::new(
                Default::default(),
                None,
            ),
            chat: Arc::clone(&chat),
            cancel: None,
            media: None,
        },
    );
    let tool = |name: &str| tools.iter().find(|tool| tool.name() == name).unwrap();
    tool("browser_open")
        .execute(json!({"url": format!("{}/game", site.uri())}))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    chat.freeze_pages().await;
    tokio::time::sleep(Duration::from_millis(2000)).await;
    let read = || async {
        let snapshot = tool("browser_snapshot").execute(json!({})).await.unwrap();
        let text = snapshot["snapshot"].as_str().unwrap().to_owned();
        let number = |after: &str| -> u64 {
            let start = text.find(after).unwrap_or_else(|| panic!("{text}")) + after.len();
            text[start..]
                .split(|c: char| !c.is_ascii_digit())
                .next()
                .unwrap()
                .parse()
                .unwrap()
        };
        (number("frames "), number("pauses "))
    };
    // The action thaws the page, and drawing carries on; each round needs its own freeze.
    for round in 0..3 {
        let (before, _) = read().await;
        tokio::time::sleep(Duration::from_millis(1000)).await;
        let (after, pauses) = read().await;
        assert_eq!(pauses, round + 1, "round {round}: it drew while frozen");
        assert!(
            after > before + 5,
            "round {round}: it stopped: {before} then {after}"
        );
        chat.freeze_pages().await;
        tokio::time::sleep(Duration::from_millis(2000)).await;
    }
}

/// First-person pages lock the pointer; this browser refuses that, so a stand-in does it.
#[tokio::test(flavor = "multi_thread")]
async fn a_page_can_lock_the_pointer_and_read_mouse_movement() {
    let _browser = ONE_BROWSER.lock().await;
    let data = tempfile::tempdir().unwrap();
    let Some(hub) = hub(data.path()) else { return };
    let site = MockServer::start().await;
    Mock::given(path("/fps"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            r#"<!doctype html><title>Fps</title><body style="margin:0">
<canvas id="c" width="800" height="600" style="display:block"></canvas><p id="out">none</p>
<script>
const out = document.getElementById('out'), c = document.getElementById('c');
let state = 'unlocked', moved = 0;
const show = () => out.textContent = state + ' moved ' + moved;
c.addEventListener('click', () => c.requestPointerLock().catch(e => { state = 'refused'; show(); }));
document.addEventListener('pointerlockchange', () => { state = document.pointerLockElement === c ? 'locked' : 'unlocked'; show(); });
document.addEventListener('mousemove', e => { if (document.pointerLockElement) { moved += Math.abs(e.movementX); show(); } });
</script></body>"#,
            "text/html",
        ))
        .mount(&site)
        .await;
    let project = tempfile::tempdir().unwrap();
    let chat = hub.chat("chat-lock", project.path());
    let tools = themis_core::browser::tools::browser_tools(
        Arc::new(FnHook::new(|_: &ToolAction| Approval::AllowOnce)),
        themis_core::browser::tools::BrowserScope {
            permissions: themis_core::tools::site_permissions::SitePermissions::new(
                Default::default(),
                None,
            ),
            chat: Arc::clone(&chat),
            cancel: None,
            media: None,
        },
    );
    let tool = |name: &str| tools.iter().find(|tool| tool.name() == name).unwrap();
    tool("browser_open")
        .execute(json!({"url": format!("{}/fps", site.uri())}))
        .await
        .unwrap();
    let locked = tool("browser_click")
        .execute(json!({"x": 100, "y": 100}))
        .await
        .unwrap();
    let text = locked["snapshot"].as_str().unwrap();
    assert!(text.contains("locked"), "{text}");
    assert!(
        !text.contains("unlocked") && !text.contains("refused"),
        "{text}"
    );
    // A real mouse move while locked reaches the page with its movement.
    let moved = tool("browser_click")
        .execute(json!({"x": 300, "y": 100}))
        .await
        .unwrap();
    let text = moved["snapshot"].as_str().unwrap();
    assert!(!text.contains("moved 0"), "{text}");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_agent_clicks_a_point_of_a_canvas_seen_in_a_screenshot() {
    let _browser = ONE_BROWSER.lock().await;
    let data = tempfile::tempdir().unwrap();
    let Some(hub) = hub(data.path()) else { return };
    let site = MockServer::start().await;
    Mock::given(path("/board"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            r#"<!doctype html><title>Board</title><body style="margin:0">
<canvas id="c" width="400" height="300" style="display:block"></canvas><p id="out">none</p>
<script>document.getElementById('c').addEventListener('click', e => {
  document.getElementById('out').textContent = 'square ' + Math.floor(e.offsetX / 50) + ',' + Math.floor(e.offsetY / 50) + ' x' + e.detail;
});</script></body>"#,
            "text/html",
        ))
        .mount(&site)
        .await;
    let project = tempfile::tempdir().unwrap();
    let chat = hub.chat("chat-4", project.path());
    let tools = themis_core::browser::tools::browser_tools(
        Arc::new(FnHook::new(|_: &ToolAction| Approval::AllowOnce)),
        themis_core::browser::tools::BrowserScope {
            permissions: themis_core::tools::site_permissions::SitePermissions::new(
                Default::default(),
                None,
            ),
            chat: Arc::clone(&chat),
            cancel: None,
            media: None,
        },
    );
    let tool = |name: &str| tools.iter().find(|tool| tool.name() == name).unwrap();
    tool("browser_open")
        .execute(json!({"url": format!("{}/board", site.uri())}))
        .await
        .unwrap();
    let clicked = tool("browser_click")
        .execute(json!({"x": 175, "y": 125}))
        .await
        .unwrap();
    let snapshot = clicked["snapshot"].as_str().unwrap();
    assert!(snapshot.contains("square 3,2 x1"), "{snapshot}");
    let twice = tool("browser_click")
        .execute(json!({"x": 25, "y": 25, "double": true}))
        .await
        .unwrap();
    assert!(
        twice["snapshot"]
            .as_str()
            .unwrap()
            .contains("square 0,0 x2"),
        "{twice}"
    );
    for wrong in [
        json!({}),
        json!({"x": 10}),
        json!({"ref": "e1", "x": 10, "y": 10}),
        json!({"x": -1, "y": 10}),
        json!({"x": 10, "y": 99999}),
    ] {
        let error = tool("browser_click")
            .execute(wrong.clone())
            .await
            .unwrap_err();
        assert!(!error.to_string().is_empty(), "{wrong}");
    }
    hub.close_chat("chat-4").await;
}

#[tokio::test(flavor = "multi_thread")]
// GitHub's Windows runners (no GPU) paint an animated headless page once and
// then stop; unverified on Windows desktops. Static pages stream there.
#[cfg_attr(
    windows,
    ignore = "a GPU-less Windows runner paints an animated headless page only once"
)]
async fn an_animated_page_streams_at_about_thirty_frames_a_second() {
    let _browser = ONE_BROWSER.lock().await;
    let data = tempfile::tempdir().unwrap();
    let Some(hub) = hub(data.path()) else { return };
    let project = tempfile::tempdir().unwrap();
    let chat = hub.chat("chat-5", project.path());
    chat.open(
        "data:text/html,<body><p id=c>0</p><script>let n=0,c=0;(function f(){n=(n+7)&255;c+=1;document.body.style.background='rgb('+n+',80,120)';document.getElementById('c').textContent='painted '+c;requestAnimationFrame(f)})()</script>",
        false,
    )
    .await
    .unwrap();
    let mut updates = chat.subscribe();
    let before = themis_core::browser::stream_stats();
    chat.start_screencast(400, 300, 1.0).await.unwrap();
    let start = std::time::Instant::now();
    let mut frames = 0;
    while start.elapsed() < std::time::Duration::from_secs(2) {
        if let Ok(Ok(themis_core::browser::BrowserUpdate::Frame { .. })) =
            tokio::time::timeout(std::time::Duration::from_millis(200), updates.recv()).await
        {
            frames += 1;
        }
    }
    // Paced to one frame per 33 ms (about 87 in 2 s unpaced) and still
    // flowing. On failure, say whether the page kept painting and what the
    // stream saw (counters are shared with tests running alongside).
    if !(10..=70).contains(&frames) {
        let after = themis_core::browser::stream_stats();
        let page = chat
            .snapshot(None)
            .await
            .map(|s| s.snapshot)
            .unwrap_or_else(|e| e);
        let painted = page
            .lines()
            .find(|line| line.contains("painted"))
            .unwrap_or("no counter");
        panic!(
            "{frames} frames in 2 s; page: {painted}; stream: {} frames from Chrome, {} starts, {} lagged events, {} restarts",
            after.frames - before.frames,
            after.starts - before.starts,
            after.lagged_events - before.lagged_events,
            after.restarts_after_lag - before.restarts_after_lag,
        );
    }
    chat.stop_screencast().await;
    hub.close_chat("chat-5").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_view_shows_the_page_after_a_first_open_was_refused() {
    let _browser = ONE_BROWSER.lock().await;
    let data = tempfile::tempdir().unwrap();
    let Some(hub) = hub(data.path()) else { return };
    let site = MockServer::start().await;
    Mock::given(path("/game"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            "<!doctype html><title>Game</title><body style=\"margin:0;background:rgb(0,160,0)\"><h1>Game</h1>",
            "text/html",
        ))
        .mount(&site)
        .await;
    // A port nothing listens on, as when a dev server is not up yet.
    let closed = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let project = tempfile::tempdir().unwrap();
    let chat = hub.chat("chat-6", project.path());
    let mut updates = chat.subscribe();
    let refused = chat
        .open(&format!("http://127.0.0.1:{closed}/"), false)
        .await;
    assert!(
        format!("{refused:?}").contains("ERR_CONNECTION_REFUSED"),
        "{refused:?}"
    );
    chat.start_screencast(800, 600, 2.0).await.unwrap();
    chat.open(&format!("{}/game", site.uri()), false)
        .await
        .unwrap();
    // Frames that arrive after the page loaded show it, not the error page.
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            if let Ok(themis_core::browser::BrowserUpdate::Frame { data, .. }) =
                updates.recv().await
            {
                use base64::Engine;
                let jpeg = base64::engine::general_purpose::STANDARD
                    .decode(data)
                    .unwrap();
                let mut decoder = zune_jpeg::JpegDecoder::new(std::io::Cursor::new(jpeg));
                let pixels = decoder.decode().unwrap();
                let info = decoder.info().unwrap();
                let (w, h) = (usize::from(info.width), usize::from(info.height));
                let at = ((h * 3 / 4) * w + w / 2) * 3;
                if pixels[at + 1] > 120 && pixels[at] < 60 {
                    return;
                }
            }
        }
    })
    .await
    .expect("frames of the page once it loads");
    chat.stop_screencast().await;
    hub.close_chat("chat-6").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_stream_and_title_follow_the_person_to_another_site() {
    let _browser = ONE_BROWSER.lock().await;
    let data = tempfile::tempdir().unwrap();
    let Some(hub) = hub(data.path()) else { return };
    let site = MockServer::start().await;
    for (page, title) in [("/a", "First site"), ("/b", "Second site")] {
        Mock::given(path(page))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                format!("<!doctype html><title>{title}</title><h1>{title}</h1>"),
                "text/html",
            ))
            .mount(&site)
            .await;
    }
    let port = site.address().port();
    let project = tempfile::tempdir().unwrap();
    let chat = hub.chat("chat-sites", project.path());
    chat.open(&format!("http://127.0.0.1:{port}/a"), false)
        .await
        .unwrap();
    assert_eq!(
        chat.state().await.title,
        "First site",
        "the page's own title"
    );
    chat.start_screencast(800, 600, 1.0).await.unwrap();
    let mut updates = chat.subscribe();
    // `localhost` is another site than `127.0.0.1`: Chrome moves the page to a new
    // renderer, which ends the old screencast.
    chat.open_for_user(&format!("http://localhost:{port}/b"))
        .await
        .unwrap();
    follows(&mut updates, "Second site", "the person's address").await;

    // Back by the person, forward by the agent: the view follows both across sites.
    chat.navigate_for_user("back").await.unwrap();
    follows(&mut updates, "First site", "the person's Back").await;
    chat.navigate("forward").await.unwrap();
    follows(&mut updates, "Second site", "the agent's Forward").await;
    hub.close_chat("chat-sites").await;
}

/// Waits for a frame and the page `title` after a navigation, as the Workspace view sees them.
async fn follows(
    updates: &mut tokio::sync::broadcast::Receiver<themis_core::browser::BrowserUpdate>,
    title: &str,
    after: &str,
) {
    let (mut frame, mut titled) = (false, false);
    let followed = tokio::time::timeout(std::time::Duration::from_secs(15), async {
        while !(frame && titled) {
            match updates.recv().await {
                Ok(themis_core::browser::BrowserUpdate::Frame { .. }) => frame = true,
                Ok(themis_core::browser::BrowserUpdate::State(state)) => {
                    titled |= state.title == title;
                }
                _ => {}
            }
        }
    })
    .await;
    // On failure, also say what the stream did (counters are shared with tests alongside).
    assert!(
        followed.is_ok(),
        "after {after}: frame {frame}, title {title:?} {titled}; {:?}",
        themis_core::browser::stream_stats()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_person_is_never_held_up_by_an_agent_action_in_progress() {
    let _browser = ONE_BROWSER.lock().await;
    let data = tempfile::tempdir().unwrap();
    let Some(hub) = hub(data.path()) else { return };
    let site = MockServer::start().await;
    // A busy page: it polls for six seconds, so its network never goes quiet while the
    // agent's open settles.
    Mock::given(path("/busy"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            "<!doctype html><title>Busy</title><input id=q aria-label=Query autofocus>
             <script>const until = Date.now() + 6000;
             (function poll() { if (Date.now() < until) fetch('/ping').finally(() => setTimeout(poll, 150)); })();</script>",
            "text/html",
        ))
        .mount(&site)
        .await;
    Mock::given(path("/ping"))
        .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
        .mount(&site)
        .await;
    Mock::given(path("/other"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw("<!doctype html><title>Other</title>", "text/html"),
        )
        .mount(&site)
        .await;
    let project = tempfile::tempdir().unwrap();
    let chat = hub.chat("chat-busy", project.path());
    chat.open("about:blank", false).await.unwrap();
    chat.start_screencast(800, 600, 1.0).await.unwrap();
    let agent = {
        let chat = Arc::clone(&chat);
        let url = format!("{}/busy", site.uri());
        tokio::spawn(async move { chat.open(&url, false).await })
    };
    // Wait for the agent's page to commit (slow runners take over a second to get there),
    // then give it a moment so its open is settling on the page's endless polling.
    let waiting = std::time::Instant::now();
    while !chat.state().await.url.ends_with("/busy") {
        assert!(
            waiting.elapsed() < std::time::Duration::from_secs(10),
            "the agent's page never committed: {}",
            chat.state().await.url
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let opened = std::time::Instant::now();
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert!(!agent.is_finished(), "the agent's open is still settling");

    // Everything the person does answers at once while the agent's action runs.
    let quick = std::time::Duration::from_millis(500);
    let started = std::time::Instant::now();
    let state = chat.state().await;
    assert!(state.url.ends_with("/busy"), "{}", state.url);
    chat.user_input(&themis_core::browser::UserInput::Text {
        text: "hello".into(),
    })
    .await
    .unwrap();
    assert!(chat.devtools_url().await.is_some());
    assert!(started.elapsed() < quick, "took {:?}", started.elapsed());
    assert!(!agent.is_finished());

    agent.await.unwrap().unwrap();
    // The agent is not held up either: a loaded page's endless polling stops counting
    // after a moment, long before it ends.
    assert!(
        opened.elapsed() < std::time::Duration::from_secs(4),
        "took {:?}",
        opened.elapsed()
    );
    let typed = chat.snapshot(None).await.unwrap().snapshot;
    assert!(
        typed.contains("hello"),
        "the person's typing reached the page:\n{typed}"
    );

    // A typed address goes at once, too.
    let started = std::time::Instant::now();
    chat.open_for_user(&format!("{}/other", site.uri()))
        .await
        .unwrap();
    assert!(started.elapsed() < quick, "took {:?}", started.elapsed());
    hub.close_chat("chat-busy").await;
}
