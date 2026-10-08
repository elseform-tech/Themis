use serde_json::{json, Value};
use std::{
    path::Path,
    sync::{Arc, Mutex},
};
use themis_desktop::{
    server::{Client, Server},
    state::AppState,
};
use wiremock::{matchers::method, Mock, MockServer, Request, ResponseTemplate};

fn cli(dir: &Path, command: &str, args: Value) -> Value {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
        .arg("--data-dir")
        .arg(dir)
        .args(["call", command, &args.to_string()])
        .env_remove("OPENCODE_KEY")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn cli_large_file_upload_retries_failed_sections_and_merge_without_repeating_successes() {
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    assert!(std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(project.path())
        .status()
        .unwrap()
        .success());
    let sources = tempfile::tempdir().unwrap();
    let text = format!(
        "BEGIN-17{}MIDDLE-42 ARCHIVE-SECRET-753{}END-99",
        " apple".repeat(105_001),
        " apple".repeat(105_001)
    );
    assert!(text.len().div_ceil(4) > 200_000);
    std::fs::write(sources.path().join("control.txt"), &text).unwrap();
    let binary = [0, 255, 128, 1, 2];
    for name in [
        "photo.png",
        "music.mp3",
        "movie.mp4",
        "archive.zip",
        "[[skill:missing]].bin",
    ] {
        std::fs::write(sources.path().join(name), binary).unwrap();
    }
    let state = AppState::new_for_test(data.path().join("settings.json"));
    let provider = MockServer::start().await;
    let summaries = Arc::new(Mutex::new(Vec::new()));
    let captured = summaries.clone();
    let attempts = Arc::new(Mutex::new(std::collections::HashMap::<String, usize>::new()));
    let observed_attempts = Arc::clone(&attempts);
    Mock::given(method("POST")).respond_with(move |request: &Request| {
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        let summary = body["messages"][0]["content"].as_str().unwrap().contains("You summarize agent context");
        let answer = if summary {
            let dump = body["messages"][1]["content"].as_str().unwrap();
            let merge = dump.contains("ORDERED SECTION NOTES TO RECONCILE");
            assert_eq!(body["max_tokens"], if merge { 4096 } else { 2048 });
            assert!(body["messages"][0]["content"].as_str().unwrap().contains("Preserve explicit subjects and objects"), "both section and merge requests must preserve action ownership");
            assert!(body["messages"][0]["content"].as_str().unwrap().contains("retrieval pointers do not replace required facts when tools or rereading are forbidden"), "both requests must preserve facts required for a no-reread task");
            if merge {
                assert!(body["messages"][0]["content"].as_str().unwrap().contains("recency alone is not a reason to discard it"));
                assert!(body["messages"][0]["content"].as_str().unwrap().contains("The new state must stand alone"));
                assert!(!body["messages"][0]["content"].as_str().unwrap().contains("Move detailed chronology, completed-work logs, and background evidence out of the working state"));
            }
            let mut calls = observed_attempts.lock().unwrap();
            let count = calls.entry(dump.to_owned()).or_default();
            *count += 1;
            if *count == 1 && (dump.contains("BEGIN-17") || dump.contains("ORDERED SECTION NOTES TO RECONCILE")) {
                return ResponseTemplate::new(if dump.contains("ORDERED SECTION NOTES TO RECONCILE") { 429 } else { 503 });
            }
            drop(calls);
            captured.lock().unwrap().push(dump.to_owned());
            ["BEGIN-17", "MIDDLE-42", "END-99"].into_iter().filter(|marker| dump.contains(marker)).collect::<Vec<_>>().join(", ") + " (section examined)"
        } else {
            let input = body.to_string();
            if ["BEGIN-17", "MIDDLE-42", "END-99", "Report all three markers", "music.mp3", "movie.mp4"].iter().all(|marker| input.contains(marker)) {
                "BEGIN-17, MIDDLE-42, END-99".to_owned()
            } else { "Missing context".to_owned() }
        };
        ResponseTemplate::new(200).set_body_json(json!({"choices":[{"message":{"role":"assistant","content":answer},"finish_reason":"stop"}]}))
    }).mount(&provider).await;
    state
        .set_secret("go".into(), "test-key".into())
        .await
        .unwrap();
    state.set_go_base_url_override(Some(provider.uri()));
    let server = Server::bind(state.clone(), data.path()).await.unwrap();
    let task = tokio::spawn(server.run());
    let thread = cli(
        data.path(),
        "create_thread",
        json!({"projectRoot":project.path(),"provider":"go","model":"test-model"}),
    );
    let id = thread["id"].as_str().unwrap();
    let paths: Vec<_> = [
        "control.txt",
        "photo.png",
        "music.mp3",
        "movie.mp4",
        "archive.zip",
        "[[skill:missing]].bin",
    ]
    .iter()
    .map(|name| sources.path().join(name))
    .collect();
    let uploaded = cli(
        data.path(),
        "attach_files",
        json!({"threadId":id,"paths":paths}),
    );
    let attachments = uploaded.as_array().unwrap();
    assert_eq!(attachments.len(), 6);
    let stored: Vec<_> = attachments
        .iter()
        .map(|file| file["path"].as_str().unwrap())
        .collect();
    assert_eq!(std::fs::read_to_string(stored[0]).unwrap(), text);
    assert!(std::process::Command::new("git")
        .arg("check-ignore")
        .args(&stored)
        .current_dir(project.path())
        .output()
        .unwrap()
        .status
        .success());
    for file in &stored[1..] {
        assert_eq!(std::fs::read(file).unwrap(), binary);
    }
    let client = Client::new(data.path().into());
    assert_eq!(
        cli(
            data.path(),
            "attachment_path",
            json!({"threadId":id,"path":stored[1]})
        ),
        stored[1]
    );
    let mut events = client.subscribe().await.unwrap();
    cli(
        data.path(),
        "send_message",
        json!({"threadId":id,"text":"Report all three markers", "attachments":stored, "reasoningEffort":null}),
    );
    let mut checkpoint = false;
    let terminal = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            let event = Client::next_event(&mut events).await.unwrap();
            if event["name"] == "thread-event" {
                let event = &event["payload"]["event"];
                if event["kind"] == "context_checkpoint" {
                    checkpoint = true;
                }
                if event["kind"] == "finished" || event["kind"] == "failed" {
                    break event.clone();
                }
            }
        }
    })
    .await
    .unwrap();
    assert!(checkpoint);
    assert_eq!(terminal["kind"], "finished", "{terminal}");
    assert_eq!(terminal["result"], "BEGIN-17, MIDDLE-42, END-99");
    {
        let dumps = summaries.lock().unwrap();
        assert!(dumps.len() > 1);
        assert!(dumps.iter().all(|dump| dump.len() < 70_000));
        for marker in ["BEGIN-17", "MIDDLE-42", "END-99"] {
            assert!(dumps.iter().any(|dump| dump.contains(marker)));
        }
    }
    {
        let calls = attempts.lock().unwrap();
        assert_eq!(
            calls.values().filter(|count| **count == 2).count(),
            2,
            "only the failed section and merge should retry"
        );
        assert!(calls.values().all(|count| *count <= 2));
        assert_eq!(
            calls.len(),
            summaries.lock().unwrap().len(),
            "successful summaries must not be repeated"
        );
    }
    let requests = provider.received_requests().await.unwrap();
    assert!(requests.len() > 2);
    let reconciliation = requests
        .iter()
        .find_map(|request| {
            let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
            body["messages"][1]["content"]
                .as_str()
                .is_some_and(|text| text.contains("ORDERED SECTION NOTES TO RECONCILE"))
                .then_some(body)
        })
        .expect("the shared CLI/app path must reconcile multi-section context");
    assert!(reconciliation["messages"][0]["content"]
        .as_str()
        .unwrap()
        .contains("one coherent task state"));
    assert!(requests.last().unwrap().body.len() < 50_000);
    assert!(String::from_utf8_lossy(&requests.last().unwrap().body)
        .contains("Original source section index"));
    let final_request: serde_json::Value =
        serde_json::from_slice(&requests.last().unwrap().body).unwrap();
    let system = final_request["messages"][0]["content"].as_str().unwrap();
    assert!(system.contains("r.get('role')=='User'"));
    assert!(system.contains("enumerate(p.open(),1)"));
    assert!(system.contains("r['content'][:4500]"));
    assert!(system.contains("Root-scoped file tools require relative paths"));
    let history = state.get_thread_history(id).await.unwrap();
    assert_eq!(
        serde_json::to_value(&history[0]).unwrap()["attachments"],
        json!(stored)
    );
    assert!(serde_json::to_string(&history)
        .unwrap()
        .contains("control.txt"));
    let evidence = project.path().join(".themis/context").join(id);
    assert!(
        evidence.is_dir(),
        "compaction must expose recoverable originals in the project"
    );
    let archives: Vec<_> = std::fs::read_dir(&evidence)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "jsonl")
        })
        .collect();
    assert_eq!(archives.len(), 2, "originals and durable section notes");
    let texts: Vec<_> = archives
        .iter()
        .map(|path| std::fs::read_to_string(path).unwrap())
        .collect();
    let original_index = texts
        .iter()
        .position(|text| text.contains("\"role\":\"User\""))
        .unwrap();
    let original_path = &archives[original_index];
    let original = &texts[original_index];
    assert!(texts
        .iter()
        .any(|text| text
            .contains("Compaction section note (navigation only; not original evidence)")));
    let records: Vec<Value> = original
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert!(records.iter().any(|record| record["content"]
        .as_str()
        .is_some_and(|content| content.contains(&text))));
    let checkpoint_text = history
        .iter()
        .filter_map(|item| match item {
            themis_desktop::transcript::HistoryItem::Event { envelope } => match &envelope.event {
                themis_desktop::types::ThreadEvent::ContextCheckpoint { summary } => {
                    Some(summary.as_str())
                }
                _ => None,
            },
            _ => None,
        })
        .next()
        .unwrap();
    assert!(
        checkpoint_text.contains(original_path.to_str().unwrap()),
        "evidence reference must be saved in the checkpoint"
    );
    assert!(std::process::Command::new("git")
        .arg("check-ignore")
        .arg(original_path)
        .current_dir(project.path())
        .output()
        .unwrap()
        .status
        .success());
    assert!(
        !checkpoint_text.contains("ARCHIVE-SECRET-753"),
        "the recovery probe must use a fact omitted by the summary"
    );
    client.call("shutdown", json!({})).await.unwrap();
    task.await.unwrap().unwrap();
    drop(state);
    std::fs::remove_dir_all(project.path().join(".git")).unwrap();
    assert!(!project.path().join(".git").exists());
    provider.reset().await;
    let recovery_root = project.path().canonicalize().unwrap();
    std::fs::write(
        recovery_root.join("pressure.txt"),
        "pressure data ".repeat(4000),
    )
    .unwrap();
    let recovery_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let observed_calls = Arc::clone(&recovery_calls);
    let recovery_original = Arc::new(Mutex::new(None::<String>));
    Mock::given(method("POST")).respond_with(move |request: &Request| {
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        let messages = body["messages"].as_array().unwrap();
        if messages[0]["content"].as_str().unwrap().contains("You summarize agent context") {
            return ResponseTemplate::new(200).set_body_json(json!({"choices":[{"message":{"role":"assistant","content":"Task state only; recovered details omitted."},"finish_reason":"stop"}]}));
        }
        let stage = observed_calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let answer = if stage >= 5 {
            let checkpoint = messages.iter().find(|message| message["content"].as_str().is_some_and(|text| text.starts_with("Earlier context checkpoint") && text.contains("Preserved bounded reads"))).expect("bounded evidence must survive another checkpoint");
            assert!(checkpoint["content"].as_str().unwrap().contains("ARCHIVE-SECRET-753"));
            json!({"choices":[{"message":{"role":"assistant","content":"ARCHIVE-SECRET-753"},"finish_reason":"stop"}]})
        } else if stage > 1 {
            if stage == 2 { assert!(body.to_string().contains("ARCHIVE-SECRET-753"), "native bounded read must deliver the fact"); }
            let calls = if stage == 4 {
                // Exceed the 16k estimate through actual tool results, rather than a turn-count trigger.
                (0..3).map(|index| json!({"id":format!("pressure_{index}"),"type":"function","function":{"name":"read_file","arguments":json!({"file_path":"pressure.txt"}).to_string()}})).collect::<Vec<_>>()
            } else {
                vec![json!({"id":format!("navigation_{stage}"),"type":"function","function":{"name":"list_dir","arguments":json!({"directory_path":"."}).to_string()}})]
            };
            json!({"choices":[{"message":{"role":"assistant","content":null,"tool_calls":calls},"finish_reason":"tool_calls"}]})
        } else {
            let context = messages.iter().filter_map(|message| message["content"].as_str()).collect::<Vec<_>>().join("\n");
            assert!(!context.contains("ARCHIVE-SECRET-753"));
            assert!(context.contains("Saved section notes (Assistant navigation only; verify facts against original sources):"));
            let path: String = serde_json::from_str(context.split("Latest snapshot: ").nth(1).unwrap().lines().next().unwrap()).unwrap();
            let relative = if stage == 0 {
                let relative = Path::new(&path).strip_prefix(&recovery_root).unwrap().to_string_lossy().into_owned();
                *recovery_original.lock().unwrap() = Some(relative.clone());
                relative
            } else {
                assert!(body.to_string().contains("Total records:"), "record metadata must reach the model request");
                recovery_original.lock().unwrap().clone().unwrap()
            };
            assert!(body["tools"].as_array().unwrap().iter().any(|tool| tool["function"]["name"] == "read_file" && tool["function"]["parameters"]["properties"]["jsonl_record"].is_object()));
            json!({"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"recover_1","type":"function","function":{"name":"read_file","arguments":if stage == 0 { json!({"file_path":relative,"find":"ARCHIVE-SECRET-","jsonl_record":999}).to_string() } else { json!({"file_path":relative,"find":"ARCHIVE-SECRET-"}).to_string() }}}]},"finish_reason":"tool_calls"}]})
        };
        ResponseTemplate::new(200).set_body_json(answer)
    }).mount(&provider).await;
    let rebooted = AppState::new_for_test(data.path().join("settings.json"));
    rebooted
        .set_secret("go".into(), "test-key".into())
        .await
        .unwrap();
    rebooted.set_go_base_url_override(Some(provider.uri()));
    rebooted
        .update_settings(themis_desktop::types::SettingsPatch {
            context_token_budget: Some(16000),
            context_messages: Some(4),
            ..Default::default()
        })
        .await
        .unwrap();
    let server = Server::bind(rebooted.clone(), data.path()).await.unwrap();
    let task = tokio::spawn(server.run());
    let mut events = client.subscribe().await.unwrap();
    cli(
        data.path(),
        "send_message",
        json!({"threadId":id,"text":"Recover the archived decision using the evidence references.","reasoningEffort":null}),
    );
    let mut recovered = false;
    let terminal = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            let event = Client::next_event(&mut events).await.unwrap();
            assert_ne!(
                event["name"], "approval-request",
                "non-Git recovery must require only read access"
            );
            if event["name"] == "thread-event" {
                let event = &event["payload"]["event"];
                if event["kind"] == "tool_finished" && event["tool"] == "read_file" {
                    recovered |= event["ok"] == true
                        && event["output"]
                            .as_str()
                            .unwrap()
                            .contains("ARCHIVE-SECRET-753");
                }
                if event["kind"] == "finished" || event["kind"] == "failed" {
                    break event.clone();
                }
            }
        }
    })
    .await
    .unwrap();
    assert!(recovered);
    assert_eq!(terminal["result"], "ARCHIVE-SECRET-753");
    assert_eq!(recovery_calls.load(std::sync::atomic::Ordering::SeqCst), 6);
    let saved = rebooted.get_thread_history(id).await.unwrap();
    let checkpoint = saved
        .iter()
        .rev()
        .find_map(|item| match item {
            themis_desktop::transcript::HistoryItem::Event { envelope } => match &envelope.event {
                themis_desktop::types::ThreadEvent::ContextCheckpoint { summary } => Some(summary),
                _ => None,
            },
            _ => None,
        })
        .unwrap();
    assert!(
        checkpoint.contains("Preserved bounded reads") && checkpoint.contains("ARCHIVE-SECRET-753")
    );
    assert_eq!(
        std::fs::read_to_string(original_path).unwrap(),
        *original,
        "restart/retrieval must not rewrite originals"
    );
    client.call("shutdown", json!({})).await.unwrap();
    task.await.unwrap().unwrap();
}

