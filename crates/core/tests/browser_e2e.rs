//! The built-in browser against a real Chromium and a local test page: the
//! snapshot, refs, typing, clicking, dropdowns, keys, notes, password masking,
//! dialogs, history and tabs. Skipped (with a message) when no Chromium is
//! installed.

use std::sync::{Arc, Mutex};

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

fn hub(root: &std::path::Path) -> Option<Arc<BrowserHub>> {
    let hub = BrowserHub::new(BrowserConfig {
        root: root.to_path_buf(),
        executable: None,
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
