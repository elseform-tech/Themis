//! The agent's debugging tools against a real Chromium and a local app: console messages
//! and expressions, elements, network with hidden credentials, storage, a breakpoint with
//! stepping and frame evaluation, performance and memory. Skipped without a Chromium.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use themis_core::browser::{BrowserConfig, BrowserHub};
use themis_core::configuration::BrowserConfig as BrowserSettings;
use themis_core::tools::site_permissions::SitePermissions;
use themis_core::tools::{Approval, FnHook, ToolAction, ToolT};
use wiremock::matchers::path;
use wiremock::{Mock, MockServer, ResponseTemplate};

const PAGE: &str = r#"<!doctype html><html><head><title>Debug me</title></head><body>
<h1 id="t" style="color: rgb(255, 0, 0)">Hello</h1>
<script src="/app.js"></script></body></html>"#;

// Line 4 is inside `total`, where the breakpoint goes.
const SCRIPT: &str = "console.log('booted', 42);
console.warn('careful');
function total(items) { let sum = 0; for (const i of items) {
  sum += i;
} return sum; }
window.total = total;
localStorage.setItem('theme', 'dark');
localStorage.setItem('authToken', 'eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0In0.c2lnbmF0dXJlLXZhbHVl');
document.cookie = 'prefs=compact';
fetch('/api/items', {headers: {Authorization: 'Bearer abcdefghijklmnopqrstuvwxyz1234567890'}}).then(r => r.json());
";

struct TempMedia(std::path::PathBuf);
impl themis_core::browser::media::MediaSink for TempMedia {
    fn save(&self, name: &str, bytes: &[u8]) -> Result<String, String> {
        let path = self.0.join(name);
        std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
        Ok(path.to_string_lossy().into_owned())
    }
}

fn tools_for(
    chat: &Arc<themis_core::browser::ChatBrowser>,
    media: &std::path::Path,
    settings: BrowserSettings,
) -> Vec<Box<dyn ToolT>> {
    themis_core::browser::tools::browser_tools(
        Arc::new(FnHook::new(|_: &ToolAction| Approval::AllowOnce)),
        themis_core::browser::tools::BrowserScope {
            permissions: SitePermissions::new(settings, None),
            chat: Arc::clone(chat),
            cancel: None,
            media: Some(Arc::new(TempMedia(media.to_path_buf()))),
        },
    )
}

async fn run(tools: &[Box<dyn ToolT>], name: &str, args: Value) -> Value {
    let tool = tools
        .iter()
        .find(|tool| tool.name() == name)
        .unwrap_or_else(|| panic!("{name} not offered"));
    tool.execute(args)
        .await
        .unwrap_or_else(|error| panic!("{name}: {error}"))
}

