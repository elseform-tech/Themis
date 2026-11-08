//! The built-in browser through the real shared server and the `themis` CLI:
//! a scripted model opens a local page, clicks a ref from the snapshot and a
//! point of the page, checks how the page looks, a site rule refuses another
//! host, and the Workspace commands (state, open an address, sound) answer for
//! the same chat. Skipped without a Chromium.

use std::path::Path;
use std::time::Duration;

use serde_json::{json, Value};
use themis_desktop::{server::Server, state::AppState, types::ProviderKind};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

fn call(data: &Path, command: &str, args: Value) -> Value {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
        .arg("--data-dir")
        .arg(data)
        .args(["call", command, &args.to_string()])
        .env_remove("OPENCODE_KEY")
        .output()
        .expect("CLI");
    assert!(
        output.status.success(),
        "{command}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap_or(Value::Null)
}

fn tool_call(id: &str, name: &str, arguments: Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "choices": [{"message": {"role": "assistant", "content": null, "tool_calls": [{
            "id": id, "type": "function",
            "function": {"name": name, "arguments": arguments.to_string()}
        }]}}]
    }))
}

fn tool_results(body: &Value) -> Vec<Value> {
    body["messages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|message| message["role"] == "tool")
        .map(|message| {
            serde_json::from_str(message["content"].as_str().unwrap_or_default())
                .unwrap_or_else(|_| message["content"].clone())
        })
        .collect()
}

/// The ref right after `name` in a snapshot line.
fn ref_after(snapshot: &str, name: &str) -> Option<String> {
    let line = snapshot.lines().find(|line| line.contains(name))?;
    let after = &line[line.find(name)? + name.len()..];
    let reference = after.trim_start().strip_prefix('[')?.split(']').next()?;
    reference.starts_with('e').then(|| reference.to_owned())
}

#[tokio::test(flavor = "multi_thread")]
async fn cli_run_drives_the_browser_and_the_workspace_commands_see_it() {
    if themis_core::browser::launch::find_browser(None, None).is_none() {
        eprintln!("skipping: no Chromium browser installed");
        return;
    }
    let site = MockServer::builder().start().await;
    let port = site.address().port();
    Mock::given(path("/app"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            "<!doctype html><title>Counter</title><h1>Counter</h1><p id=n>0</p><button onclick=\"n.textContent = Number(n.textContent) + 1\">Add one</button>",
            "text/html; charset=utf-8",
        ))
        .mount(&site)
        .await;
    let llm = MockServer::builder().start().await;
    Mock::given(method("POST"))
        .respond_with(move |request: &Request| {
            let body: Value = request.body_json().unwrap_or(Value::Null);
            let results = tool_results(&body);
            match results.len() {
                0 => tool_call("c1", "browser", json!({"action": "open", "url": format!("http://127.0.0.1:{port}/app")})),
                1 => {
                    let snapshot = results[0]["snapshot"].as_str().unwrap_or_default();
                    let reference = ref_after(snapshot, "\"Add one\"").unwrap_or_else(|| "e0".into());
                    tool_call("c2", "browser", json!({"action": "click", "ref": reference}))
                }
                2 => tool_call("c3", "browser", json!({"action": "open", "url": format!("http://localhost:{port}/app")})),
                3 => tool_call("c4", "browser", json!({"action": "screenshot"})),
                4 => tool_call(
                    "c5",
                    "attach_file",
                    json!({"files": [{"path": results[3]["path"], "caption": "Counter at one"}]}),
                ),
                // A point of the page, as a canvas game would need, and one outside it.
                5 => tool_call("c6", "browser", json!({"action": "click", "x": 5, "y": 5})),
                6 => tool_call("c7", "browser", json!({"action": "click", "x": 5, "y": 100_000})),
                7 => tool_call("c8", "browser", json!({"action": "check_looks"})),
                // A short recording of its own, which the chat shows as a video.
                8 => tool_call("c9", "browser", json!({"action": "record", "operation": "start"})),
                9 => tool_call("c10", "browser", json!({"action": "click", "x": 5, "y": 5})),
                10 => tool_call("c11", "browser", json!({"action": "record", "operation": "stop"})),
                _ => ResponseTemplate::new(200).set_body_json(json!({
                    "choices": [{"message": {"role": "assistant", "content": "Clicked it."}, "finish_reason": "stop"}]
                })),
            }
        })
        .mount(&llm)
        .await;

    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    std::fs::write(
        data.path().join("runtime.jsonc"),
        json!({
            "schema_version": 1,
            "approval": {"rules": [
                {"tool": "browser", "command": ["localhost"], "action": "deny"},
                {"tool": "browser", "command": ["127.0.0.1"], "action": "allow"},
                {"tool": "*", "risk": "read", "action": "allow"}
            ]}
        })
        .to_string(),
    )
    .unwrap();
    let state = AppState::new_for_test(data.path().join("settings.json"));
    state
        .set_secret("go".into(), "synthetic-test-key".into())
        .await
        .unwrap();
    state.set_go_base_url_override(Some(llm.uri()));
    let thread = state
        .create_thread(
            project.path().to_string_lossy().into(),
            ProviderKind::Go,
            Some("test-model".into()),
        )
        .await
        .unwrap();
    let server = Server::bind(state.clone(), data.path()).await.unwrap();
    let task = tokio::spawn(server.run());
    let data_dir = data.path().to_owned();
    let thread_id = thread.id.clone();
    {
        let data_dir = data_dir.clone();
        let thread_id = thread_id.clone();
        tokio::task::spawn_blocking(move || {
            call(
                &data_dir,
                "send_message",
                json!({"threadId": thread_id, "text": "Click Add one on the app."}),
            );
        })
        .await
        .unwrap();
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(40);
    loop {
        let done = llm
            .received_requests()
            .await
            .unwrap_or_default()
            .iter()
            .any(|request| tool_results(&request.body_json().unwrap_or(Value::Null)).len() >= 11);
        if done && !state.get_thread(&thread.id).await.is_ok_and(|t| t.running) {
            break;
        }
        if std::time::Instant::now() > deadline {
            let requests = llm.received_requests().await.unwrap_or_default();
            let last = requests
                .last()
                .map(|request| tool_results(&request.body_json().unwrap_or(Value::Null)));
            let pending = state.pending_approvals();
            panic!(
                "run did not finish; last tool results: {last:?}; pending approvals: {pending:?}"
            );
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let last: Value = llm
        .received_requests()
        .await
        .unwrap()
        .last()
        .unwrap()
        .body_json()
        .unwrap();
    // One browser tool, its action picking what to do.
    let browser = last["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["function"]["name"] == "browser")
        .expect("the browser tool is offered");
    let actions = browser["function"]["parameters"]["properties"]["action"]["enum"].to_string();
    for name in [
        "open",
        "click",
        "type",
        "snapshot",
        "check_looks",
        "screenshot",
        "record",
    ] {
        assert!(
            actions.contains(&format!("\"{name}\"")),
            "{name} not offered: {actions}"
        );
    }
    assert!(
        !last["tools"].to_string().contains("\"browser_open\""),
        "no browser tool of old"
    );
    let results = tool_results(&last);
    assert_eq!(results[0]["title"], "Counter", "{}", results[0]);
    assert!(
        results[1]["snapshot"].as_str().unwrap().contains("text: 1"),
        "{}",
        results[1]
    );
    assert!(
        results[2].to_string().contains("approval denied"),
        "{}",
        results[2]
    );
    let shot = results[3]["path"].as_str().unwrap();
    assert!(
        shot.replace('\\', "/").contains("/.themis/attachments/") && shot.ends_with(".png"),
        "{shot}"
    );
    assert_eq!(
        results[4]["attached"][0]["caption"], "Counter at one",
        "{}",
        results[4]
    );
    assert!(
        results[5]["snapshot"].as_str().unwrap().contains("text: 1"),
        "{}",
        results[5]
    );
    assert!(
        results[6].to_string().contains("outside the visible page"),
        "{}",
        results[6]
    );
    // The plain counter page looks right in both colour schemes.
    assert_eq!(results[7]["problems"], 0, "{}", results[7]);
    assert!(
        results[7]["checked"]
            .as_str()
            .unwrap()
            .ends_with("in light and dark"),
        "{}",
        results[7]
    );
    let recording = results[10]["path"].as_str().unwrap().to_owned();
    assert!(recording.ends_with(".mp4"), "{}", results[10]);
    let shot = shot.to_owned();

    tokio::task::spawn_blocking(move || {
        let browser = call(&data_dir, "browser_state", json!({"threadId": thread_id}));
        assert_eq!(browser["open"], true, "{browser}");
        assert_eq!(browser["available"], true);
        assert!(
            browser["url"].as_str().unwrap().ends_with("/app"),
            "{browser}"
        );
        assert_eq!(browser["controller"], "idle");
        // The person opening an address never takes the browser from the agent.
        let address = browser["url"].as_str().unwrap().to_owned();
        call(
            &data_dir,
            "browser_open_url",
            json!({"threadId": thread_id, "address": address}),
        );
        assert_eq!(
            call(&data_dir, "browser_state", json!({"threadId": thread_id}))["controller"],
            "idle"
        );
        // The speaker starts off and the person can turn it on.
        assert_eq!(browser["muted"], true, "{browser}");
        call(
            &data_dir,
            "browser_set_muted",
            json!({"threadId": thread_id, "muted": false}),
        );
        assert_eq!(
            call(&data_dir, "browser_state", json!({"threadId": thread_id}))["muted"],
            false
        );
        // The reply's media is saved with the conversation.
        let history = call(
            &data_dir,
            "get_thread_history",
            json!({"threadId": thread_id}),
        )
        .to_string();
        assert!(
            history.contains("media_attached") && history.contains("Counter at one"),
            "{history}"
        );
        // The screenshot and recording steps carry what they saved as structured media,
        // so the chat shows them without reading the tool's output.
        let history: Value = serde_json::from_str(&history).unwrap();
        let media: Vec<Value> = history
            .as_array()
            .unwrap()
            .iter()
            .map(|item| &item["envelope"]["event"])
            .filter(|event| event["kind"] == "tool_finished" && event["tool"] == "browser")
            .map(|event| event["media"].clone())
            .filter(|media| !media.is_null())
            .collect();
        let media_of = |kind: &str| {
            media
                .iter()
                .find(|media| media["kind"] == kind)
                .cloned()
                .unwrap_or(Value::Null)
        };
        let shot_media = media_of("image");
        assert_eq!(shot_media["kind"], "image", "{shot_media}");
        assert_eq!(shot_media["path"], shot.as_str(), "{shot_media}");
        assert!(shot_media["width"].as_u64().unwrap() > 0, "{shot_media}");
        assert!(shot_media["height"].as_u64().unwrap() > 0, "{shot_media}");
        let video_media = media_of("video");
        assert_eq!(video_media["kind"], "video", "{video_media}");
        assert_eq!(video_media["path"], recording.as_str(), "{video_media}");
        // Other steps carry none.
        assert_eq!(media.len(), 2, "{media:?}");
        // The person's Capture and Record save files the composer can attach.
        let captured = call(&data_dir, "browser_capture", json!({"threadId": thread_id}));
        assert!(
            captured["name"]
                .as_str()
                .unwrap()
                .starts_with("screenshot-"),
            "{captured}"
        );
        assert!(std::path::Path::new(captured["path"].as_str().unwrap()).is_file());
        call(
            &data_dir,
            "browser_record",
            json!({"threadId": thread_id, "action": "start"}),
        );
        assert_eq!(
            call(&data_dir, "browser_state", json!({"threadId": thread_id}))["recording"]["by"],
            "user"
        );
        std::thread::sleep(Duration::from_millis(700));
        let video = call(
            &data_dir,
            "browser_record",
            json!({"threadId": thread_id, "action": "stop"}),
        );
        assert!(video["name"].as_str().unwrap().ends_with(".mp4"), "{video}");
        let devtools = call(
            &data_dir,
            "browser_devtools_url",
            json!({"threadId": thread_id}),
        );
        assert!(
            devtools
                .as_str()
                .unwrap()
                .contains("/devtools/devtools_app.html?ws="),
            "{devtools}"
        );
        // The Browser tab opening mid-task streams at its pixel density without laying the
        // agent's page out again; a resize by the person does. Captures are in CSS pixels.
        let captured_width = || {
            let captured = call(&data_dir, "browser_capture", json!({"threadId": thread_id}));
            let png = std::fs::read(captured["path"].as_str().unwrap()).unwrap();
            u32::from_be_bytes(png[16..20].try_into().unwrap())
        };
        let before = captured_width();
        call(
            &data_dir,
            "browser_screencast",
            json!({"threadId": thread_id, "width": 700, "height": 500, "scale": 2.0, "resize": false}),
        );
        assert_eq!(captured_width(), before, "the page kept its size");
        call(
            &data_dir,
            "browser_screencast",
            json!({"threadId": thread_id, "width": 700, "height": 500, "scale": 2.0, "resize": true}),
        );
        assert_eq!(captured_width(), 700, "the page follows a resized panel");
        // Closing Workspace and showing the preview answer at once, in turn.
        call(
            &data_dir,
            "browser_screencast_stop",
            json!({"threadId": thread_id}),
        );
        call(
            &data_dir,
            "browser_preview",
            json!({"threadId": thread_id, "width": 440}),
        );
        call(
            &data_dir,
            "browser_screencast_stop",
            json!({"threadId": thread_id}),
        );
        // Discarding the chat closes its browser.
        call(&data_dir, "discard_thread", json!({"threadId": thread_id}));
    })
    .await
    .unwrap();
    task.abort();
}
