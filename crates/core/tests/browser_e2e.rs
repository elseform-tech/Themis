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
    let site = MockServer::builder().start().await;
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
    Mock::given(path("/opener"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            "<!doctype html><title>Opener</title><a href=\"/second\" target=\"_blank\">Open second</a> <button onclick=\"window.open('/second?script')\">Script window</button>",
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
            screenshots: Default::default(),
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
    chat.start_screencast(800, 600, 1.0, true).await.unwrap();
    // A ref quoted as it looks in a JSON result still works, as models send it.
    let clicked = tool("browser_click")
        .execute(json!({"ref": format!("\"{submit}\"")}))
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
    let state = chat.snapshot(None, false).await.unwrap();
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

    // Keys, and windows a page opens load in the chat's one page.
    tool("browser_press")
        .execute(json!({"key": "Tab"}))
        .await
        .unwrap();
    let error = tool("browser_press")
        .execute(json!({"key": "Hyper+x"}))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("unknown key"), "{error}");
    for (button, landed) in [
        ("\"Open second\"", "/second"),
        ("\"Script window\"", "/second?script"),
    ] {
        let opener = tool("browser_open")
            .execute(json!({"url": format!("{}/opener", site.uri())}))
            .await
            .unwrap();
        let reference = ref_of(opener["snapshot"].as_str().unwrap(), button);
        let clicked = tool("browser_click")
            .execute(json!({"ref": reference}))
            .await
            .unwrap();
        let mut state = chat.state().await;
        for _ in 0..50 {
            if state.url.ends_with(landed) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            state = chat.state().await;
        }
        assert!(state.url.ends_with(landed), "{state:?}");
        let snapshot = tool("browser_snapshot").execute(json!({})).await.unwrap();
        let reports = format!("{clicked}{snapshot}");
        assert!(reports.contains("tried to open a new window"), "{reports}");
    }
    assert!(tool_names(&tools).iter().all(|name| name != "browser_tabs"));
    tool("browser_open")
        .execute(json!({"url": url}))
        .await
        .unwrap();
    let state = chat.state().await;
    assert!(state.url.ends_with("/signup"), "{state:?}");

    // A screenshot of the page and of one element, and a short recording.
    let shot = tool("browser_screenshot").execute(json!({})).await.unwrap();
    let png = std::fs::read(shot["path"].as_str().unwrap()).unwrap();
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    assert!(shot["width"].as_u64().unwrap() > 100, "{shot}");
    // The browser renders at Retina scale, but the model gets CSS pixels.
    let png_width = u32::from_be_bytes(png[16..20].try_into().unwrap());
    assert_eq!(u64::from(png_width), shot["width"].as_u64().unwrap());
    let state = chat.snapshot(None, false).await.unwrap();
    let heading_button = ref_of(&state.snapshot, "\"Create account\"");
    let part = tool("browser_screenshot")
        .execute(json!({"ref": heading_button}))
        .await
        .unwrap();
    assert!(
        part["width"].as_u64().unwrap() < shot["width"].as_u64().unwrap(),
        "{part}"
    );
    // A CSS selector or text=… works as for clicks: a live goal run asked for `body` and
    // `#scene` and went in circles on a ref-only error.
    for element in ["button", "body", "text=Create account"] {
        let by_selector = tool("browser_screenshot")
            .execute(json!({"ref": element}))
            .await
            .unwrap();
        assert!(
            by_selector["width"].as_u64().unwrap() > 0,
            "{element}: {by_selector}"
        );
    }
    let missing = tool("browser_screenshot")
        .execute(json!({"ref": "#no-such-thing"}))
        .await
        .unwrap_err()
        .to_string();
    assert!(missing.contains("#no-such-thing"), "{missing}");
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
    chat.open("data:text/html,<h1>Hello</h1>").await.unwrap();
    let mut updates = chat.subscribe();
    chat.start_screencast(800, 600, 1.0, true).await.unwrap();
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
    chat.start_screencast(800, 600, 1.0, true)
        .await
        .expect("restarting on the same tab");
    chat.start_screencast(640, 480, 2.0, true)
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
    chat.open("data:text/html,<h1>Recorded</h1>").await.unwrap();
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
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        chat.snapshot(None, false),
    )
    .await
    .expect("the agent acts right after the person")
    .unwrap();
    let devtools = chat.devtools_url().await.unwrap();
    assert!(devtools.starts_with("http://127.0.0.1:"), "{devtools}");
    chat.stop_screencast().await;
    hub.close_chat("chat-3").await;
}

/// The next frame: its pixel size, the page's CSS size it names, and its JPEG bytes.
async fn next_frame(
    updates: &mut tokio::sync::broadcast::Receiver<themis_core::browser::BrowserUpdate>,
) -> (usize, usize, u32, u32, usize) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Ok(themis_core::browser::BrowserUpdate::Frame {
                data,
                width,
                height,
            }) = updates.recv().await
            {
                let (_, w, h) = frame_colour(&data);
                return (w, h, width, height, data.len() * 3 / 4);
            }
        }
    })
    .await
    .expect("a frame")
}

