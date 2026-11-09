//! The browser's `devtools` action against a real Chromium and a local app: console messages
//! and expressions, elements, network with hidden credentials, storage, a breakpoint with
//! stepping and frame evaluation, performance and memory. Skipped without a Chromium.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use themis_core::browser::launch::BrowserProcess;
use themis_core::browser::{BrowserConfig, BrowserHub, ChatBrowser};
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
            screenshots: Default::default(),
        },
    )
}

/// The browser tool's `action` with `args`, as a model calls it.
async fn run(tools: &[Box<dyn ToolT>], action: &str, mut args: Value) -> Value {
    let [tool] = tools else {
        panic!("one browser tool, not {}", tools.len())
    };
    assert!(args.get("action").is_none(), "{args}");
    args["action"] = json!(action);
    tool.execute(args)
        .await
        .unwrap_or_else(|error| panic!("{action}: {error}"))
}

/// The actions the browser tool offers.
fn offered(tools: &[Box<dyn ToolT>]) -> Vec<String> {
    serde_json::from_value(tools[0].args_schema()["properties"]["action"]["enum"].clone()).unwrap()
}

/// One browser at a time: three Chromes starting together on a loaded CI machine can miss
/// the startup limit.
static ONE_BROWSER: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
static WARMED: tokio::sync::OnceCell<()> = tokio::sync::OnceCell::const_new();

/// Starts `executable` once per test process before any test times its own start. The first
/// Chrome start on a fresh CI machine is cold: on Windows runners it took 27 to 53 s in about
/// one run in eight and sometimes passed the 45 s startup limit, failing whichever test took
/// the lock first. A start that runs out of time still warms the machine, so it is tried
/// again (each try has the product's own limit); later starts take a few seconds.
async fn warm_up(executable: &std::path::Path) {
    WARMED
        .get_or_init(|| async {
            for attempt in 1..=3 {
                let profile = tempfile::tempdir().unwrap();
                match BrowserProcess::launch(executable, profile.path(), true).await {
                    Ok(_browser) => return,
                    Err(error) => eprintln!("warm-up start {attempt} failed: {error}"),
                }
            }
        })
        .await;
}

