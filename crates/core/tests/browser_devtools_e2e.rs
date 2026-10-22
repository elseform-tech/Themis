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
    let messages = run(&tools, "browser_console", json!({})).await;
    let text = messages["messages"].to_string();
    assert!(
        text.contains("booted 42") && text.contains("\"level\":\"warning\"")
            || text.contains("\"level\":\"warn\""),
        "{messages}"
    );
    assert_eq!(
        run(
            &tools,
            "browser_console",
            json!({"expression": "document.title"})
        )
        .await["value"],
        "Debug me"
    );
    assert_eq!(
        run(
            &tools,
            "browser_console",
            json!({"expression": "total([1, 2, 3])"})
        )
        .await["value"],
        6
    );
    let thrown = run(
        &tools,
        "browser_console",
        json!({"expression": "notDefined()"}),
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
        "browser_console",
        json!({"expression": "console.info('later')"}),
    )
    .await;
    let newer = run(&tools, "browser_console", json!({"since": next})).await;
    assert!(
        newer["messages"].to_string().contains("later")
            && !newer["messages"].to_string().contains("booted"),
        "{newer}"
    );

    // Elements: inspect, then change a style live.
    let inspected = run(&tools, "browser_elements", json!({"selector": "#t"})).await;
    assert_eq!(inspected["result"]["tag"], "h1");
    assert_eq!(inspected["result"]["styles"]["color"], "rgb(255, 0, 0)");
    let restyled = run(
        &tools,
        "browser_elements",
        json!({"selector": "#t", "action": "set_style", "name": "color", "value": "blue"}),
    )
    .await;
    assert_eq!(restyled["result"], "rgb(0, 0, 255)");

    // Network: the request is listed; its credential header is hidden, its body shown.
    let listed = run(&tools, "browser_network", json!({})).await;
    let api = listed["requests"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["url"].as_str().unwrap().ends_with("/api/items"))
        .expect("api request listed")
        .clone();
    assert_eq!(api["status"], 200);
    let detail = run(&tools, "browser_network", json!({"id": api["id"]})).await;
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
    let local = run(&tools, "browser_storage", json!({"area": "local"})).await;
    assert_eq!(local["result"]["theme"], "dark");
    assert!(
        local["result"]["authToken"]
            .as_str()
            .unwrap()
            .starts_with("[hidden"),
        "{local}"
    );
    let cookies = run(&tools, "browser_storage", json!({"area": "cookies"})).await;
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
        "browser_debugger",
        json!({"action": "set", "url": "app.js", "line": 4}),
    )
    .await;
    assert!(set["breakpoint"].is_string(), "{set}");
    run(
        &tools,
        "browser_console",
        json!({"expression": "setTimeout(() => total([5, 6]), 0); 'scheduled'"}),
    )
    .await;
    let mut state = Value::Null;
    for _ in 0..30 {
        state = run(&tools, "browser_debugger", json!({"action": "state"})).await;
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
            "browser_console",
            json!({"expression": "items.length"})
        )
        .await["value"],
        2
    );
    run(
        &tools,
        "browser_debugger",
        json!({"action": "remove", "id": set["breakpoint"]}),
    )
    .await;
    let resumed = run(&tools, "browser_debugger", json!({"action": "resume"})).await;
    assert_eq!(resumed["paused"], false, "{resumed}");

    // Performance and memory, with their files saved.
    let metrics = run(&tools, "browser_performance", json!({})).await;
    assert!(
        metrics["page"]["resources"].as_u64().unwrap() >= 2,
        "{metrics}"
    );
    let trace = run(
        &tools,
        "browser_performance",
        json!({"action": "trace", "seconds": 1}),
    )
    .await;
    assert!(trace["events"].as_u64().unwrap() > 0, "{trace}");
    assert!(std::path::Path::new(trace["file"].as_str().unwrap()).exists());
    run(&tools, "browser_memory", json!({})).await;
    let again = run(&tools, "browser_memory", json!({})).await;
    assert!(again["growth_since_last_summary"].is_object(), "{again}");
    let snapshot = run(&tools, "browser_memory", json!({"action": "snapshot"})).await;
    assert!(snapshot["objects"].as_u64().unwrap() > 0, "{snapshot}");
    assert!(std::path::Path::new(snapshot["file"].as_str().unwrap()).exists());

    // The run's end clears breakpoints: a breakpoint set now no longer pauses afterwards.
    let set = run(
        &tools,
        "browser_debugger",
        json!({"action": "set", "url": "app.js", "line": 4}),
    )
    .await;
    assert!(set["breakpoint"].is_string());
    chat.end_debugging().await;
    run(
        &tools,
        "browser_console",
        json!({"expression": "total([1])"}),
    )
    .await;
    let after = run(&tools, "browser_debugger", json!({"action": "state"})).await;
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
    assert!(off
        .iter()
        .all(|tool| !tool.name().starts_with("browser_console")));
    assert!(off.iter().any(|tool| tool.name() == "browser_open"));
}