/// Opening the Browser tab mid-task leaves the agent's page at its size and sends frames at
/// the panel's own pixel density; only a resize by the person lays the page out again.
#[tokio::test(flavor = "multi_thread")]
async fn the_browser_tab_streams_at_its_pixel_density_and_resizes_only_when_asked() {
    let _browser = ONE_BROWSER.lock().await;
    let data = tempfile::tempdir().unwrap();
    let Some(hub) = hub(data.path()) else { return };
    let project = tempfile::tempdir().unwrap();
    let chat = hub.chat("chat-crisp", project.path());
    // Text over the whole page, as a real one has: the frame's bytes are what streams.
    let words = "Crisp text for the Browser tab. ".repeat(400);
    chat.open(
        // No scrollbar: on Linux a scrollbar would take its width off the visible page size.
        &format!(
            "data:text/html,<html style='overflow:hidden'><body style='font:14px sans-serif'>{words}</body></html>"
        ))
    .await
    .unwrap();
    let size = || async { chat.snapshot(None, false).await.unwrap().visible_page };
    // The agent's page at the default size.
    assert_eq!(size().await.as_deref(), Some("1280x800"));
    let mut updates = chat.subscribe();
    // A 744 × 560 panel on a Retina screen opens while the agent works.
    chat.start_screencast(744, 560, 2.0, false).await.unwrap();
    let (w, h, css_w, css_h, bytes) = next_frame(&mut updates).await;
    eprintln!("kept page: frame {w}x{h} px for page {css_w}x{css_h} css, {bytes} bytes");
    // The page kept its layout, so the agent's coordinates still hold.
    assert_eq!(size().await.as_deref(), Some("1280x800"));
    assert_eq!(
        (css_w, css_h),
        (1280, 800),
        "frames name the page's CSS size"
    );
    // At most the panel's device pixels, the page's shape kept: 1488 × 930.
    assert!(w <= 1488 && h <= 1120, "{w}x{h}");
    assert!(w >= 1480 && (900..=940).contains(&h), "{w}x{h}");
    // The person drags the panel wider: the page follows the panel now.
    chat.start_screencast(900, 560, 2.0, true).await.unwrap();
    assert_eq!(size().await.as_deref(), Some("900x560"));
    let resized = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let (w, h, css_w, _, bytes) = next_frame(&mut updates).await;
            if css_w == 900 {
                return (w, h, bytes);
            }
        }
    })
    .await
    .expect("a frame of the resized page");
    eprintln!(
        "resized page: frame {}x{} px for page 900x560 css, {} bytes",
        resized.0, resized.1, resized.2
    );
    // Chrome on Linux rounds a frame's width down a little (1792 for 1800): within 1%.
    assert!(
        (1782..=1800).contains(&resized.0) && resized.1 == 1120,
        "{}x{}",
        resized.0,
        resized.1
    );
    chat.stop_screencast().await;
    hub.close_chat("chat-crisp").await;
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
                let outline = chat.snapshot(None, false).await.unwrap().snapshot;
                if outline.contains(text) {
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            panic!(
                "the page never showed {text:?}: {}",
                chat.snapshot(None, false).await.unwrap().snapshot
            );
        }
    };
    chat.open(page).await.unwrap();
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
/// On a Mac a page otherwise follows a scroll past its edge and springs back, and a trackpad can
/// wobble even a page with nothing to scroll. The page's own choice wins, and its DOM is untouched.
#[tokio::test(flavor = "multi_thread")]
async fn pages_dont_bounce_at_their_edges_unless_they_ask_to() {
    let _browser = ONE_BROWSER.lock().await;
    let data = tempfile::tempdir().unwrap();
    let Some(hub) = hub(data.path()) else { return };
    let site = MockServer::builder().start().await;
    const REPORT: &str = r#"<p id="out"></p><script>out.textContent = 'overscroll ' + getComputedStyle(document.documentElement).overscrollBehaviorY + ' styles ' + document.querySelectorAll('style').length;</script>"#;
    Mock::given(path("/plain"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            format!("<!doctype html><title>Plain</title><body>{REPORT}</body>"),
            "text/html",
        ))
        .mount(&site)
        .await;
    Mock::given(path("/own"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            format!("<!doctype html><title>Own</title><style>html{{overscroll-behavior:contain}}</style><body>{REPORT}</body>"),
            "text/html",
        ))
        .mount(&site)
        .await;
    let project = tempfile::tempdir().unwrap();
    let chat = hub.chat("chat-bounce", project.path());
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
            screenshots: Default::default(),
        },
    );
    let tool = |name: &str| tools.iter().find(|tool| tool.name() == name).unwrap();
    let read = |page: &'static str| {
        let (open, snapshot) = (tool("browser_open"), tool("browser_snapshot"));
        let url = format!("{}/{page}", site.uri());
        async move {
            open.execute(json!({"url": url})).await.unwrap();
            snapshot.execute(json!({})).await.unwrap().to_string()
        }
    };
    let plain = read("plain").await;
    assert!(plain.contains("overscroll none styles 0"), "{plain}");
    let own = read("own").await;
    assert!(own.contains("overscroll contain styles 1"), "{own}");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_agent_moves_the_mouse_and_reopening_a_page_reuses_its_tab() {
    let _browser = ONE_BROWSER.lock().await;
    let data = tempfile::tempdir().unwrap();
    let Some(hub) = hub(data.path()) else { return };
    let site = MockServer::builder().start().await;
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
            screenshots: Default::default(),
        },
    );
    let tool = |name: &str| tools.iter().find(|tool| tool.name() == name).unwrap();
    let url = format!("{}/aim", site.uri());
    tool("browser_open")
        .execute(json!({"url": url}))
        .await
        .unwrap();
    // Models fill every field: a ref of "null" means no ref, and a zero point beside a ref means no point.
    tool("browser_click")
        .execute(json!({"reference": "null", "x": 20, "y": 20}))
        .await
        .unwrap();
    let snapshot = tool("browser_snapshot")
        .execute(json!({}))
        .await
        .unwrap()
        .to_string();
    assert!(snapshot.contains("moved to 20"), "{snapshot}");
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
}

fn tool_names(tools: &[Box<dyn themis_core::tools::ToolT>]) -> Vec<String> {
    tools.iter().map(|tool| tool.name().to_owned()).collect()
}

/// At most OPEN_PAGES chats keep a page open: one more closes the least recently used idle
/// one, which reopens at the same address when that chat next browses.
#[tokio::test(flavor = "multi_thread")]
async fn only_a_few_chats_keep_a_page_open_and_a_closed_one_reopens_where_it_was() {
    let _browser = ONE_BROWSER.lock().await;
    let data = tempfile::tempdir().unwrap();
    let Some(hub) = hub(data.path()) else { return };
    let project = tempfile::tempdir().unwrap();
    let limit = themis_core::browser::session::OPEN_PAGES;
    let chats: Vec<_> = (0..=limit)
        .map(|i| hub.chat(&format!("chat-{i}"), project.path()))
        .collect();
    for (i, chat) in chats.iter().enumerate() {
        chat.open(&format!(
            "data:text/html,<title>Page {i}</title><h1>Page {i}</h1>"
        ))
        .await
        .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let mut open = Vec::new();
    for chat in &chats {
        open.push(chat.state().await.open);
    }
    assert_eq!(open.iter().filter(|open| **open).count(), limit, "{open:?}");
    assert!(!open[0], "the oldest page closes first: {open:?}");
    let back = chats[0].snapshot(None, false).await.unwrap();
    assert!(back.snapshot.contains("Page 0"), "{}", back.snapshot);
    assert!(
        back.new_since_last_action
            .iter()
            .any(|note| note.contains("closed while unused")),
        "{:?}",
        back.new_since_last_action
    );
    assert!(chats[0].state().await.open);
    let mut still_open = 0;
    for chat in &chats {
        still_open += usize::from(chat.state().await.open);
    }
    assert_eq!(still_open, limit);
}

/// A browser no chat has used for a while exits (its process and helpers gone), and the next
/// action starts it again with each page back at its address. A chat used recently keeps it.
#[tokio::test(flavor = "multi_thread")]
async fn an_idle_browser_exits_and_comes_back_where_it_was() {
    let _browser = ONE_BROWSER.lock().await;
    let data = tempfile::tempdir().unwrap();
    let Some(hub) = hub(data.path()) else { return };
    let project = tempfile::tempdir().unwrap();
    let chat = hub.chat("chat-idle", project.path());
    chat.open("data:text/html,<title>Kept</title><h1>Kept page</h1>")
        .await
        .unwrap();
    assert!(
        !hub.exit_when_idle(std::time::Duration::from_secs(600))
            .await,
        "used just now: stays"
    );
    assert!(chat.state().await.open);
    assert!(hub.exit_when_idle(std::time::Duration::ZERO).await);
    assert!(!chat.state().await.open);
    assert!(
        !hub.exit_when_idle(std::time::Duration::ZERO).await,
        "nothing left to end"
    );
    let back = chat.snapshot(None, false).await.unwrap();
    assert!(back.snapshot.contains("Kept page"), "{}", back.snapshot);
    assert!(chat.state().await.open);
}

/// A first-person page turns without limit: relative moves carry on past the page's edge with
/// the exact distance, and a click doesn't turn the view. A page without the lock has an edge.
#[tokio::test(flavor = "multi_thread")]
async fn a_locked_page_turns_past_the_edge_and_clicking_does_not_turn_it() {
    let _browser = ONE_BROWSER.lock().await;
    let data = tempfile::tempdir().unwrap();
    let Some(hub) = hub(data.path()) else { return };
    let site = MockServer::builder().start().await;
    Mock::given(path("/look"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            r#"<!doctype html><title>Look</title><body style="margin:0;height:100vh"><p id="out">0,0,0</p>
<script>let x = 0, y = 0, shots = 0;
const show = () => { document.getElementById('out').textContent = x + ',' + y + ',' + shots; };
document.addEventListener('mousemove', e => { x += e.movementX; y += e.movementY; show(); });
document.addEventListener('mousedown', () => {
  if (document.pointerLockElement) { shots += 1; show(); } else { document.body.requestPointerLock(); }
});
</script></body>"#,
            "text/html",
        ))
        .mount(&site)
        .await;
    let project = tempfile::tempdir().unwrap();
    let chat = hub.chat("chat-look", project.path());
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
            screenshots: Default::default(),
        },
    );
    let tool = |name: &str| tools.iter().find(|tool| tool.name() == name).unwrap();
    tool("browser_open")
        .execute(json!({"url": format!("{}/look", site.uri())}))
        .await
        .unwrap();
    // No lock yet: the page has an edge, and a move past it says so.
    let refused = tool("browser_move")
        .execute(json!({"dx": 99999}))
        .await
        .unwrap_err()
        .to_string();
    assert!(refused.contains("pointer lock"), "{refused}");
    assert!(tool("browser_move")
        .execute(json!({"x": 1, "dx": 1}))
        .await
        .is_err());
    tool("browser_click")
        .execute(json!({"x": 300, "y": 300}))
        .await
        .unwrap();
    let reading = |report: &Value| {
        let snapshot = report["snapshot"].as_str().unwrap().to_owned();
        snapshot
            .split(|c: char| !(c.is_ascii_digit() || c == ',' || c == '-'))
            .find(|part| part.matches(',').count() == 2)
            .map(|part| {
                part.split(',')
                    .map(|n| n.parse::<i64>().unwrap())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|| panic!("no reading in {snapshot}"))
    };
    let first = tool("browser_move")
        .execute(json!({"dx": 0, "dy": 0}))
        .await
        .unwrap();
    let base = reading(&first);
    // Five full page widths right, then a good way up: every pixel arrives.
    let mut last = first;
    for _ in 0..5 {
        last = tool("browser_move")
            .execute(json!({"dx": 2000}))
            .await
            .unwrap();
    }
    assert_eq!(reading(&last)[0] - base[0], 10_000, "{last}");
    let up = tool("browser_move")
        .execute(json!({"dx": -7, "dy": -3000}))
        .await
        .unwrap();
    assert_eq!(reading(&up)[0] - base[0], 9_993, "{up}");
    assert_eq!(reading(&up)[1] - base[1], -3000, "{up}");
    // A click at some other point shoots and doesn't turn the view.
    let shot = tool("browser_click")
        .execute(json!({"x": 640, "y": 300}))
        .await
        .unwrap();
    let after = reading(&shot);
    assert_eq!(after[0] - base[0], 9_993, "{shot}");
    assert_eq!(after[1] - base[1], -3000, "{shot}");
    assert_eq!(after[2], base[2] + 1, "{shot}");
}