/// `check` of the browser's state, asked again until it holds or `limit` runs out; the
/// last answer either way. Browser events arrive whenever the page and machine get to them.
async fn eventually<T>(limit: Duration, mut check: impl AsyncFnMut() -> (bool, T)) -> T {
    let deadline = tokio::time::Instant::now() + limit;
    loop {
        let (done, value) = check().await;
        if done || tokio::time::Instant::now() >= deadline {
            return value;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// A browser hub with its own data folder, after the one-time warm-up; `None` (the test is
/// skipped) without a Chromium browser.
async fn started_hub(data: &tempfile::TempDir) -> Option<Arc<BrowserHub>> {
    let hub = BrowserHub::new(BrowserConfig {
        root: data.path().to_path_buf(),
        executable: None,
        headless: true,
    });
    let Some(executable) = hub.executable() else {
        eprintln!("skipping: no Chromium browser installed");
        return None;
    };
    warm_up(&executable).await;
    Some(hub)
}

#[tokio::test(flavor = "multi_thread")]
async fn the_agent_debugs_a_local_app_with_the_devtools_toolset() {
    let _browser = ONE_BROWSER.lock().await;
    let data = tempfile::tempdir().unwrap();
    let Some(hub) = started_hub(&data).await else {
        return;
    };
    let site = MockServer::builder().start().await;
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
        "open",
        json!({"url": format!("{}/app", site.uri())}),
    )
    .await;
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Console: every level, then expressions and exceptions.
    let messages = run(&tools, "devtools", json!({"panel": "console"})).await;
    let text = messages["messages"].to_string();
    assert!(
        text.contains("booted 42") && text.contains("\"level\":\"warning\"")
            || text.contains("\"level\":\"warn\""),
        "{messages}"
    );
    assert_eq!(
        run(
            &tools,
            "devtools",
            json!({"panel": "console", "expression": "document.title"})
        )
        .await["value"],
        "Debug me"
    );
    assert_eq!(
        run(
            &tools,
            "devtools",
            json!({"panel": "console", "expression": "total([1, 2, 3])"})
        )
        .await["value"],
        6
    );
    let thrown = run(
        &tools,
        "devtools",
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
        "devtools",
        json!({"panel": "console", "expression": "console.info('later')"}),
    )
    .await;
    // The message arrives as a browser event shortly after the expression returns.
    let mut newer = Value::Null;
    for _ in 0..20 {
        newer = run(
            &tools,
            "devtools",
            json!({"panel": "console", "since": next}),
        )
        .await;
        if newer["messages"].to_string().contains("later") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(
        newer["messages"].to_string().contains("later")
            && !newer["messages"].to_string().contains("booted"),
        "{newer}"
    );

    // Elements: inspect, then change a style live.
    let inspected = run(
        &tools,
        "devtools",
        json!({"panel": "elements", "selector": "#t"}),
    )
    .await;
    assert_eq!(inspected["result"]["tag"], "h1");
    assert_eq!(inspected["result"]["styles"]["color"], "rgb(255, 0, 0)");
    let restyled = run(
        &tools,
        "devtools",
        json!({"panel": "elements", "selector": "#t", "operation": "set_style", "name": "color", "value": "blue"}),
    )
    .await;
    assert_eq!(restyled["result"], "rgb(0, 0, 255)");

    // Network: the request is listed; its credential header is hidden, its body shown.
    let listed = run(&tools, "devtools", json!({"panel": "network"})).await;
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
        "devtools",
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
        "devtools",
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
        "devtools",
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
        "devtools",
        json!({"panel": "debugger", "operation": "set", "url": "app.js", "line": 4}),
    )
    .await;
    assert!(set["breakpoint"].is_string(), "{set}");
    run(
        &tools,
        "devtools",
        json!({"panel": "console", "expression": "setTimeout(() => total([5, 6]), 0); 'scheduled'"}),
    )
    .await;
    let mut state = Value::Null;
    for _ in 0..30 {
        state = run(
            &tools,
            "devtools",
            json!({"panel": "debugger", "operation": "state"}),
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
            "devtools",
            json!({"panel": "console", "expression": "items.length"})
        )
        .await["value"],
        2
    );
    run(
        &tools,
        "devtools",
        json!({"panel": "debugger", "operation": "remove", "id": set["breakpoint"]}),
    )
    .await;
    let resumed = run(
        &tools,
        "devtools",
        json!({"panel": "debugger", "operation": "resume"}),
    )
    .await;
    assert_eq!(resumed["paused"], false, "{resumed}");

    // The panel repeated as the action is the panel's default.
    let repeated = run(
        &tools,
        "devtools",
        json!({"panel": "console", "operation": "console", "expression": "1 + 1"}),
    )
    .await;
    assert_eq!(repeated["value"], 2, "{repeated}");
    // A promise is awaited, as in the DevTools console.
    let awaited = run(
        &tools,
        "devtools",
        json!({"panel": "console", "expression": "Promise.resolve(5)"}),
    )
    .await;
    assert_eq!(awaited["value"], 5, "{awaited}");
    // Pages never get the camera, microphone, location or notifications.
    let denied = run(&tools, "devtools", json!({"panel": "console", "expression": "Promise.all(['geolocation','notifications','camera','microphone'].map(n => navigator.permissions.query({name: n}).then(p => p.state, () => 'denied')))"})).await;
    assert_eq!(
        denied["value"],
        json!(["denied", "denied", "denied", "denied"]),
        "{denied}"
    );

    // Performance and memory, with their files saved.
    let metrics = run(&tools, "devtools", json!({"panel": "performance"})).await;
    assert!(
        metrics["page"]["resources"].as_u64().unwrap() >= 2,
        "{metrics}"
    );
    let trace = run(
        &tools,
        "devtools",
        json!({"panel": "performance", "operation": "trace", "seconds": 1}),
    )
    .await;
    assert!(trace["events"].as_u64().unwrap() > 0, "{trace}");
    assert!(std::path::Path::new(trace["file"].as_str().unwrap()).exists());
    run(&tools, "devtools", json!({"panel": "memory"})).await;
    let again = run(&tools, "devtools", json!({"panel": "memory"})).await;
    assert!(again["growth_since_last_summary"].is_object(), "{again}");
    let snapshot = run(
        &tools,
        "devtools",
        json!({"panel": "memory", "operation": "snapshot"}),
    )
    .await;
    assert!(snapshot["objects"].as_u64().unwrap() > 0, "{snapshot}");
    assert!(std::path::Path::new(snapshot["file"].as_str().unwrap()).exists());

    // The run's end clears breakpoints: a breakpoint set now no longer pauses afterwards.
    let set = run(
        &tools,
        "devtools",
        json!({"panel": "debugger", "operation": "set", "url": "app.js", "line": 4}),
    )
    .await;
    assert!(set["breakpoint"].is_string());
    chat.end_debugging().await;
    run(
        &tools,
        "devtools",
        json!({"panel": "console", "expression": "total([1])"}),
    )
    .await;
    let after = run(
        &tools,
        "devtools",
        json!({"panel": "debugger", "operation": "state"}),
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
    assert!(!offered(&off).contains(&"devtools".to_owned()));
    assert!(offered(&off).contains(&"open".to_owned()));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_download_the_agent_starts_lands_in_the_settings_folder() {
    let _browser = ONE_BROWSER.lock().await;
    let data = tempfile::tempdir().unwrap();
    let Some(hub) = started_hub(&data).await else {
        return;
    };
    let site = MockServer::builder().start().await;
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
        "open",
        json!({"url": format!("{}/files", site.uri())}),
    )
    .await;
    let snapshot = opened["snapshot"].as_str().unwrap();
    let line = snapshot
        .lines()
        .find(|l| l.contains("Get the report"))
        .unwrap();
    let reference = line.split('[').nth(1).unwrap().split(']').next().unwrap();
    let clicked = run(&tools, "click", json!({"ref": reference})).await;
    // The download may still be starting when the click returns; it is saved either way.
    let saved = downloads.path().join("report.csv");
    let contents = eventually(Duration::from_secs(20), async || {
        let contents = std::fs::read_to_string(&saved).ok();
        (contents.as_deref() == Some("a,b\n1,2\n"), contents)
    })
    .await;
    assert_eq!(
        contents.as_deref(),
        Some("a,b\n1,2\n"),
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
    let _browser = ONE_BROWSER.lock().await;
    let data = tempfile::tempdir().unwrap();
    let Some(hub) = started_hub(&data).await else {
        return;
    };
    let site = MockServer::builder().start().await;
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
    chat.open(&format!("{}/files", site.uri())).await.unwrap();
    let clicked = chat
        .evaluate_for_test("document.getElementById('get').click()")
        .await;
    assert!(clicked.is_ok(), "{clicked:?}");
    let reported = refused_since(&chat, started, "blocked").await;
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
    let reported = refused_since(&chat, started, "no run").await;
    assert!(
        reported
            .iter()
            .any(|d| d["refused"].as_str().is_some_and(|r| r.contains("no run"))),
        "{reported:?}"
    );
}

/// The agent's downloads since `since`, once one was refused with a reason containing
/// `reason` (or after 20 s): the click's download event reaches Themis when it does.
async fn refused_since(chat: &ChatBrowser, since: std::time::Instant, reason: &str) -> Vec<Value> {
    eventually(Duration::from_secs(20), async || {
        let reported = chat.agent_downloads_for_test(since).await;
        let refused = reported
            .iter()
            .any(|d| d["refused"].as_str().is_some_and(|r| r.contains(reason)));
        (refused, reported)
    })
    .await
}