#[tokio::test(flavor = "multi_thread")]
async fn the_agent_debugs_a_local_app_with_the_devtools_toolset() {
    let data = tempfile::tempdir().unwrap();
    let hub = BrowserHub::new(BrowserConfig {
        root: data.path().to_path_buf(),
        executable: None,
        headless: true,
    });
    if hub.executable().is_none() {
        eprintln!("skipping: no Chromium browser installed");
        return;
    }
    let site = MockServer::start().await;
    Mock::given(path("/app"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(PAGE, "text/html"))
        .mount(&site)
        .await;
    Mock::given(path("/app.js"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(SCRIPT, "text/javascript"))
        .mount(&site)
        .await;
    Mock::given(path("/api/items"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Set-Cookie", "sid=topsecret; HttpOnly; Path=/")
                .set_body_raw(r#"{"items":[1,2]}"#, "application/json"),
        )
        .mount(&site)
        .await;
    let project = tempfile::tempdir().unwrap();
    let media = tempfile::tempdir().unwrap();
    let chat = hub.chat("debug-chat", project.path());
    let tools = tools_for(&chat, media.path(), BrowserSettings::default());
    run(
        &tools,
        "browser_open",
        json!({"url": format!("{}/app", site.uri())}),
    )
    .await;
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Console: every level, then expressions and exceptions.
    let messages = run(&tools, "browser_devtools", json!({"panel": "console"})).await;
    let text = messages["messages"].to_string();
    assert!(
        text.contains("booted 42") && text.contains("\"level\":\"warning\"")
            || text.contains("\"level\":\"warn\""),
        "{messages}"
    );
    assert_eq!(
        run(
            &tools,
            "browser_devtools",
            json!({"panel": "console", "expression": "document.title"})
        )
        .await["value"],
        "Debug me"
    );
    assert_eq!(
        run(
            &tools,
            "browser_devtools",
            json!({"panel": "console", "expression": "total([1, 2, 3])"})
        )
        .await["value"],
        6
    );
    let thrown = run(
        &tools,
        "browser_devtools",
        json!({"panel": "console", "expression": "notDefined()"}),
    )
    .await;
    assert!(
        thrown["exception"]
            .as_str()
            .unwrap()
            .contains("ReferenceError"),
        "{thrown}"
    );
    let next = messages["next_since"].as_u64().unwrap();
    run(
        &tools,
        "browser_devtools",
        json!({"panel": "console", "expression": "console.info('later')"}),
    )
    .await;
    let newer = run(
        &tools,
        "browser_devtools",
        json!({"panel": "console", "since": next}),
    )
    .await;
    assert!(
        newer["messages"].to_string().contains("later")
            && !newer["messages"].to_string().contains("booted"),
        "{newer}"
    );

    // Elements: inspect, then change a style live.
    let inspected = run(
        &tools,
        "browser_devtools",
        json!({"panel": "elements", "selector": "#t"}),
    )
    .await;
    assert_eq!(inspected["result"]["tag"], "h1");
    assert_eq!(inspected["result"]["styles"]["color"], "rgb(255, 0, 0)");
    let restyled = run(
        &tools,
        "browser_devtools",
        json!({"panel": "elements", "selector": "#t", "action": "set_style", "name": "color", "value": "blue"}),
    )
    .await;
    assert_eq!(restyled["result"], "rgb(0, 0, 255)");

    // Network: the request is listed; its credential header is hidden, its body shown.
    let listed = run(&tools, "browser_devtools", json!({"panel": "network"})).await;
    let api = listed["requests"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["url"].as_str().unwrap().ends_with("/api/items"))
        .expect("api request listed")
        .clone();
    assert_eq!(api["status"], 200);
    let detail = run(
        &tools,
        "browser_devtools",
        json!({"panel": "network", "id": api["id"]}),
    )
    .await;
    assert!(
        detail["body"].as_str().unwrap().contains("items"),
        "{detail}"
    );
    let authorization = detail["request_headers"]
        .as_object()
        .unwrap()
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("authorization"))
        .unwrap()
        .1
        .clone();
    assert!(
        authorization.as_str().unwrap().starts_with("[hidden"),
        "{authorization}"
    );

    // Storage: ordinary values shown, tokens and cookie values hidden, httpOnly never read.
    let local = run(
        &tools,
        "browser_devtools",
        json!({"panel": "storage", "area": "local"}),
    )
    .await;
    assert_eq!(local["result"]["theme"], "dark");
    assert!(
        local["result"]["authToken"]
            .as_str()
            .unwrap()
            .starts_with("[hidden"),
        "{local}"
    );
    let cookies = run(
        &tools,
        "browser_devtools",
        json!({"panel": "storage", "area": "cookies"}),
    )
    .await;
    let rows = cookies["cookies"].as_array().unwrap();
    let prefs = rows
        .iter()
        .find(|c| c["name"] == "prefs")
        .expect("prefs cookie");
    assert_eq!(prefs["value"], "[hidden: 7 chars]");
    let sid = rows
        .iter()
        .find(|c| c["name"] == "sid")
        .expect("httpOnly cookie listed");
    assert!(sid["value"].as_str().unwrap().contains("httpOnly"), "{sid}");
    assert!(!cookies.to_string().contains("topsecret"));

    // Debugger: a breakpoint inside `total` pauses; the frame's variables and console work.
    let set = run(
        &tools,
        "browser_devtools",
        json!({"panel": "debugger", "action": "set", "url": "app.js", "line": 4}),
    )
    .await;
    assert!(set["breakpoint"].is_string(), "{set}");
    run(
        &tools,
        "browser_devtools",
        json!({"panel": "console", "expression": "setTimeout(() => total([5, 6]), 0); 'scheduled'"}),
    )
    .await;
    let mut state = Value::Null;
    for _ in 0..30 {
        state = run(
            &tools,
            "browser_devtools",
            json!({"panel": "debugger", "action": "state"}),
        )
        .await;
        if state["paused"] == true {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(state["paused"], true, "{state}");
    assert_eq!(state["call_stack"][0]["function"], "total", "{state}");
    assert!(
        state["variables"].as_object().unwrap().contains_key("sum"),
        "{state}"
    );
    assert_eq!(
        run(
            &tools,
            "browser_devtools",
            json!({"panel": "console", "expression": "items.length"})
        )
        .await["value"],
        2
    );
    run(
        &tools,
        "browser_devtools",
        json!({"panel": "debugger", "action": "remove", "id": set["breakpoint"]}),
    )
    .await;
    let resumed = run(
        &tools,
        "browser_devtools",
        json!({"panel": "debugger", "action": "resume"}),
    )
    .await;
    assert_eq!(resumed["paused"], false, "{resumed}");

    // A promise is awaited, as in the DevTools console.
    let awaited = run(
        &tools,
        "browser_devtools",
        json!({"panel": "console", "expression": "Promise.resolve(5)"}),
    )
    .await;
    assert_eq!(awaited["value"], 5, "{awaited}");
    // Pages never get the camera, microphone, location or notifications.
    let denied = run(&tools, "browser_devtools", json!({"panel": "console", "expression": "Promise.all(['geolocation','notifications','camera','microphone'].map(n => navigator.permissions.query({name: n}).then(p => p.state, () => 'denied')))"})).await;
    assert_eq!(
        denied["value"],
        json!(["denied", "denied", "denied", "denied"]),
        "{denied}"
    );

    // Performance and memory, with their files saved.
    let metrics = run(&tools, "browser_devtools", json!({"panel": "performance"})).await;
    assert!(
        metrics["page"]["resources"].as_u64().unwrap() >= 2,
        "{metrics}"
    );
    let trace = run(
        &tools,
        "browser_devtools",
        json!({"panel": "performance", "action": "trace", "seconds": 1}),
    )
    .await;
    assert!(trace["events"].as_u64().unwrap() > 0, "{trace}");
    assert!(std::path::Path::new(trace["file"].as_str().unwrap()).exists());
    run(&tools, "browser_devtools", json!({"panel": "memory"})).await;
    let again = run(&tools, "browser_devtools", json!({"panel": "memory"})).await;
    assert!(again["growth_since_last_summary"].is_object(), "{again}");
    let snapshot = run(
        &tools,
        "browser_devtools",
        json!({"panel": "memory", "action": "snapshot"}),
    )
    .await;
    assert!(snapshot["objects"].as_u64().unwrap() > 0, "{snapshot}");
    assert!(std::path::Path::new(snapshot["file"].as_str().unwrap()).exists());

    // The run's end clears breakpoints: a breakpoint set now no longer pauses afterwards.
    let set = run(
        &tools,
        "browser_devtools",
        json!({"panel": "debugger", "action": "set", "url": "app.js", "line": 4}),
    )
    .await;
    assert!(set["breakpoint"].is_string());
    chat.end_debugging().await;
    run(
        &tools,
        "browser_devtools",
        json!({"panel": "console", "expression": "total([1])"}),
    )
    .await;
    let after = run(
        &tools,
        "browser_devtools",
        json!({"panel": "debugger", "action": "state"}),
    )
    .await;
    assert_eq!(after["paused"], false, "{after}");

    // With Developer mode off, no debugging tool is offered at all.
    let off = tools_for(
        &chat,
        media.path(),
        BrowserSettings {
            developer_mode: false,
            ..BrowserSettings::default()
        },
    );
    assert!(off.iter().all(|tool| tool.name() != "browser_devtools"));
    assert!(off.iter().any(|tool| tool.name() == "browser_open"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_download_the_agent_starts_lands_in_the_settings_folder() {
    let data = tempfile::tempdir().unwrap();
    let hub = BrowserHub::new(BrowserConfig {
        root: data.path().to_path_buf(),
        executable: None,
        headless: true,
    });
    if hub.executable().is_none() {
        eprintln!("skipping: no Chromium browser installed");
        return;
    }
    let site = MockServer::start().await;
    Mock::given(path("/files"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            r#"<!doctype html><title>Files</title><a href="/report.csv" download>Get the report</a>"#,
            "text/html",
        ))
        .mount(&site)
        .await;
    Mock::given(path("/report.csv"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Disposition", "attachment; filename=\"report.csv\"")
                .set_body_raw("a,b\n1,2\n", "text/csv"),
        )
        .mount(&site)
        .await;
    let project = tempfile::tempdir().unwrap();
    let media = tempfile::tempdir().unwrap();
    let downloads = tempfile::tempdir().unwrap();
    let chat = hub.chat("download-chat", project.path());
    let settings = BrowserSettings {
        downloads_directory: Some(downloads.path().to_string_lossy().into_owned()),
        ..BrowserSettings::default()
    };
    let tools = tools_for(&chat, media.path(), settings);
    let opened = run(
        &tools,
        "browser_open",
        json!({"url": format!("{}/files", site.uri())}),
    )
    .await;
    let snapshot = opened["snapshot"].as_str().unwrap();
    let line = snapshot
        .lines()
        .find(|l| l.contains("Get the report"))
        .unwrap();
    let reference = line.split('[').nth(1).unwrap().split(']').next().unwrap();
    let clicked = run(&tools, "browser_click", json!({"ref": reference})).await;
    // The download may still be starting when the click returns; it is saved either way.
    let saved = downloads.path().join("report.csv");
    for _ in 0..50 {
        if saved.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(
        std::fs::read_to_string(&saved).unwrap(),
        "a,b\n1,2\n",
        "click result: {clicked}"
    );
    if let Some(reported) = clicked["downloads"].as_array() {
        assert!(
            reported[0]["saved_to"]
                .as_str()
                .unwrap()
                .ends_with("report.csv"),
            "{clicked}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_download_nobody_allowed_is_cancelled() {
    let data = tempfile::tempdir().unwrap();
    let hub = BrowserHub::new(BrowserConfig {
        root: data.path().to_path_buf(),
        executable: None,
        headless: true,
    });
    if hub.executable().is_none() {
        eprintln!("skipping: no Chromium browser installed");
        return;
    }
    let site = MockServer::start().await;
    Mock::given(path("/files"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            r#"<!doctype html><title>Files</title><a id="get" href="/report.csv" download>Get</a>"#,
            "text/html",
        ))
        .mount(&site)
        .await;
    Mock::given(path("/report.csv"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Disposition", "attachment; filename=\"report.csv\"")
                .set_body_raw("a\n", "text/csv"),
        )
        .mount(&site)
        .await;
    let project = tempfile::tempdir().unwrap();
    let downloads = tempfile::tempdir().unwrap();
    let chat = hub.chat("refusing-chat", project.path());
    // A run whose Downloads permission refuses (as Block or a denied Ask would).
    chat.set_download_policy(Some(themis_core::browser::session::DownloadPolicy {
        directory: downloads.path().to_path_buf(),
        gate: Arc::new(|_site: &str| Err("Downloads is blocked".to_owned())),
    }));
    let started = std::time::Instant::now();
    chat.open(&format!("{}/files", site.uri()), false)
        .await
        .unwrap();
    let clicked = chat
        .evaluate_for_test("document.getElementById('get').click()")
        .await;
    assert!(clicked.is_ok(), "{clicked:?}");
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let reported = chat.agent_downloads_for_test(started).await;
    assert!(
        reported
            .iter()
            .any(|d| d["refused"].as_str().is_some_and(|r| r.contains("blocked"))),
        "{reported:?}"
    );
    assert!(!downloads.path().join("report.csv").exists());
    // With no run's policy at all, a page's own download is cancelled too.
    chat.set_download_policy(None);
    let started = std::time::Instant::now();
    chat.evaluate_for_test("document.getElementById('get').click()")
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let reported = chat.agent_downloads_for_test(started).await;
    assert!(
        reported
            .iter()
            .any(|d| d["refused"].as_str().is_some_and(|r| r.contains("no run"))),
        "{reported:?}"
    );
}