/// A page left open stops drawing until something needs it, and draws again afterwards, every
/// time. The page counts the long pauses between its frames, which only a freeze can cause.
#[tokio::test(flavor = "multi_thread")]
async fn a_frozen_page_stops_drawing_until_the_next_action() {
    let _browser = ONE_BROWSER.lock().await;
    let data = tempfile::tempdir().unwrap();
    let Some(hub) = hub(data.path()) else { return };
    let site = MockServer::builder().start().await;
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
            screenshots: Default::default(),
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
    // The chat's preview of a frozen page still gets a picture of it.
    let mut updates = chat.subscribe();
    chat.start_preview(440).await.unwrap();
    let framed = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match updates.recv().await {
                Ok(themis_core::browser::BrowserUpdate::Frame { data, .. }) => return data,
                Ok(_) => {}
                Err(error) => panic!("{error}"),
            }
        }
    })
    .await
    .expect("no frame for a frozen page");
    // At the preview's size, not the page's.
    let (_, width, height) = frame_colour(&framed);
    assert!(
        (400..=440).contains(&width) && height > 0 && height < width,
        "{width}x{height}"
    );
    chat.stop_screencast().await;
    // The action thaws the page, and drawing carries on; each round needs its own freeze.
    for round in 0..3 {
        let (before, _) = read().await;
        tokio::time::sleep(Duration::from_millis(2000)).await;
        let (after, pauses) = read().await;
        assert_eq!(pauses, round + 1, "round {round}: it drew while frozen");
        // A slow runner may draw only a few frames, but a page left dead draws none.
        assert!(
            after > before + 1,
            "round {round}: it stopped: {before} then {after}"
        );
        chat.freeze_pages().await;
        tokio::time::sleep(Duration::from_millis(2000)).await;
    }
}

/// The mean colour of a base64 JPEG frame, and its size.
fn frame_colour(data: &str) -> ([u8; 3], usize, usize) {
    use base64::Engine;
    let jpeg = base64::engine::general_purpose::STANDARD
        .decode(data)
        .unwrap();
    let (rgb, width, height) = themis_core::browser::media::decode_jpeg(&jpeg).unwrap();
    let mut sum = [0u64; 3];
    for pixel in rgb.as_chunks::<3>().0 {
        for (total, value) in sum.iter_mut().zip(pixel) {
            *total += u64::from(*value);
        }
    }
    let count = (width * height).max(1) as u64;
    (sum.map(|total| (total / count) as u8), width, height)
}

/// Waits up to `limit` for a frame of the stream that `matches`; returns how many came.
async fn frame_where(
    updates: &mut tokio::sync::broadcast::Receiver<themis_core::browser::BrowserUpdate>,
    limit: Duration,
    matches: impl Fn([u8; 3], usize, usize) -> bool,
) -> Result<usize, usize> {
    let mut seen = 0;
    let found = tokio::time::timeout(limit, async {
        loop {
            match updates.recv().await {
                Ok(themis_core::browser::BrowserUpdate::Frame { data, .. }) => {
                    seen += 1;
                    let (colour, width, height) = frame_colour(&data);
                    if matches(colour, width, height) {
                        return;
                    }
                }
                Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(error) => panic!("{error}"),
            }
        }
    })
    .await;
    found.map(|()| seen).map_err(|_| seen)
}

/// Closing Workspace stops its stream as the chat's preview starts. The stop waits for an
/// agent action in progress, so it can land after the preview began: the preview must keep
/// following the page (owner report: it stayed on "Click the circles…" after "All done").
#[tokio::test(flavor = "multi_thread")]
async fn the_preview_follows_the_page_when_a_late_stop_from_workspace_lands() {
    let _browser = ONE_BROWSER.lock().await;
    let data = tempfile::tempdir().unwrap();
    let Some(hub) = hub(data.path()) else { return };
    let site = MockServer::builder().start().await;
    Mock::given(path("/game"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            r#"<!doctype html><title>Circles</title>
<body style="margin:0;background:rgb(200,30,30)"><h1 id="say">Click the circles</h1>
<button id="finish" onclick="document.body.style.background='rgb(30,200,30)';document.getElementById('say').textContent='All done'">Finish</button></body>"#,
            "text/html",
        ))
        .mount(&site)
        .await;
    let project = tempfile::tempdir().unwrap();
    let chat = hub.chat("chat-preview-late-stop", project.path());
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
            screenshots: Default::default(),
        },
    );
    let tools = Arc::new(tools);
    let tool = |tools: &[Box<dyn themis_core::tools::ToolT>], name: &str| {
        tools.iter().position(|tool| tool.name() == name).unwrap()
    };
    let open = tool(&tools, "browser_open");
    tools[open]
        .execute(json!({"url": format!("{}/game", site.uri())}))
        .await
        .unwrap();
    // The person watched in Workspace.
    chat.start_screencast(800, 600, 1.0, true).await.unwrap();
    let mut updates = chat.subscribe();
    // The agent is busy: a held key keeps its action going for a while.
    let press = tool(&tools, "browser_press");
    let busy = {
        let tools = Arc::clone(&tools);
        tokio::spawn(async move {
            tools[press]
                .execute(json!({"key": "x", "hold_ms": 2500}))
                .await
        })
    };
    tokio::time::sleep(Duration::from_millis(300)).await;
    // Workspace closes: its stop is sent first, the preview's start a moment later.
    let stop = {
        let chat = Arc::clone(&chat);
        tokio::spawn(async move { chat.stop_screencast().await })
    };
    tokio::time::sleep(Duration::from_millis(250)).await;
    chat.start_preview(440).await.unwrap();
    busy.await.unwrap().unwrap();
    stop.await.unwrap();
    // The preview is small, and the page still the red one.
    let red = |[r, g, _]: [u8; 3], width: usize, _| width <= 440 && r > 150 && g < 80;
    frame_where(&mut updates, Duration::from_secs(5), red)
        .await
        .expect("the preview never drew the page");
    // The game ends; the preview must show it.
    let snapshot = chat.snapshot(None, false).await.unwrap().snapshot;
    let finish = ref_of(&snapshot, "\"Finish\"");
    tools[tool(&tools, "browser_click")]
        .execute(json!({"ref": finish}))
        .await
        .unwrap();
    let green = |[r, g, _]: [u8; 3], width: usize, _| width <= 440 && g > 150 && r < 80;
    let followed = frame_where(&mut updates, Duration::from_secs(5), green).await;
    assert!(
        followed.is_ok(),
        "the preview kept the old page: {followed:?} frames after the change; {:?}",
        themis_core::browser::stream_stats()
    );
    chat.stop_screencast().await;
    hub.close_chat("chat-preview-late-stop").await;
}