#[tokio::test]
async fn attachment_paths_and_symlinked_storage_cannot_escape_project() {
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let state = AppState::new_for_test(data.path().join("settings.json"));
    let thread = state
        .create_thread(
            project.path().to_string_lossy().into_owned(),
            themis_desktop::types::ProviderKind::Go,
            None,
        )
        .await
        .unwrap();
    std::fs::write(outside.path().join("secret.txt"), "control").unwrap();
    // A rejected selection does not create attachment copies.
    assert!(state
        .attach_files(
            thread.id.clone(),
            vec![outside.path().to_string_lossy().into_owned()]
        )
        .await
        .is_err());
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(
            outside.path().join("secret.txt"),
            project.path().join(".themis/attachments/escape.txt"),
        )
        .unwrap();
        let server = Server::bind(state.clone(), data.path()).await.unwrap();
        let task = tokio::spawn(server.run());
        let client = Client::new(data.path().into());
        let error = client.call("send_message", json!({"threadId":thread.id,"text":"Inspect", "attachments":[project.path().join(".themis/attachments/escape.txt")],"reasoningEffort":null})).await.unwrap_err();
        assert!(error.contains("does not belong"), "{error}");
        assert!(client.call("attachment_path", json!({"threadId":thread.id,"path":project.path().join(".themis/attachments/escape.txt")})).await.unwrap_err().contains("does not belong"));
        assert!(!state.get_thread(&thread.id).await.unwrap().running);
        assert!(state
            .get_thread_history(&thread.id)
            .await
            .unwrap()
            .is_empty());
        client.call("shutdown", json!({})).await.unwrap();
        task.await.unwrap().unwrap();
        let other_project = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), other_project.path().join(".themis")).unwrap();
        let other = state
            .create_thread(
                other_project.path().to_string_lossy().into_owned(),
                themis_desktop::types::ProviderKind::Go,
                None,
            )
            .await
            .unwrap();
        assert!(state
            .attach_files(
                other.id,
                vec![outside
                    .path()
                    .join("secret.txt")
                    .to_string_lossy()
                    .into_owned()]
            )
            .await
            .is_err());
        assert!(!outside.path().join("attachments").exists());
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn cli_completed_tool_original_survives_restart_without_compaction() {
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    assert!(std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(project.path())
        .status()
        .unwrap()
        .success());
    let original = format!(
        "{}historical receipt: copper-753",
        "observed source line\n".repeat(5000)
    );
    std::fs::write(project.path().join("observation.txt"), &original).unwrap();
    let provider = MockServer::start().await;
    Mock::given(method("POST")).respond_with(|request: &Request| {
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        let tool = body["messages"].as_array().unwrap().iter().any(|message| message["role"] == "tool");
        let message = if tool { json!({"role":"assistant","content":"READY"}) } else {
            json!({"role":"assistant","content":null,"tool_calls":[{"id":"original-read","type":"function","function":{"name":"read_file","arguments":"{\"file_path\":\"observation.txt\"}"}}]})
        };
        ResponseTemplate::new(200).set_body_json(json!({"choices":[{"message":message,"finish_reason":if tool {"stop"} else {"tool_calls"}}]}))
    }).mount(&provider).await;
    let client = Client::new(data.path().into());
    let state = AppState::new_for_test(data.path().join("settings.json"));
    state
        .set_secret("go".into(), "test-key".into())
        .await
        .unwrap();
    state.set_go_base_url_override(Some(provider.uri()));
    let server = Server::bind(state.clone(), data.path()).await.unwrap();
    let task = tokio::spawn(server.run());
    let thread = cli(
        data.path(),
        "create_thread",
        json!({"projectRoot":project.path(),"provider":"go","model":"test-model"}),
    );
    let id = thread["id"].as_str().unwrap();
    let mut events = client.subscribe().await.unwrap();
    cli(
        data.path(),
        "send_message",
        json!({"threadId":id,"text":"Read observation.txt and reply READY","reasoningEffort":null}),
    );
    let terminal = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            let item = Client::next_event(&mut events).await.unwrap();
            if item["name"] == "approval-request" {
                cli(data.path(), "approve_action", json!({"threadId":id,"approvalId":item["payload"]["approval_id"],"decision":"once"}));
            }
            if item["name"] == "thread-event" {
                let event = &item["payload"]["event"];
                assert_ne!(event["kind"], "context_checkpoint");
                if event["kind"] == "tool_finished" {
                    let output = event["output"].as_str().unwrap();
                    assert!(output.contains("[Output truncated]"));
                    assert!(!output.contains("copper-753"));
                }
                if event["kind"] == "finished" || event["kind"] == "failed" { break event.clone(); }
            }
        }
    }).await.unwrap();
    assert_eq!(terminal["result"], "READY");
    let evidence = project.path().join(".themis/context").join(id);
    let archives: Vec<_> = std::fs::read_dir(&evidence)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "jsonl")
        })
        .collect();
    assert_eq!(archives.len(), 1);
    let saved = std::fs::read_to_string(&archives[0]).unwrap();
    let records: Vec<Value> = saved
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let output = records[1]["message_type"]["ToolResult"][0]["function"]["arguments"]
        .as_str()
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(output).unwrap()["content"],
        original
    );
    client.call("shutdown", json!({})).await.unwrap();
    task.await.unwrap().unwrap();
    drop(state);
    std::fs::write(
        project.path().join("observation.txt"),
        "current receipt: silver-864",
    )
    .unwrap();
    provider.reset().await;
    let archive = archives[0].clone();
    Mock::given(method("POST")).respond_with(move |request: &Request| {
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        let tool = body["messages"].as_array().unwrap().iter().find(|message| message["role"] == "tool");
        let message = if let Some(tool) = tool {
            assert!(tool.to_string().contains("copper-753"), "actual recovery tool result: {tool}");
            assert!(!tool.to_string().contains("silver-864"));
            json!({"role":"assistant","content":"copper-753"})
        } else {
            assert!(!body.to_string().contains("copper-753"), "display/context must not leak the historical receipt");
            assert!(body["messages"][0]["content"].as_str().unwrap().contains(archive.parent().unwrap().to_str().unwrap()), "the restarted model must receive the journal directory without needing a checkpoint");
            let script = "import json,sys; r=[json.loads(x) for x in open(sys.argv[1])]; v=json.loads(r[1]['message_type']['ToolResult'][0]['function']['arguments']); print(v['content'].split('historical receipt: ')[1])";
            json!({"role":"assistant","content":null,"tool_calls":[{"id":"recover","type":"function","function":{"name":"shell","arguments":json!({"command":"python3","args":["-c",script,archive],"cwd":""}).to_string()}}]})
        };
        ResponseTemplate::new(200).set_body_json(json!({"choices":[{"message":message,"finish_reason":if tool.is_some() {"stop"} else {"tool_calls"}}]}))
    }).mount(&provider).await;
    let state = AppState::new_for_test(data.path().join("settings.json"));
    state
        .set_secret("go".into(), "test-key".into())
        .await
        .unwrap();
    state.set_go_base_url_override(Some(provider.uri()));
    let server = Server::bind(state.clone(), data.path()).await.unwrap();
    let task = tokio::spawn(server.run());
    let mut events = client.subscribe().await.unwrap();
    cli(
        data.path(),
        "send_message",
        json!({"threadId":id,"text":"Recover the historical receipt using the saved original.","reasoningEffort":null}),
    );
    let terminal = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            let item = Client::next_event(&mut events).await.unwrap();
            if item["name"] == "approval-request" {
                cli(data.path(), "approve_action", json!({"threadId":id,"approvalId":item["payload"]["approval_id"],"decision":"once"}));
            }
            if item["name"] == "thread-event" {
                let event = &item["payload"]["event"];
                if event["kind"] == "finished" || event["kind"] == "failed" { break event.clone(); }
            }
        }
    }).await.unwrap();
    assert_eq!(terminal["result"], "copper-753");
    assert_eq!(std::fs::read_to_string(&archives[0]).unwrap(), saved);
    client.call("shutdown", json!({})).await.unwrap();
    task.await.unwrap().unwrap();
}