/// First-person pages lock the pointer; this browser refuses that, so a stand-in does it.
#[tokio::test(flavor = "multi_thread")]
async fn a_page_can_lock_the_pointer_and_read_mouse_movement() {
    let _browser = ONE_BROWSER.lock().await;
    let data = tempfile::tempdir().unwrap();
    let Some(hub) = hub(data.path()) else { return };
    let site = MockServer::builder().start().await;
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
            screenshots: Default::default(),
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
    let moved = tool("browser_move")
        .execute(json!({"dx": 200}))
        .await
        .unwrap();
    let text = moved["snapshot"].as_str().unwrap();
    assert!(!text.contains("moved 0"), "{text}");
}

/// The person plays a first-person page in the Workspace view: their click locks the
/// pointer (the view learns it from the state), their locked mouse's movement turns the view
/// past the page's edge, presses land where the pointer is held, and Escape gives it back.
#[tokio::test(flavor = "multi_thread")]
async fn the_person_locks_the_pointer_turns_the_view_and_escapes() {
    use themis_core::browser::UserInput;
    let _browser = ONE_BROWSER.lock().await;
    let data = tempfile::tempdir().unwrap();
    let Some(hub) = hub(data.path()) else { return };
    let site = MockServer::builder().start().await;
    Mock::given(path("/fps"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            r#"<!doctype html><title>Fps</title><body style="margin:0">
<canvas id="c" width="800" height="600" style="display:block"></canvas><p id="out">none</p>
<script>
const out = document.getElementById('out'), c = document.getElementById('c');
let state = 'unlocked', moved = 0, shots = 0;
const show = () => out.textContent = state + ' moved ' + moved + ' shots ' + shots;
c.addEventListener('mousedown', () => { if (document.pointerLockElement) { shots += 1; show(); } else c.requestPointerLock(); });
document.addEventListener('pointerlockchange', () => { state = document.pointerLockElement === c ? 'locked' : 'unlocked'; show(); });
document.addEventListener('mousemove', e => { if (document.pointerLockElement) { moved += Math.abs(e.movementX); show(); } });
</script></body>"#,
            "text/html",
        ))
        .mount(&site)
        .await;
    let project = tempfile::tempdir().unwrap();
    let chat = hub.chat("chat-person-lock", project.path());
    chat.open(&format!("{}/fps", site.uri())).await.unwrap();
    let press = |kind: &str| UserInput::Mouse {
        kind: kind.into(),
        x: 100.0,
        y: 100.0,
        button: "left".into(),
        clicks: 1,
        modifiers: 0,
        delta_x: 0.0,
        delta_y: 0.0,
    };
    let page = || async { chat.snapshot(None, false).await.unwrap().snapshot };
    chat.user_input(&press("down")).await.unwrap();
    chat.user_input(&press("up")).await.unwrap();
    let seen = page().await;
    assert!(
        chat.state().await.pointer_locked,
        "the view is told the page locked: {seen}"
    );
    assert!(page().await.contains("locked moved 0"));
    // Turning far past the page's right edge, as a locked mouse may.
    for _ in 0..6 {
        chat.user_input(&UserInput::MouseBy { dx: 400.0, dy: 0.0 })
            .await
            .unwrap();
    }
    let text = page().await;
    assert!(
        !text.contains("moved 0") && text.contains("locked"),
        "{text}"
    );
    // A press while locked is a shot where the pointer is held, not a new lock.
    chat.user_input(&press("down")).await.unwrap();
    chat.user_input(&press("up")).await.unwrap();
    assert!(page().await.contains("shots 1"));
    // Escape gives the mouse back.
    chat.user_input(&UserInput::Key {
        kind: "down".into(),
        key: "Escape".into(),
        code: "Escape".into(),
        text: None,
        key_code: 27,
        modifiers: 0,
    })
    .await
    .unwrap();
    assert!(!chat.state().await.pointer_locked);
    assert!(page().await.contains("unlocked"));
    // Moves while unlocked are not relative turns.
    chat.user_input(&UserInput::MouseBy { dx: 50.0, dy: 0.0 })
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn the_agent_holds_a_key_named_as_a_page_names_it_and_sees_the_page_size() {
    let _browser = ONE_BROWSER.lock().await;
    let data = tempfile::tempdir().unwrap();
    let Some(hub) = hub(data.path()) else { return };
    let site = MockServer::builder().start().await;
    Mock::given(path("/keys"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            r#"<!doctype html><title>Keys</title><body><p id="out">no key yet</p>
<script>let down = 0;
document.addEventListener('keydown', () => { down = performance.now(); });
document.addEventListener('keyup', e => { document.getElementById('out').textContent =
  e.code + ' held ' + (performance.now() - down >= 1000 ? 'long' : 'briefly'); });
</script>"#,
            "text/html",
        ))
        .mount(&site)
        .await;
    let project = tempfile::tempdir().unwrap();
    let chat = hub.chat("chat-keys", project.path());
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
            screenshots: Default::default(),
        },
    );
    let tool = |name: &str| tools.iter().find(|tool| tool.name() == name).unwrap();
    let opened = tool("browser_open")
        .execute(json!({"url": format!("{}/keys", site.uri())}))
        .await
        .unwrap();
    let size = opened["visible_page"].as_str().unwrap();
    let (width, height) = size.split_once('x').unwrap();
    assert!(width.parse::<u32>().unwrap() > 0, "{opened}");
    assert!(height.parse::<u32>().unwrap() > 0, "{opened}");
    let tapped = tool("browser_press")
        .execute(json!({"key": "KeyW"}))
        .await
        .unwrap();
    assert!(
        tapped["snapshot"].as_str().unwrap().contains("KeyW held"),
        "{tapped}"
    );
    let held = tool("browser_press")
        .execute(json!({"key": "KeyW", "hold_ms": 1200}))
        .await
        .unwrap();
    assert!(
        held["snapshot"]
            .as_str()
            .unwrap()
            .contains("KeyW held long"),
        "{held}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_agent_clicks_a_point_of_a_canvas_seen_in_a_screenshot() {
    let _browser = ONE_BROWSER.lock().await;
    let data = tempfile::tempdir().unwrap();
    let Some(hub) = hub(data.path()) else { return };
    let site = MockServer::builder().start().await;
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
            screenshots: Default::default(),
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
        "data:text/html,<body><p id=c>0</p><script>let n=0,c=0;(function f(){n=(n+7)&255;c+=1;document.body.style.background='rgb('+n+',80,120)';document.getElementById('c').textContent='painted '+c;requestAnimationFrame(f)})()</script>")
    .await
    .unwrap();
    let mut updates = chat.subscribe();
    let before = themis_core::browser::stream_stats();
    chat.start_screencast(400, 300, 1.0, true).await.unwrap();
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
            .snapshot(None, false)
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
    let site = MockServer::builder().start().await;
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
    let refused = chat.open(&format!("http://127.0.0.1:{closed}/")).await;
    assert!(
        format!("{refused:?}").contains("ERR_CONNECTION_REFUSED"),
        "{refused:?}"
    );
    chat.start_screencast(800, 600, 2.0, true).await.unwrap();
    chat.open(&format!("{}/game", site.uri())).await.unwrap();
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
    let site = MockServer::builder().start().await;
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
    chat.open(&format!("http://127.0.0.1:{port}/a"))
        .await
        .unwrap();
    assert_eq!(
        chat.state().await.title,
        "First site",
        "the page's own title"
    );
    chat.start_screencast(800, 600, 1.0, true).await.unwrap();
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
    let site = MockServer::builder().start().await;
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
    chat.open("about:blank").await.unwrap();
    chat.start_screencast(800, 600, 1.0, true).await.unwrap();
    let agent = {
        let chat = Arc::clone(&chat);
        let url = format!("{}/busy", site.uri());
        tokio::spawn(async move { chat.open(&url).await })
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
    let typed = chat.snapshot(None, false).await.unwrap().snapshot;
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

/// Dragging is real mouse input the page can't tell from a person's, whichever way the page
/// listens: native HTML5 drag and drop, or plain mouse events.
#[tokio::test(flavor = "multi_thread")]
async fn the_agent_drags_with_the_mouse_on_html5_and_mouse_event_pages() {
    let _browser = ONE_BROWSER.lock().await;
    let data = tempfile::tempdir().unwrap();
    let Some(hub) = hub(data.path()) else { return };
    let site = MockServer::builder().start().await;
    Mock::given(path("/board"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            r#"<!doctype html><title>Board</title><body style="margin:0">
<button id="card" draggable="true" style="position:fixed;left:20px;top:20px;width:120px;height:40px">Card one</button>
<button id="done" style="position:fixed;left:300px;top:20px;width:200px;height:200px">Done column</button>
<p id="out" style="position:fixed;top:300px">waiting</p>
<script>
const out = document.getElementById('out'), done = document.getElementById('done');
document.getElementById('card').addEventListener('dragstart', e => { e.dataTransfer.setData('text/plain', 'card-1'); });
done.addEventListener('dragover', e => e.preventDefault());
done.addEventListener('drop', e => { e.preventDefault(); out.textContent = 'dropped ' + e.dataTransfer.getData('text/plain') + ' trusted ' + e.isTrusted; });
</script></body>"#,
            "text/html",
        ))
        .mount(&site)
        .await;
    Mock::given(path("/slider"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            r#"<!doctype html><title>Slider</title><body style="margin:0;height:500px">
<div id="knob" style="position:fixed;left:50px;top:50px;width:30px;height:30px;background:#888"></div>
<p id="out" style="position:fixed;top:300px">waiting</p>
<script>
let held = false, moves = 0;
const knob = document.getElementById('knob'), out = document.getElementById('out');
knob.addEventListener('mousedown', e => { held = true; });
document.addEventListener('mousemove', e => { if (held && e.buttons === 1) { moves++; knob.style.left = e.clientX + 'px'; } });
document.addEventListener('mouseup', e => { if (held) out.textContent = 'let go at ' + e.clientX + ' after moves ' + (moves > 1 ? 'many' : moves) + ' trusted ' + e.isTrusted; held = false; });
</script></body>"#,
            "text/html",
        ))
        .mount(&site)
        .await;
    let project = tempfile::tempdir().unwrap();
    let chat = hub.chat("chat-drag", project.path());
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
            screenshots: Default::default(),
        },
    );
    let tool = |name: &str| tools.iter().find(|tool| tool.name() == name).unwrap();

    // By refs, on a page using native drag and drop.
    let opened = tool("browser_open")
        .execute(json!({"url": format!("{}/board", site.uri())}))
        .await
        .unwrap();
    let snapshot = opened["snapshot"].as_str().unwrap();
    let (card, done) = (
        ref_of(snapshot, "Card one\""),
        ref_of(snapshot, "Done column\""),
    );
    let dropped = tool("browser_drag")
        .execute(json!({"from_reference": card, "to_reference": done}))
        .await
        .unwrap();
    let text = dropped["snapshot"].as_str().unwrap();
    assert!(text.contains("dropped card-1 trusted true"), "{text}");

    // By points, on a page using plain mouse events; a model's zero points beside refs are ignored.
    tool("browser_open")
        .execute(json!({"url": format!("{}/slider", site.uri())}))
        .await
        .unwrap();
    let slid = tool("browser_drag")
        .execute(json!({"from_x": 65, "from_y": 65, "to_x": 400, "to_y": 65}))
        .await
        .unwrap();
    let text = slid["snapshot"].as_str().unwrap();
    assert!(
        text.contains("let go at 400 after moves many trusted true"),
        "{text}"
    );

    // Nothing to go on, half a point, or a point off the page: told, and nothing happens.
    for args in [
        json!({}),
        json!({"from_x": 10, "from_y": 10}),
        json!({"from_x": 10, "to_x": 20, "to_y": 20}),
        json!({"from_x": 10, "from_y": 10, "to_x": 99999, "to_y": 10}),
    ] {
        assert!(
            tool("browser_drag").execute(args.clone()).await.is_err(),
            "{args}"
        );
    }

    // Holding the button needs somewhere to hold it, and letting go needs a press.
    let up = tool("browser_press")
        .execute(json!({"key": "mouseUp"}))
        .await
        .unwrap_err()
        .to_string();
    assert!(up.contains("mouseDown"), "{up}");
}

/// The agent can't see a pointer, so results say what it is over and what a drag did. Plain
/// `div`s that can be dragged have no role, and without a ref it could only guess at them.
#[tokio::test(flavor = "multi_thread")]
async fn the_agent_reads_what_is_under_the_mouse_and_what_a_drag_did() {
    let _browser = ONE_BROWSER.lock().await;
    let data = tempfile::tempdir().unwrap();
    let Some(hub) = hub(data.path()) else { return };
    let site = MockServer::builder().start().await;
    Mock::given(path("/cards"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            r#"<!doctype html><title>Cards</title><body style="margin:0">
<div id="todo" class="column" style="position:fixed;left:0;top:0;width:200px;height:300px">
  <div id="card" class="task-card" draggable="true" style="margin:40px 10px;height:40px">Write the report</div>
</div>
<div id="done" class="column" style="position:fixed;left:300px;top:0;width:200px;height:300px">Done</div>
<script>
const done = document.getElementById('done'), card = document.getElementById('card');
card.addEventListener('dragstart', e => e.dataTransfer.setData('text/plain', 'x'));
done.addEventListener('dragover', e => e.preventDefault());
done.addEventListener('drop', e => { e.preventDefault(); done.appendChild(card); });
</script></body>"#,
            "text/html",
        ))
        .mount(&site)
        .await;
    let project = tempfile::tempdir().unwrap();
    let chat = hub.chat("chat-mouse", project.path());
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
            screenshots: Default::default(),
        },
    );
    let tool = |name: &str| tools.iter().find(|tool| tool.name() == name).unwrap();
    let opened = tool("browser_open")
        .execute(json!({"url": format!("{}/cards", site.uri())}))
        .await
        .unwrap();

    // A draggable div gets a ref it can use, named by its text.
    let snapshot = opened["snapshot"].as_str().unwrap();
    assert!(
        snapshot.contains("draggable \"Write the report\" ["),
        "{snapshot}"
    );
    let card = ref_of(snapshot, "Write the report\"");

    // Over nothing, and over the card: it reads which, without a screenshot.
    let moved = tool("browser_move")
        .execute(json!({"x": 100, "y": 60}))
        .await
        .unwrap();
    let under = moved["under_mouse"].as_str().unwrap();
    assert!(
        under.contains("task-card") && under.contains("Write the report"),
        "{under}"
    );
    assert!(under.contains("draggable"), "{under}");
    assert_eq!(chat.state().await.pointer, Some((100.0, 60.0)));

    // A miss says so: it pressed on the column, and no drag began.
    let missed = tool("browser_drag")
        .execute(json!({"from_x": 100, "from_y": 250, "to_x": 400, "to_y": 150}))
        .await
        .unwrap();
    let result = missed["drag"].as_str().unwrap();
    assert!(result.contains("column"), "{result}");
    assert!(result.contains("nothing was dragged"), "{result}");

    // A hit names both ends and the outcome, by ref.
    let snapshot = tool("browser_snapshot")
        .execute(json!({}))
        .await
        .unwrap()
        .to_string();
    let done = {
        let after = tool("browser_drag")
            .execute(json!({"from_reference": card, "to_x": 400, "to_y": 150}))
            .await
            .unwrap();
        after["drag"].as_str().unwrap().to_owned()
    };
    assert!(done.contains("Write the report"), "{done} after {snapshot}");
    assert!(done.contains("native drag"), "{done}");
    assert!(
        done.contains("dropped on") && done.contains("column"),
        "{done}"
    );
    assert_eq!(chat.state().await.pointer, Some((400.0, 150.0)));
}

const BOARD: &str = r#"<!doctype html><title>Board</title><body style="margin:0">
<div class="column todo" style="position:fixed;left:0;top:0;width:200px;height:300px"><h3>To Do</h3>
  <div id="card" class="task-card" draggable="true" style="height:40px">Write the report</div></div>
<div id="done" class="column" style="position:fixed;left:300px;top:0;width:200px;height:300px"><h3>Done</h3></div>
<p id="out" style="position:fixed;top:320px">card in To Do</p>
<script>
const done = document.getElementById('done'), card = document.getElementById('card'), out = document.getElementById('out');
card.addEventListener('dragstart', e => e.dataTransfer.setData('text/plain', 'x'));
done.addEventListener('dragover', e => e.preventDefault());
done.addEventListener('drop', e => { e.preventDefault(); done.appendChild(card); out.textContent = 'card in Done'; });
</script></body>"#;

/// What agents reach for, taken from what one tried in a real run: elements by selector or text,
/// as Playwright takes them, and the mouse held down by hand.
#[tokio::test(flavor = "multi_thread")]
async fn the_agent_aims_by_selector_and_can_hold_the_mouse_down() {
    let _browser = ONE_BROWSER.lock().await;
    let data = tempfile::tempdir().unwrap();
    let Some(hub) = hub(data.path()) else { return };
    let site = MockServer::builder().start().await;
    Mock::given(path("/board"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(BOARD, "text/html"))
        .mount(&site)
        .await;
    let project = tempfile::tempdir().unwrap();
    let chat = hub.chat("chat-aim-selector", project.path());
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
            screenshots: Default::default(),
        },
    );
    let tool = |name: &str| tools.iter().find(|tool| tool.name() == name).unwrap();
    let url = format!("{}/board", site.uri());
    let open = || async {
        tool("browser_open")
            .execute(json!({"url": url}))
            .await
            .unwrap()
    };

    // By text and by CSS, for either end.
    open().await;
    let moved = tool("browser_drag")
        .execute(json!({"from_reference": "text=Write the report", "to_reference": "#done"}))
        .await
        .unwrap();
    assert!(
        moved["snapshot"].as_str().unwrap().contains("card in Done"),
        "{moved}"
    );

    tool("browser_navigate")
        .execute(json!({"action": "reload"}))
        .await
        .unwrap();
    let moved = tool("browser_drag")
        .execute(json!({"from_reference": ".task-card", "to_reference": "text=Done"}))
        .await
        .unwrap();
    assert!(
        moved["snapshot"].as_str().unwrap().contains("card in Done"),
        "{moved}"
    );

    // Nothing matching says so, and names what to do.
    let missing = tool("browser_drag")
        .execute(json!({"from_reference": "text=No such card", "to_reference": "#done"}))
        .await
        .unwrap_err()
        .to_string();
    assert!(
        missing.contains("No such card") && missing.contains("browser_snapshot"),
        "{missing}"
    );

    // Clicking takes them too.
    tool("browser_navigate")
        .execute(json!({"action": "reload"}))
        .await
        .unwrap();
    tool("browser_click")
        .execute(json!({"reference": "text=Write the report"}))
        .await
        .unwrap();

    // The mouse by hand, as the agent tried: move, press, move, let go.
    tool("browser_navigate")
        .execute(json!({"action": "reload"}))
        .await
        .unwrap();
    tool("browser_move")
        .execute(json!({"x": 100, "y": 80}))
        .await
        .unwrap();
    let down = tool("browser_press")
        .execute(json!({"key": "mouseDown"}))
        .await
        .unwrap();
    assert!(
        down["under_mouse"]
            .as_str()
            .unwrap()
            .contains("Write the report"),
        "{down}"
    );
    tool("browser_move")
        .execute(json!({"x": 400, "y": 150}))
        .await
        .unwrap();
    let up = tool("browser_press")
        .execute(json!({"key": "mouseUp"}))
        .await
        .unwrap();
    assert!(
        up["snapshot"].as_str().unwrap().contains("card in Done"),
        "{up}"
    );
    assert!(up["drag"].as_str().unwrap().contains("native drag"), "{up}");

    // Having let go, moving is just moving again, with no button held.
    tool("browser_move")
        .execute(json!({"x": 10, "y": 10}))
        .await
        .unwrap();
}

const SKELETON: &str = r#"<!doctype html><title>Skeleton</title><body style="margin:0">
<main class="board" style="position:fixed;left:0;top:50px;width:600px;height:300px">
  <section class="column todo" style="position:absolute;left:0;top:0;width:200px;height:300px"><h3>To Do</h3>
    <div id="card" class="task-card" draggable="true" style="height:40px">Write the report</div></section>
  <section class="column" style="position:absolute;left:300px;top:0;width:200px;height:300px"><h3>Done</h3></section>
</main>
<div style="display:none" class="never-shown">Hidden words</div>
<input type="password" class="secret" value="hunter2-secret" style="position:fixed;top:0;left:0">
<canvas id="empty"></canvas><canvas id="blank" width="50" height="50"></canvas>
<canvas id="drawn" width="50" height="50"></canvas>
<p id="out" style="position:fixed;top:400px">card in To Do</p>
<script>
document.getElementById('empty').width = 0; document.getElementById('empty').height = 0;
const pen = document.getElementById('drawn').getContext('2d'); pen.fillStyle = 'red'; pen.fillRect(5, 5, 10, 10);
const done = document.querySelectorAll('.column')[1], card = document.getElementById('card');
card.addEventListener('dragstart', e => e.dataTransfer.setData('text/plain', 'x'));
done.addEventListener('dragover', e => e.preventDefault());
done.addEventListener('drop', e => { e.preventDefault(); done.appendChild(card); document.getElementById('out').textContent = 'card in Done'; });
</script></body>"#;

/// The page's containers, where they are and how to name them, so nothing has to be guessed;
/// and whether a canvas drew anything, which a model that can't see has no other way to tell.
#[tokio::test(flavor = "multi_thread")]
async fn the_agent_reads_the_pages_structure_instead_of_guessing() {
    let _browser = ONE_BROWSER.lock().await;
    let data = tempfile::tempdir().unwrap();
    let Some(hub) = hub(data.path()) else { return };
    let site = MockServer::builder().start().await;
    Mock::given(path("/skeleton"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(SKELETON, "text/html"))
        .mount(&site)
        .await;
    let many = format!(
        "<!doctype html><title>Many</title><body>{}</body>",
        "<div class=\"row\">x</div>".repeat(5000)
    );
    Mock::given(path("/many"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(many, "text/html"))
        .mount(&site)
        .await;
    let project = tempfile::tempdir().unwrap();
    let chat = hub.chat("chat-skeleton", project.path());
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
            screenshots: Default::default(),
        },
    );
    let tool = |name: &str| tools.iter().find(|tool| tool.name() == name).unwrap();
    let opened = tool("browser_open")
        .execute(json!({"url": format!("{}/skeleton", site.uri())}))
        .await
        .unwrap();
    assert!(opened.get("layout").is_none(), "only when asked: {opened}");

    let read = tool("browser_snapshot")
        .execute(json!({"layout": true}))
        .await
        .unwrap();
    let layout = read["layout"].as_str().unwrap();
    let line = |needle: &str| {
        layout
            .lines()
            .find(|line| line.contains(needle))
            .unwrap_or_else(|| panic!("no line with {needle} in:\n{layout}"))
            .to_owned()
    };

    // Containers the outline leaves out, nested, with where they are.
    assert!(line("section.column").contains("(0,50"), "{layout}");
    let done = line("\"Done\"");
    assert!(
        done.contains("(300,50") && done.contains("200×300"),
        "{layout}"
    );
    assert!(line("task-card").contains("(draggable)"), "{layout}");
    assert!(
        line("task-card").starts_with("    "),
        "nested under its column:\n{layout}"
    );

    // Canvases say what they drew.
    assert!(
        line("canvas#empty").contains("0×0") && line("canvas#empty").contains("nothing drawn"),
        "{layout}"
    );
    assert!(line("canvas#blank").contains("blank"), "{layout}");
    assert!(line("canvas#drawn").contains("has drawing"), "{layout}");

    // What is hidden isn't listed, and a field's value never appears.
    assert!(!layout.contains("Hidden words"), "{layout}");
    assert!(!layout.contains("hunter2"), "{layout}");

    // The selector printed for a line is one the tools take.
    let selector = done.rsplit("→ ").next().unwrap().trim().to_owned();
    let moved = tool("browser_drag")
        .execute(json!({"from_reference": "#card", "to_reference": selector}))
        .await
        .unwrap();
    assert!(
        moved["snapshot"].as_str().unwrap().contains("card in Done"),
        "{moved}"
    );

    // A huge page stays bounded, and says so.
    tool("browser_open")
        .execute(json!({"url": format!("{}/many", site.uri())}))
        .await
        .unwrap();
    let read = tool("browser_snapshot")
        .execute(json!({"layout": true}))
        .await
        .unwrap();
    let layout = read["layout"].as_str().unwrap();
    assert!(
        layout.lines().count() <= 202,
        "{} lines",
        layout.lines().count()
    );
    assert!(
        layout.contains("more not shown"),
        "{}",
        &layout[layout.len().saturating_sub(120)..]
    );
}

/// Trailboard as an agent built it: a comment line before the doctype (shown on the page, and
/// the page drops to quirks mode), the same `//` line first in its stylesheet (which kills the
/// first rule, the columns' background), an "Add task" button whose text is its background
/// colour in light mode only, a stylesheet that is missing and a console error.
const TRAILBOARD: &str = r#"// part of the project
<!doctype html><html><head><title>Trailboard</title>
<link rel="stylesheet" href="/trail/style.css">
<link rel="stylesheet" href="/trail/theme.css">
</head><body>
<main class="board">
  <section class="column"><h2>To do</h2><p>Plan the route</p></section>
  <section class="column"><h2>Done</h2><p>Buy boots</p></section>
</main>
<button class="add">Add task</button>
<script>console.error('board state failed to load');</script>
</body></html>"#;

const TRAILBOARD_CSS: &str = r#"// part of the project
.column { background: #e8eefc; padding: 8px; }
body { font-family: sans-serif; background: #ffffff; color: #1f2937; }
.board { display: flex; gap: 12px; }
button.add { background: #2563eb; color: #2563eb; border: none; padding: 8px 12px; }
@media (prefers-color-scheme: dark) {
  body { background: #111827; color: #e5e7eb; }
  button.add { color: #ffffff; }
}
"#;

/// The same board done right, with things that only look like problems: a code sample, a
/// visually hidden label, a strip that scrolls sideways on purpose and a dark mode.
const CLEAN_BOARD: &str = r#"<!doctype html><html><head><title>Clean board</title>
<meta name="color-scheme" content="light dark">
<style>
body { font-family: sans-serif; background: #ffffff; color: #1f2937; margin: 16px; }
.column { background: #e8eefc; padding: 8px; }
.board { display: flex; gap: 12px; }
button.add { background: #2563eb; color: #ffffff; border: none; padding: 8px 12px; }
.visually-hidden { position: absolute; width: 1px; height: 1px; overflow: hidden; clip: rect(0 0 0 0); white-space: nowrap; }
.strip { overflow-x: auto; white-space: nowrap; width: 300px; }
.strip span { display: inline-block; width: 200px; }
input { border: 1px solid #6b7280; background: #ffffff; color: #111827; }
@media (prefers-color-scheme: dark) {
  body { background: #111827; color: #e5e7eb; }
  .column { background: #1f2937; }
  input { background: #1f2937; color: #f9fafb; border-color: #9ca3af; }
}
</style></head><body>
<main class="board">
  <section class="column"><h2>To do</h2><p>Plan the route</p></section>
  <section class="column"><h2>Done</h2><p>Buy boots</p></section>
</main>
<label><span class="visually-hidden">New task</span><input placeholder="New task"></label>
<button class="add">Add task</button>
<pre><code>// part of the project
function add() {}</code></pre>
<div class="strip"><span>One</span><span>Two</span><span>Three</span></div>
</body></html>"#;

/// The other kinds of problem the check measures, one each.
const CUT_OFF: &str = r#"<!doctype html><html><head><title>Cut off</title><style>
body { margin: 0; background: #ffffff; color: #111827; font-family: sans-serif; }
.banner { width: 3000px; background: #fde68a; }
.note { height: 18px; width: 120px; overflow: hidden; }
.ghost { background: transparent; border: 0; color: transparent; width: 60px; height: 30px; }
.bare { border: 0; background: #ffffff; color: #111827; }
.faint { background: #ffffff; color: #b0b0b0; border: 1px solid #111827; }
</style></head><body>
<div class="banner">Spring sale banner</div>
<p class="note">This note is far too long to fit inside its small box, so most of it is hidden from view.</p>
<button class="ghost">Save</button>
<input class="bare" aria-label="Search">
<button class="faint">Cancel</button>
</body></html>"#;

#[tokio::test(flavor = "multi_thread")]
async fn the_looks_check_finds_what_broke_trailboard_and_nothing_on_a_clean_page() {
    let _browser = ONE_BROWSER.lock().await;
    let data = tempfile::tempdir().unwrap();
    let Some(hub) = hub(data.path()) else { return };
    let site = MockServer::builder().start().await;
    Mock::given(path("/trail/"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(TRAILBOARD, "text/html; charset=utf-8"),
        )
        .mount(&site)
        .await;
    Mock::given(path("/trail/style.css"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(TRAILBOARD_CSS, "text/css"))
        .mount(&site)
        .await;
    Mock::given(path("/trail/theme.css"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&site)
        .await;
    Mock::given(path("/cut/"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(CUT_OFF, "text/html; charset=utf-8"))
        .mount(&site)
        .await;
    Mock::given(path("/clean/"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(CLEAN_BOARD, "text/html; charset=utf-8"),
        )
        .mount(&site)
        .await;
    let project = tempfile::tempdir().unwrap();
    let chat = hub.chat("chat-looks", project.path());
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
            screenshots: Default::default(),
        },
    );
    let tool = |name: &str| {
        tools
            .iter()
            .find(|tool| tool.name() == name)
            .unwrap_or_else(|| panic!("{name}"))
    };
    let dark = "window.matchMedia('(prefers-color-scheme: dark)').matches";
    let scheme =
        || async { chat.dev_console(Some(dark), 0, false).await.unwrap()["value"].clone() };

    let opened = tool("browser_open")
        .execute(json!({"url": format!("{}/trail/", site.uri())}))
        .await
        .unwrap();
    let before = scheme().await;
    let looks = tool("browser_check_looks")
        .execute(json!({}))
        .await
        .unwrap();
    let text = looks.to_string();
    assert_eq!(looks["untrusted"], true, "{looks:#}");
    assert!(looks["problems"].as_u64().unwrap() >= 6, "{looks:#}");
    // One list of the result as plain text.
    let list = |name: &str| {
        looks[name]
            .as_array()
            .map(|lines| {
                lines
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default()
    };
    let page = list("page");
    assert!(page.contains("quirks mode"), "{looks:#}");
    assert!(
        page.contains("theme.css") && page.contains("404"),
        "{looks:#}"
    );
    assert!(page.contains("board state failed to load"), "{looks:#}");
    // The dropped rule names its file, line and the line itself.
    assert!(
        page.contains("style.css")
            && page.contains("line 1")
            && page.contains("// part of the project"),
        "{looks:#}"
    );
    // The comment line shown as page text, whatever the colour scheme.
    let both = list("both");
    assert!(
        both.contains("source code") && both.contains("// part of the project"),
        "{looks:#}"
    );
    // The button is unreadable in light mode only, and named the way the snapshot names it.
    let light = list("light_only");
    let snapshot = opened["snapshot"].as_str().unwrap();
    let add = ref_of(snapshot, "\"Add task\"");
    assert!(
        light.contains("button \"Add task\"") && light.contains(&format!("[{add}]")),
        "{looks:#}"
    );
    assert!(!list("dark_only").contains("Add task"), "{looks:#}");
    assert!(!both.contains("Add task"), "{looks:#}");
    assert!(text.len() < 8_000, "bounded: {} bytes", text.len());
    // The page is back in the scheme it had.
    assert_eq!(scheme().await, before);

    tool("browser_open")
        .execute(json!({"url": format!("{}/cut/", site.uri())}))
        .await
        .unwrap();
    let cut = tool("browser_check_looks")
        .execute(json!({}))
        .await
        .unwrap();
    let found = cut["both"]
        .as_array()
        .map(|lines| {
            lines
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    for expected in [
        "past it): div.banner \"Spring sale banner\"",
        "text cut off",
        "control with no visible text or edge: button \"Save\"",
        "field with no visible edge: textbox \"Search\"",
        "faint control text",
    ] {
        assert!(found.contains(expected), "{expected} in {cut:#}");
    }
    assert!(
        cut["page"].to_string().contains("scrolls sideways"),
        "{cut:#}"
    );

    tool("browser_open")
        .execute(json!({"url": format!("{}/clean/", site.uri())}))
        .await
        .unwrap();
    let clean = tool("browser_check_looks")
        .execute(json!({}))
        .await
        .unwrap();
    assert_eq!(clean["problems"], 0, "{clean:#}");
    assert_eq!(scheme().await, before);

    // A big page is looked at only up to the cap, and says so.
    let big = format!(
        "<!doctype html><title>Big</title>{}",
        "<p>row</p>".repeat(6_000)
    );
    Mock::given(path("/big/"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(big, "text/html"))
        .mount(&site)
        .await;
    tool("browser_open")
        .execute(json!({"url": format!("{}/big/", site.uri())}))
        .await
        .unwrap();
    let big = tool("browser_check_looks")
        .execute(json!({}))
        .await
        .unwrap();
    assert_eq!(
        big["checked"], "the first 5000 elements, in light and dark",
        "{big:#}"
    );
    hub.close_chat("chat-looks").await;
}

/// Secrets on a page with problems: a filled password field and secret-looking text fields,
/// all unreadable, and stylesheets whose URLs carry a token.
const SECRET_FORM: &str = r#"<!doctype html><html><head><title>Secrets</title>
<link rel="stylesheet" href="/sec/style.css?token=tok-stylesheet-0451">
<link rel="stylesheet" href="/sec/gone.css?token=tok-missing-0452#frag-0453">
</head><body>
<input type="password" value="hunter2-looks-secret" class="hush">
<input name="note" value="sk9f8e7d6c5b4a39281706f5e4d3c2b1a0zz" class="hush">
<input name="api_key" value="plainish-key-value-0454" class="hush">
</body></html>"#;

const SECRET_CSS: &str =
    "// a comment line\n.gone { color: red; }\n.hush { color: #ffffff; background: #ffffff; border: 1px solid #000000; }\n";

#[tokio::test(flavor = "multi_thread")]
async fn the_looks_check_never_shows_field_values_or_tokens_in_urls() {
    let _browser = ONE_BROWSER.lock().await;
    let data = tempfile::tempdir().unwrap();
    let Some(hub) = hub(data.path()) else { return };
    let site = MockServer::builder().start().await;
    Mock::given(path("/sec/"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(SECRET_FORM, "text/html; charset=utf-8"),
        )
        .mount(&site)
        .await;
    Mock::given(path("/sec/style.css"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(SECRET_CSS, "text/css"))
        .mount(&site)
        .await;
    Mock::given(path("/sec/gone.css"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&site)
        .await;
    let project = tempfile::tempdir().unwrap();
    let chat = hub.chat("chat-looks-secrets", project.path());
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
            screenshots: Default::default(),
        },
    );
    let tool = |name: &str| {
        tools
            .iter()
            .find(|tool| tool.name() == name)
            .unwrap_or_else(|| panic!("{name}"))
    };
    tool("browser_open")
        .execute(json!({"url": format!("{}/sec/", site.uri())}))
        .await
        .unwrap();
    let looks = tool("browser_check_looks")
        .execute(json!({}))
        .await
        .unwrap();
    let text = looks.to_string();
    for secret in [
        "hunter2",
        "sk9f8e7d6c5b4a39281706f5e4d3c2b1a0zz",
        "plainish-key-value-0454",
        "tok-stylesheet-0451",
        "tok-missing-0452",
        "frag-0453",
    ] {
        assert!(!text.contains(secret), "{secret} leaked: {looks:#}");
    }
    // The problems are still reported.
    assert!(text.matches("invisible text").count() >= 3, "{looks:#}");
    assert!(
        text.contains("style.css line 1") && text.contains("gone.css (HTTP 404)"),
        "{looks:#}"
    );
    hub.close_chat("chat-looks-secrets").await;
}
