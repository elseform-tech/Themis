//! Document preparation, maintenance evidence and answer retries through the CLI.

use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn cli_credential_lookup_error_is_not_a_missing_key() {
    struct UnavailableStore;
    impl themis_desktop::secrets::SecretStore for UnavailableStore {
        fn get(&self, _: &str) -> Option<String> {
            None
        }
        fn try_get(&self, _: &str) -> Result<Option<String>, String> {
            Err("OS credential lookup unavailable".into())
        }
        fn has(&self, _: &str) -> bool {
            false
        }
        fn set(&self, _: &str, _: &str) -> Result<(), String> {
            unreachable!()
        }
        fn clear(&self, _: &str) -> Result<(), String> {
            unreachable!()
        }
    }
    let data = tempfile::tempdir().unwrap();
    let state = AppState::new_for_test_with_secret_store(
        data.path().join("settings.json"),
        Arc::new(UnavailableStore),
    );
    let server = Server::bind(state, data.path()).await.unwrap();
    let serving = tokio::spawn(server.run());
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
        .arg("--data-dir")
        .arg(data.path())
        .args(["call", "get_secret_status", "{}"])
        .env_remove("OPENCODE_KEY")
        .output()
        .unwrap();
    Client::new(data.path().into())
        .call("shutdown", json!({}))
        .await
        .unwrap();
    serving.await.unwrap().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("OS credential lookup unavailable"));
    assert!(
        output.stdout.is_empty(),
        "failed lookup must not emit confirmed absence"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn cli_summary_override_rejects_unlisted_model_before_preparation() {
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir(project.path().join(".themis")).unwrap();
    std::fs::write(
        project.path().join(".themis/config.jsonc"),
        r#"{"compaction":{"summary_model":"gpt-6-luna"}}"#,
    )
    .unwrap();
    let source = project.path().join("story.txt");
    std::fs::write(
        &source,
        "A fictional archivist records the tide.\n".repeat(6000),
    )
    .unwrap();
    let provider = MockServer::builder().start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data":[{"id":"model-a"}]})))
        .expect(1)
        .mount(&provider)
        .await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&provider)
        .await;
    let state = AppState::new_for_test(data.path().join("settings.json"));
    state
        .set_secret("go".into(), "synthetic-test-key".into())
        .await
        .unwrap();
    state.set_go_base_url_override(Some(provider.uri()));
    let server = Server::bind(state, data.path()).await.unwrap();
    let serving = tokio::spawn(server.run());
    let client = Client::new(data.path().into());
    let thread = cli(
        data.path(),
        "create_thread",
        json!({"projectRoot":project.path(),"provider":"go","model":"model-a"}),
    );
    let id = thread["id"].as_str().unwrap();
    let files = cli(
        data.path(),
        "attach_files",
        json!({"threadId":id,"paths":[source]}),
    );
    let result = client.call("send_message", json!({"threadId":id,"text":"Summarize the attachment","attachments":[files[0]["path"]]})).await;
    if result.is_ok() {
        // A failing regression must still clean up its admitted run before teardown.
        if client
            .call("get_thread", json!({"threadId":id}))
            .await
            .unwrap()["running"]
            == true
        {
            client
                .call("stop_thread", json!({"threadId":id}))
                .await
                .unwrap();
        }
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            while client
                .call("get_thread", json!({"threadId":id}))
                .await
                .unwrap()["running"]
                == true
            {
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            }
        })
        .await
        .unwrap();
    }
    let idle = client
        .call("get_thread", json!({"threadId":id}))
        .await
        .unwrap()["running"]
        == false;
    let history = client
        .call("get_thread_history", json!({"threadId":id}))
        .await
        .unwrap();
    client.call("shutdown", json!({})).await.unwrap();
    serving.await.unwrap().unwrap();
    assert!(idle);
    let error = result.expect_err("unlisted summarizer must fail before preparation");
    assert!(
        error.contains("Summary model 'gpt-6-luna' is unavailable"),
        "{error}"
    );
    let events: Vec<_> = history
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["kind"] == "event")
        .map(|row| row["envelope"]["event"]["kind"].as_str().unwrap())
        .collect();
    assert_eq!(events, ["started", "failed"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn cli_large_narrative_is_semantically_prepared_before_ordinary_generation() {
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir(project.path().join(".themis")).unwrap();
    std::fs::write(
        project.path().join(".themis/config.jsonc"),
        r#"{"context_token_budget":200000,"compaction":{"max_stages":96}}"#,
    )
    .unwrap();
    let source = project.path().join("story.txt");
    let text = format!("# Opening\nEARLY_CAPSULE_FACT: Mira repairs the tide clock.\n{}\n# Middle\nMIDDLE_CAPSULE_FACT: The key is counterfeit.\n{}\n# Ending\nLATE_CAPSULE_FACT: The captain survives.\n",
        "The town observes the changing water and keeps a record of its work.\n".repeat(2400),
        "The travellers compare their accounts and distinguish plans from completed events.\n".repeat(2400));
    std::fs::write(&source, &text).unwrap();
    let small = project.path().join("witness.md");
    std::fs::write(&small, "Mica is the witness, not the captain.").unwrap();
    let provider = MockServer::builder().start().await;
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let observed = requests.clone();
    Mock::given(method("POST")).respond_with(move |request: &Request| {
        let body: Value = request.body_json().unwrap();
        observed.lock().unwrap().push(body.clone());
        let prompt = body["messages"][0]["content"].as_str().unwrap();
        let answer = if prompt.contains("You summarize an attached document") {
            "Entities/relationships: Mira repairs the tide clock; Mica is a witness. Chronology/causality: The key is counterfeit; the captain survives. Ambiguities: none. Source references: original story opening, middle and ending."
        } else if prompt.contains("You summarize agent context") {
            "Goal/constraints: preserve original evidence. Confirmed findings: narrative facts supplied. Completed changes/tests: none. Unresolved work: answer. Next action: answer."
        } else {
            "The captain survives; the key is counterfeit."
        };
        ResponseTemplate::new(200).set_body_json(json!({"choices":[{"message":{"content":answer},"finish_reason":"stop"}]}))
    }).mount(&provider).await;
    let state = AppState::new_for_test(data.path().join("settings.json"));
    state
        .set_secret("go".into(), "test-key".into())
        .await
        .unwrap();
    state.set_go_base_url_override(Some(provider.uri()));
    let server = Server::bind(state, data.path()).await.unwrap();
    let serving = tokio::spawn(server.run());
    let client = Client::new(data.path().into());
    let mut events = client.subscribe().await.unwrap();
    let thread = cli(
        data.path(),
        "create_thread",
        json!({"projectRoot":project.path(),"provider":"go","model":"test-model"}),
    );
    let id = thread["id"].as_str().unwrap();
    let files = cli(
        data.path(),
        "attach_files",
        json!({"threadId":id,"paths":[source,small]}),
    );
    cli(
        data.path(),
        "send_message",
        json!({"threadId":id,"text":"Retell the attached story faithfully, including chronology, motives and final outcomes. No tools.","attachments":[files[0]["path"],files[1]["path"]]}),
    );
    let mut starts = 0;
    tokio::time::timeout(std::time::Duration::from_secs(60), async {
        loop {
            let item = Client::next_event(&mut events).await.unwrap();
            if item["name"] != "thread-event" || item["payload"]["thread_id"] != id {
                continue;
            }
            let event = &item["payload"]["event"];
            assert_ne!(event["kind"], "failed", "{event}");
            if event["kind"] == "started" {
                starts += 1;
            }
            if event["kind"] == "finished" {
                break;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(
        starts, 1,
        "preparation and ordinary generation share one run start"
    );
    let usage = cli(data.path(), "get_context_usage", json!({"threadId":id}));
    assert_eq!(
        usage["total_requests"].as_u64().unwrap() as usize,
        requests.lock().unwrap().len(),
        "document preparation must be counted in the same run"
    );
    let captured = requests.lock().unwrap();
    let document: Vec<_> = captured
        .iter()
        .filter(|body| {
            body["messages"][0]["content"]
                .as_str()
                .unwrap()
                .contains("You summarize an attached document")
        })
        .collect();
    assert!(!document.is_empty(), "large narrative must be prepared semantically before ordinary generation; maps do not establish semantic coverage");
    let prepared = serde_json::to_string(&document).unwrap();
    for marker in [
        "EARLY_CAPSULE_FACT",
        "MIDDLE_CAPSULE_FACT",
        "LATE_CAPSULE_FACT",
    ] {
        assert!(
            prepared.contains(marker),
            "missing original source range: {marker}"
        );
    }
    let mut fragments = std::collections::BTreeMap::<usize, String>::new();
    let mut complete = false;
    for body in &document {
        for line in body["messages"][1]["content"].as_str().unwrap().lines() {
            let Ok(record) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            if record["content"] == text {
                complete = true;
            }
            if let (Some(start), Some(fragment)) = (
                record["serialized_byte_start"].as_u64(),
                record["prepared_record_fragment"].as_str(),
            ) {
                fragments.insert(start as usize, fragment.to_owned());
            }
        }
    }
    if !complete {
        let mut reconstructed = String::new();
        for (offset, fragment) in fragments {
            assert_eq!(
                offset,
                reconstructed.len(),
                "source coverage must be gap-free"
            );
            reconstructed.push_str(&fragment);
        }
        let record: Value = serde_json::from_str(&reconstructed).unwrap();
        assert_eq!(
            record["content"], text,
            "entire source must be supplied, not only sampled markers"
        );
    }
    let ordinary = captured.last().unwrap().to_string();
    assert!(ordinary.contains("Document semantic capsule"));
    assert!(
        ordinary.contains("Mica is the witness"),
        "small attachment must remain available"
    );
    assert!(
        !ordinary.contains(&text),
        "ordinary request must use a bounded capsule"
    );
    assert_eq!(std::fs::read_to_string(source).unwrap(), text);
    assert_eq!(
        std::fs::read_to_string(files[0]["path"].as_str().unwrap()).unwrap(),
        text
    );
    serving.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn cli_stop_during_document_preparation_emits_one_terminal_and_allows_next_run() {
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir(project.path().join(".themis")).unwrap();
    std::fs::write(
        project.path().join(".themis/config.jsonc"),
        r#"{"context_token_budget":200000,"compaction":{"max_stages":96}}"#,
    )
    .unwrap();
    let source = project.path().join("story.txt");
    let text = format!("# Opening\nEARLY_CAPSULE_FACT: Mira repairs the tide clock.\n{}\n# Middle\nMIDDLE_CAPSULE_FACT: The key is counterfeit.\n{}\n# Ending\nLATE_CAPSULE_FACT: The captain survives.\n",
        "The town observes the changing water and keeps a record of its work.\n".repeat(2400),
        "The travellers compare their accounts and distinguish plans from completed events.\n".repeat(2400));
    std::fs::write(&source, &text).unwrap();
    let small = project.path().join("witness.md");
    std::fs::write(&small, "Mica is the witness, not the captain.").unwrap();
    let provider = MockServer::builder().start().await;
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let observed = requests.clone();
    Mock::given(method("POST")).respond_with(move |request: &Request| {
        let body: Value = request.body_json().unwrap();
        observed.lock().unwrap().push(body.clone());
        let prompt = body["messages"][0]["content"].as_str().unwrap();
        let answer = if prompt.contains("You summarize an attached document") {
            "Entities/relationships: Mira repairs the tide clock; Mica is a witness. Chronology/causality: The key is counterfeit; the captain survives. Ambiguities: none. Source references: original story opening, middle and ending."
        } else if prompt.contains("You summarize agent context") {
            "Goal/constraints: preserve original evidence. Confirmed findings: narrative facts supplied. Completed changes/tests: none. Unresolved work: answer. Next action: answer."
        } else {
            "The captain survives; the key is counterfeit."
        };
        let response = ResponseTemplate::new(200).set_body_json(json!({"choices":[{"message":{"content":answer},"finish_reason":"stop"}]}));
        if prompt.contains("You summarize an attached document") { response.set_delay(std::time::Duration::from_secs(30)) } else { response }
    }).mount(&provider).await;
    let state = AppState::new_for_test(data.path().join("settings.json"));
    state
        .set_secret("go".into(), "test-key".into())
        .await
        .unwrap();
    state.set_go_base_url_override(Some(provider.uri()));
    let server = Server::bind(state, data.path()).await.unwrap();
    let serving = tokio::spawn(server.run());
    let client = Client::new(data.path().into());
    let mut events = client.subscribe().await.unwrap();
    let thread = cli(
        data.path(),
        "create_thread",
        json!({"projectRoot":project.path(),"provider":"go","model":"test-model"}),
    );
    let id = thread["id"].as_str().unwrap();
    let files = cli(
        data.path(),
        "attach_files",
        json!({"threadId":id,"paths":[source,small]}),
    );
    cli(
        data.path(),
        "send_message",
        json!({"threadId":id,"text":"Retell the attached story faithfully, including chronology, motives and final outcomes. No tools.","attachments":[files[0]["path"],files[1]["path"]]}),
    );
    tokio::time::timeout(std::time::Duration::from_secs(60), async {
        while requests.lock().unwrap().is_empty() {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let item = Client::next_event(&mut events).await.unwrap();
            if item["name"] == "thread-event"
                && item["payload"]["thread_id"] == id
                && item["payload"]["event"]["kind"] == "started"
            {
                break;
            }
        }
    })
    .await
    .expect("clients need an active, stoppable run while document preparation is pending");
    cli(data.path(), "stop_thread", json!({"threadId":id}));
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let item = Client::next_event(&mut events).await.unwrap();
            if item["name"] == "thread-event"
                && item["payload"]["thread_id"] == id
                && item["payload"]["event"]["kind"] == "failed"
            {
                break;
            }
        }
    })
    .await
    .unwrap();
    // A terminal followed by a second run must never be misattributed to that run.
    cli(
        data.path(),
        "send_message",
        json!({"threadId":id,"text":"Reply ready. No tools."}),
    );
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            let item = Client::next_event(&mut events).await.unwrap();
            if item["name"] != "thread-event" || item["payload"]["thread_id"] != id {
                continue;
            }
            assert_ne!(
                item["payload"]["event"]["kind"], "failed",
                "duplicate terminal or next-run failure: {item}"
            );
            if item["payload"]["event"]["kind"] == "finished" {
                break;
            }
        }
    })
    .await
    .unwrap();
    let history = cli(data.path(), "get_thread_history", json!({"threadId":id}));
    let failures = history
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["envelope"]["event"]["kind"] == "failed")
        .count();
    assert_eq!(failures, 1, "one terminal for the cancelled run");
    assert_eq!(
        requests.lock().unwrap().len(),
        2,
        "cancelled document request, then next ordinary request"
    );
    serving.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn cli_original_derived_reads_reach_summary_after_delivery_and_maintenance() {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let folder = project.path().join(".themis/context/source");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(
        project.path().join(".themis/config.jsonc"),
        r#"{"context_token_budget":24000,"recent_messages":4,"total_turns":24}"#,
    )
    .unwrap();
    let source = json!({"role":"User","evidence_kind":"original","content":format!("ACQUIRED_ORIGINAL_FAILURE\n{}", "source observation ".repeat(4000))}).to_string();
    std::fs::write(folder.join("original.jsonl"), &source).unwrap();
    let provider = MockServer::builder().start().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let ordinary_calls = calls.clone();
    let supplied = Arc::new(AtomicBool::new(false));
    let observed = supplied.clone();
    Mock::given(method("POST")).respond_with(move |request:&Request| {
        let body:Value = request.body_json().unwrap();
        if body["messages"][0]["content"].as_str().unwrap().contains("You summarize agent context") {
            let input = body["messages"][1]["content"].as_str().unwrap();
            if input.contains("ACQUIRED_ORIGINAL_FAILURE") && input.contains("original_derived_excerpt") && input.contains("source_sha256") {
                observed.store(true, Ordering::SeqCst);
            }
            return ResponseTemplate::new(200).set_body_json(json!({"choices":[{"message":{"content":"Goal/constraints: preserve. Confirmed findings: observed original failure. Completed changes/tests: reads completed. Unresolved work: finish. Next action: continue."},"finish_reason":"stop"}]}));
        }
        let index = ordinary_calls.fetch_add(1, Ordering::SeqCst);
        let message = if index < 12 {
            json!({"tool_calls":[{"id":format!("source-{index}"),"type":"function","function":{"name":"read_file","arguments":json!({"file_path":".themis/context/source/original.jsonl","jsonl_record":1,"offset":index*4000}).to_string()}}]})
        } else { json!({"content":"Completed source inspection"}) };
        ResponseTemplate::new(200).set_body_json(json!({"choices":[{"message":message,"finish_reason":if index<12 {"tool_calls"} else {"stop"}}]}))
    }).mount(&provider).await;
    let state = AppState::new_for_test(data.path().join("settings.json"));
    state
        .set_secret("go".into(), "test-key".into())
        .await
        .unwrap();
    state.set_go_base_url_override(Some(provider.uri()));
    let server = Server::bind(state, data.path()).await.unwrap();
    let serving = tokio::spawn(server.run());
    let client = Client::new(data.path().into());
    let mut events = client.subscribe().await.unwrap();
    let thread = cli(
        data.path(),
        "create_thread",
        json!({"projectRoot":project.path(),"provider":"go","model":"test-model"}),
    );
    let id = thread["id"].as_str().unwrap();
    cli(
        data.path(),
        "set_thread_approval_mode",
        json!({"threadId":id,"mode":"yolo"}),
    );
    cli(
        data.path(),
        "send_message",
        json!({"threadId":id,"text":"Inspect successive original source ranges, then finish."}),
    );
    let mut checkpoints = 0;
    let mut started = 0;
    let mut finished = 0;
    let terminal = tokio::time::timeout(std::time::Duration::from_secs(60), async {
        loop {
            let item = Client::next_event(&mut events).await.unwrap();
            if item["name"] != "thread-event" || item["payload"]["thread_id"] != id {
                continue;
            }
            let event = &item["payload"]["event"];
            checkpoints += usize::from(event["kind"] == "context_checkpoint");
            started += usize::from(event["kind"] == "tool_started");
            finished += usize::from(event["kind"] == "tool_finished");
            if event["kind"] == "finished" || event["kind"] == "failed" {
                break event.clone();
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(terminal["kind"], "finished", "{terminal}");
    assert_eq!(calls.load(Ordering::SeqCst), 13);
    assert_eq!(
        (started, finished),
        (12, 12),
        "completed reads must not be replayed across compaction"
    );
    assert!(
        checkpoints > 0,
        "acquired original ranges must create actual semantic pressure"
    );
    assert!(
        supplied.load(Ordering::SeqCst),
        "the actual summary request must include the acquired original excerpt with provenance"
    );
    assert_eq!(
        std::fs::read_to_string(folder.join("original.jsonl")).unwrap(),
        source
    );
    client.call("shutdown", json!({})).await.unwrap();
    serving.await.unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn cli_plain_and_shell_evidence_survives_maintenance_and_reaches_summary() {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let folder = project.path().join(".themis/context/source");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(
        project.path().join(".themis/config.jsonc"),
        // A read adds about 1k tokens. Maintenance starts well below compaction so one read can't
        // jump past it: at the default 0.83 this test passed or failed by the prompt's size.
        // The budget leaves room for the tool list (memory included).
        r#"{"context_token_budget":25500,"recent_messages":2,"total_turns":30,"compaction":{"maintenance_threshold_ratio":0.7}}"#,
    )
    .unwrap();
    let source = format!(
        "{}The copper key belongs to Mira.{}The violet parcel arrived Tuesday.{}",
        "a".repeat(3000),
        "b".repeat(3450),
        "c".repeat(100000)
    );
    let source_path = project.path().join("story.txt");
    std::fs::write(&source_path, &source).unwrap();
    let provider = MockServer::builder().start().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let ordinary_calls = calls.clone();
    let supplied = Arc::new(AtomicUsize::new(0));
    let observed = supplied.clone();
    let maintained = Arc::new(AtomicBool::new(false));
    let maintained_request = maintained.clone();
    let summary_started = Arc::new(AtomicBool::new(false));
    let summary_seen = summary_started.clone();
    Mock::given(method("POST")).respond_with(move |request:&Request| {
        let body:Value = request.body_json().unwrap();
        if body["messages"][0]["content"].as_str().unwrap().contains("You summarize agent context") {
            summary_seen.store(true, Ordering::SeqCst);
            let input = body["messages"][1]["content"].as_str().unwrap();
            if input.contains("The copper key belongs to Mira.") { observed.fetch_or(1, Ordering::SeqCst); }
            if input.contains("The violet parcel arrived Tuesday.") { observed.fetch_or(2, Ordering::SeqCst); }
            return ResponseTemplate::new(200).set_body_json(json!({"choices":[{"message":{"content":"Goal/constraints: preserve. Confirmed findings: observed original failure. Completed changes/tests: reads completed. Unresolved work: finish. Next action: continue."},"finish_reason":"stop"}]}));
        }
        let index = ordinary_calls.fetch_add(1, Ordering::SeqCst);
        if index >= 2 && observed.load(Ordering::SeqCst) == 0 {
            let acquired: String = body["messages"].as_array().unwrap().iter().filter(|m| m["role"] == "tool").map(|m| m["content"].as_str().unwrap_or("")).collect();
            assert!(acquired.contains("The copper key belongs to Mira."), "plain evidence disappeared before any semantic summary");
            assert!(acquired.contains("The violet parcel arrived Tuesday."), "shell evidence disappeared before any semantic summary");
        }
        if !summary_seen.load(Ordering::SeqCst) && body["messages"].as_array().unwrap().iter().any(|m| m["role"] == "assistant" && m["content"].as_str().is_some_and(|c| c.starts_with("Prepared investigation") && c.len() <= 2048)) {
            maintained_request.store(true, Ordering::SeqCst);
        }
        let message = if index < 20 {
            let (name, args) = if index == 1 {
                ("shell", json!({"command":"python3","args":["-c","from pathlib import Path; print(Path('story.txt').read_text()[4000:8000])"]}))
            } else {
                ("read_file", json!({"file_path":"story.txt","offset":index*4000}))
            };
            json!({"content":if index == 0 { format!("Prepared investigation {}", "public intent ".repeat(500)) } else { String::new() },"tool_calls":[{"id":format!("source-{index}"),"type":"function","function":{"name":name,"arguments":args.to_string()}}]})
        } else { json!({"content":"Completed source inspection"}) };
        ResponseTemplate::new(200).set_body_json(json!({"choices":[{"message":message,"finish_reason":if index<20 {"tool_calls"} else {"stop"}}]}))
    }).mount(&provider).await;
    let state = AppState::new_for_test(data.path().join("settings.json"));
    state
        .set_secret("go".into(), "test-key".into())
        .await
        .unwrap();
    state.set_go_base_url_override(Some(provider.uri()));
    let server = Server::bind(state, data.path()).await.unwrap();
    let serving = tokio::spawn(server.run());
    let client = Client::new(data.path().into());
    let mut events = client.subscribe().await.unwrap();
    let thread = cli(
        data.path(),
        "create_thread",
        json!({"projectRoot":project.path(),"provider":"go","model":"test-model"}),
    );
    let id = thread["id"].as_str().unwrap();
    cli(
        data.path(),
        "set_thread_approval_mode",
        json!({"threadId":id,"mode":"yolo"}),
    );
    cli(
        data.path(),
        "send_message",
        json!({"threadId":id,"text":"Inspect successive source ranges, then finish."}),
    );
    let mut checkpoints = 0;
    let mut started = 0;
    let mut finished = 0;
    let terminal = tokio::time::timeout(std::time::Duration::from_secs(60), async {
        loop {
            let item = Client::next_event(&mut events).await.unwrap();
            if item["name"] != "thread-event" || item["payload"]["thread_id"] != id {
                continue;
            }
            let event = &item["payload"]["event"];
            checkpoints += usize::from(event["kind"] == "context_checkpoint");
            started += usize::from(event["kind"] == "tool_started");
            finished += usize::from(event["kind"] == "tool_finished");
            if event["kind"] == "finished" || event["kind"] == "failed" {
                break event.clone();
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(terminal["kind"], "finished", "{terminal}");
    assert_eq!(calls.load(Ordering::SeqCst), 21);
    assert_eq!(
        (started, finished),
        (20, 20),
        "completed reads must not be replayed across compaction"
    );
    assert!(
        checkpoints > 0,
        "acquired plain and shell ranges must create actual semantic pressure"
    );
    assert_eq!(
        supplied.load(Ordering::SeqCst),
        3,
        "both acquired facts must reach actual summary requests"
    );
    assert!(
        maintained.load(Ordering::SeqCst),
        "exercise actual public-intent maintenance before semantic compaction"
    );
    assert_eq!(std::fs::read_to_string(source_path).unwrap(), source);
    client.call("shutdown", json!({})).await.unwrap();
    serving.await.unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn cli_answer_retries_empty_and_transient_responses_without_replaying_completed_tools() {
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir(project.path().join(".themis")).unwrap();
    std::fs::write(
        project.path().join(".themis/config.jsonc"),
        r#"{"context_token_budget":24000,"recent_messages":2,"total_turns":10}"#,
    )
    .unwrap();
    let provider = MockServer::builder().start().await;
    let times = Arc::new(std::sync::Mutex::new(Vec::new()));
    let observed = times.clone();
    let summary_limits = Arc::new(std::sync::Mutex::new(Vec::new()));
    let observed_limits = summary_limits.clone();
    Mock::given(method("POST")).respond_with(move |request: &Request| {
        let body: Value = request.body_json().unwrap();
        if body["messages"][0]["content"].as_str().unwrap().contains("You summarize agent context") {
            let mut limits = observed_limits.lock().unwrap();
            let limit = body["max_tokens"].as_u64().unwrap();
            limits.push(limit);
            if limits.len() == 1 { return ResponseTemplate::new(429); }
            let complete = limits.len() != 1;
            return ResponseTemplate::new(200).set_body_json(json!({"choices":[{"message":{"content":"Goal/constraints: preserve task. Confirmed findings: original references retained. Completed changes/tests: see observed actions. Unresolved work: reconcile current task. Next action: Append once, then finish."},"finish_reason":if complete {"stop"} else {"length"}}]}));
        }
        let mut times = observed.lock().unwrap();
        let index = times.len();
        times.push(std::time::Instant::now());
        match index {
            0 => ResponseTemplate::new(200).set_body_json(json!({"choices":[{"message":{"tool_calls":[{"id":"append-once","type":"function","function":{"name":"write_file","arguments":json!({"file_path":"result.txt","content":"x","append":true}).to_string()}}]},"finish_reason":"tool_calls"}]})),
            1 => ResponseTemplate::new(200).set_body_json(json!({"choices":[{"message":{"content":null,"reasoning_content":"private"},"finish_reason":"stop"}]})),
            2 => ResponseTemplate::new(200).set_body_json(json!({"choices":[{"message":{"tool_calls":[{"id":"confirm-append","type":"function","function":{"name":"read_file","arguments":json!({"file_path":"result.txt"}).to_string()}}]},"finish_reason":"tool_calls"}]})),
            3 => ResponseTemplate::new(429).insert_header("Retry-After","2"),
            4 => ResponseTemplate::new(503),
            _ => ResponseTemplate::new(200).set_body_json(json!({"choices":[{"message":{"content":"Completed without replay"},"finish_reason":"stop"}]})),
        }
    }).mount(&provider).await;
    let state = AppState::new_for_test(data.path().join("settings.json"));
    state
        .set_secret("go".into(), "test-key".into())
        .await
        .unwrap();
    state.set_go_base_url_override(Some(provider.uri()));
    let server = Server::bind(state, data.path()).await.unwrap();
    let serving = tokio::spawn(server.run());
    let client = Client::new(data.path().into());
    let mut events = client.subscribe().await.unwrap();
    let thread = cli(
        data.path(),
        "create_thread",
        json!({"projectRoot":project.path(),"provider":"go","model":"test-model"}),
    );
    let id = thread["id"].as_str().unwrap();
    cli(
        data.path(),
        "set_thread_approval_mode",
        json!({"threadId":id,"mode":"yolo"}),
    );
    let source = project.path().join("large.log");
    std::fs::write(&source, "old data ".repeat(110_000)).unwrap();
    let staged = cli(
        data.path(),
        "attach_files",
        json!({"threadId":id,"paths":[source]}),
    );
    client.call("import_legacy_history", json!({"threadId":id,"messages":[{"id":"pressure","role":"user","text":"Old observed evidence ".repeat(5000)}]})).await.unwrap();
    cli(
        data.path(),
        "send_message",
        json!({"threadId":id,"text":"Append once and finish","attachments":[staged[0]["path"]]}),
    );
    let mut compactions = 0;
    let mut completed = 0;
    let terminal = tokio::time::timeout(std::time::Duration::from_secs(60), async {
        loop {
            let item = Client::next_event(&mut events).await.unwrap();
            if item["name"] != "thread-event" || item["payload"]["thread_id"] != id {
                continue;
            }
            let event = &item["payload"]["event"];
            compactions += usize::from(event["kind"] == "context_compacting");
            completed += usize::from(event["kind"] == "tool_finished");
            if event["kind"] == "finished" || event["kind"] == "failed" {
                break event.clone();
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(terminal["kind"], "finished", "{terminal}");
    assert_eq!(terminal["result"], "Completed without replay");
    assert_eq!(compactions, 1);
    assert_eq!(completed, 2);
    {
        let limits = summary_limits.lock().unwrap();
        assert!(limits.len() >= 2, "the failed source stage must retry");
        assert_eq!(limits[1], limits[0] * 2);
    }
    assert_eq!(
        std::fs::read_to_string(project.path().join("result.txt")).unwrap(),
        "x"
    );
    {
        let times = times.lock().unwrap();
        // The last request is the spec check after the completed write.
        assert_eq!(times.len(), 7);
        assert!(
            times[2].duration_since(times[1]) >= std::time::Duration::from_secs(1),
            "Empty output must recover before claiming completion"
        );
        assert!(
            times[4].duration_since(times[3]) >= std::time::Duration::from_secs(2),
            "Retry-After is a minimum"
        );
        assert!(
            times[5].duration_since(times[4]) >= std::time::Duration::from_secs(2),
            "The second backoff increases"
        );
    }
    client.call("shutdown", json!({})).await.unwrap();
    serving.await.unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn cli_bounds_alternating_unchanged_reads_by_the_turn_limit() {
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir(project.path().join(".themis")).unwrap();
    std::fs::write(
        project.path().join(".themis/config.jsonc"),
        r#"{"context_token_budget":24000,"recent_messages":2,"total_turns":10}"#,
    )
    .unwrap();
    std::fs::write(project.path().join("a.txt"), "A").unwrap();
    std::fs::write(project.path().join("b.txt"), "B").unwrap();
    let provider = MockServer::builder().start().await;
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let observed_calls = calls.clone();
    Mock::given(method("POST")).respond_with(move |request: &Request| {
        let body: Value = request.body_json().unwrap();
        let summary = body["messages"][0]["content"].as_str().unwrap().contains("You summarize agent context");
        let message = if summary {
            json!({"content":"Goal/constraints: preserve task. Confirmed findings: original references retained. Completed changes/tests: see observed actions. Unresolved work: reconcile current task. Next action: Read A and B; originals remain available."})
        } else {
            let index = observed_calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            json!({"tool_calls":[{"id":format!("read-{index}"),"type":"function","function":{"name":"read_file","arguments":json!({"file_path":if index.is_multiple_of(2) {"a.txt"} else {"b.txt"}}).to_string()}}]})
        };
        ResponseTemplate::new(200).set_body_json(json!({"choices":[{"message":message,"finish_reason":if summary {"stop"} else {"tool_calls"}}]}))
    }).mount(&provider).await;
    let state = AppState::new_for_test(data.path().join("settings.json"));
    state
        .set_secret("go".into(), "test-key".into())
        .await
        .unwrap();
    state.set_go_base_url_override(Some(provider.uri()));
    let server = Server::bind(state, data.path()).await.unwrap();
    let serving = tokio::spawn(server.run());
    let client = Client::new(data.path().into());
    let mut events = client.subscribe().await.unwrap();
    let thread = cli(
        data.path(),
        "create_thread",
        json!({"projectRoot":project.path(),"provider":"go","model":"test-model"}),
    );
    let id = thread["id"].as_str().unwrap();
    cli(
        data.path(),
        "set_thread_approval_mode",
        json!({"threadId":id,"mode":"yolo"}),
    );
    let source = project.path().join("large.log");
    std::fs::write(&source, "old data ".repeat(110_000)).unwrap();
    let staged = cli(
        data.path(),
        "attach_files",
        json!({"threadId":id,"paths":[source]}),
    );
    client.call("import_legacy_history", json!({"threadId":id,"messages":[{"id":"pressure","role":"user","text":"Old observed evidence ".repeat(5000)}]})).await.unwrap();
    cli(
        data.path(),
        "send_message",
        json!({"threadId":id,"text":"Inspect both files","attachments":[staged[0]["path"]]}),
    );
    let mut compactions = 0;
    let mut completed = 0;
    let terminal = tokio::time::timeout(std::time::Duration::from_secs(60), async {
        loop {
            let item = Client::next_event(&mut events).await.unwrap();
            if item["name"] != "thread-event" || item["payload"]["thread_id"] != id {
                continue;
            }
            let event = &item["payload"]["event"];
            compactions += usize::from(event["kind"] == "context_compacting");
            completed += usize::from(event["kind"] == "tool_finished");
            if ["finished", "incomplete", "failed"]
                .contains(&event["kind"].as_str().unwrap_or_default())
            {
                break event.clone();
            }
        }
    })
    .await
    .unwrap();
    // A model that never stops re-reading is warned at every streak and ends at the
    // configured turn limit; no repeat stops the run early.
    assert_eq!(terminal["kind"], "incomplete", "{terminal}");
    // At this budget the tool definitions alone take about a third of the window, so
    // how many checkpoints the old evidence needs moves with the tool list.
    assert!(compactions >= 1, "{compactions}");
    assert_eq!(completed, 10, "every turn ran its read");
    // Ten tool turns, then the handoff request for the incomplete summary.
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 11);
    assert!(
        !cli(data.path(), "get_thread", json!({"threadId":id}))["running"]
            .as_bool()
            .unwrap()
    );
    assert!(project.path().join(".themis/context").join(id).is_dir());
    client.call("shutdown", json!({})).await.unwrap();
    serving.await.unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn cli_recovers_from_malformed_shell_arguments_after_compaction() {
    for budget in [24_000, 200_000] {
        let data = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        std::fs::create_dir(project.path().join(".themis")).unwrap();
        std::fs::write(
            project.path().join(".themis/config.jsonc"),
            json!({"context_token_budget":budget,"recent_messages":2,"total_turns":8,"compaction":{"timeout_seconds":300}}).to_string(),
        )
        .unwrap();
        let malformed = r#"{"command":"python3","args":["-c","from pathlib import Path; Path(\'unsafe.txt\').write_text(\'unsafe\')"]}"#;
        let provider = MockServer::builder().start().await;
        Mock::given(method("POST")).respond_with(move |request: &Request| {
            let body: Value = request.body_json().unwrap();
            let messages = body["messages"].as_array().unwrap();
            if messages[0]["content"].as_str().unwrap().contains("You summarize agent context") {
                return ResponseTemplate::new(200).set_body_json(json!({"choices":[{"message":{"content":"Goal/constraints: preserve task. Confirmed findings: original references retained. Completed changes/tests: see observed actions. Unresolved work: reconcile current task. Next action: Goal: reject malformed call, then run corrected script."},"finish_reason":"stop"}]}));
            }
            // Reproduce the provider's refusal to parse malformed historical arguments.
            for call in messages.iter().flat_map(|m| m["tool_calls"].as_array().into_iter().flatten()) {
                if serde_json::from_str::<Value>(call["function"]["arguments"].as_str().unwrap()).is_err() {
                    return ResponseTemplate::new(500).set_body_string("function_call arguments JSON parse error");
                }
            }
            let last = messages.last().unwrap();
            if last["tool_call_id"] == "corrected" {
                let result: Value = serde_json::from_str(last["content"].as_str().unwrap()).unwrap();
                assert_eq!(result["exit_code"],0);
                // Windows stdout ends lines with \r\n.
                assert_eq!(result["output"].as_str().unwrap().trim_end(),"verified");
                return ResponseTemplate::new(200).set_body_json(json!({"choices":[{"message":{"content":"Corrected script verified."},"finish_reason":"stop"}]}));
            }
            let (id,args) = if last["tool_call_id"] == "malformed" {
                assert!(last["content"].as_str().unwrap().contains("invalid arguments"));
                ("corrected",json!({"command":"python3","args":["-c",r#"from pathlib import Path
assert not Path('unsafe.txt').exists()
Path('result.txt').write_text("apostrophe: ' and backslash: \\")
print('verified')"#],"cwd":""}).to_string())
            } else { ("malformed",malformed.to_string()) };
            ResponseTemplate::new(200).set_body_json(json!({"choices":[{"message":{"tool_calls":[{"id":id,"type":"function","function":{"name":"shell","arguments":args}}]},"finish_reason":"tool_calls"}]}))
        }).mount(&provider).await;
        let state = AppState::new_for_test(data.path().join("settings.json"));
        state
            .set_secret("go".into(), "test-key".into())
            .await
            .unwrap();
        state.set_go_base_url_override(Some(provider.uri()));
        let server = Server::bind(state, data.path()).await.unwrap();
        let serving = tokio::spawn(server.run());
        let client = Client::new(data.path().into());
        let mut events = client.subscribe().await.unwrap();
        let thread = cli(
            data.path(),
            "create_thread",
            json!({"projectRoot":project.path(),"provider":"go","model":"test-model"}),
        );
        let id = thread["id"].as_str().unwrap();
        let config = cli(
            data.path(),
            "get_runtime_configuration",
            json!({"projectRoot":project.path()}),
        );
        assert_eq!(config["config"]["compaction"]["timeout_seconds"], 300);
        cli(
            data.path(),
            "set_thread_approval_mode",
            json!({"threadId":id,"mode":"yolo"}),
        );
        let source = project.path().join("large.log");
        std::fs::write(&source, "old data ".repeat(110_000)).unwrap();
        let staged = cli(
            data.path(),
            "attach_files",
            json!({"threadId":id,"paths":[source]}),
        );
        import_pressure(&client, id, &"Old observed evidence ".repeat(budget / 4)).await;
        cli(
            data.path(),
            "send_message",
            json!({"threadId":id,"text":"Run the script.","attachments":[staged[0]["path"]]}),
        );
        let mut compactions = 0;
        let terminal = tokio::time::timeout(std::time::Duration::from_secs(60), async {
            loop {
                let event = Client::next_event(&mut events).await.unwrap();
                if event["name"] != "thread-event" || event["payload"]["thread_id"] != id {
                    continue;
                }
                let event = &event["payload"]["event"];
                compactions += usize::from(event["kind"] == "context_compacting");
                if event["kind"] == "finished" || event["kind"] == "failed" {
                    break event.clone();
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(
            terminal["result"], "Corrected script verified.",
            "{terminal}"
        );
        assert!(compactions > 0);
        assert!(!project.path().join("unsafe.txt").exists());
        assert_eq!(
            std::fs::read_to_string(project.path().join("result.txt")).unwrap(),
            "apostrophe: ' and backslash: \\"
        );
        let originals: String = std::fs::read_dir(project.path().join(".themis/context").join(id))
            .unwrap()
            .map(|p| p.unwrap().path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "jsonl")
            })
            .map(|path| std::fs::read_to_string(path).unwrap())
            .collect();
        assert!(
            originals.contains(&serde_json::to_string(malformed).unwrap()),
            "raw rejected arguments survive on disk"
        );
        client.call("shutdown", json!({})).await.unwrap();
        serving.await.unwrap().unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn cli_maintenance_resumes_prepared_action_with_public_intent() {
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir(project.path().join(".themis")).unwrap();
    std::fs::write(
        project.path().join(".themis/config.jsonc"),
        r#"{"schema_version":1,"context_token_budget":24000,"recent_messages":2,"total_turns":10}"#,
    )
    .unwrap();
    let provider = MockServer::builder().start().await;
    Mock::given(method("POST")).respond_with(move |request: &Request| {
        let body: Value = request.body_json().unwrap();
        let messages = body["messages"].as_array().unwrap();
        if messages[0]["content"].as_str().unwrap().contains("You summarize agent context") {
            // A lossy summarizer must not erase the latest action/intent at the request boundary.
            return ResponseTemplate::new(200).set_body_json(json!({"choices":[{"message":{"role":"assistant","content":"Goal/constraints: preserve task. Confirmed findings: original references retained. Completed changes/tests: see observed actions. Unresolved work: reconcile current task. Next action: Goal: repair and verify. No edits yet; inspect requirements."},"finish_reason":"stop"}]}));
        }
        let last = messages.last().unwrap();
        if last["tool_call_id"] == "execute" {
            let result: Value = serde_json::from_str(last["content"].as_str().unwrap()).unwrap();
            assert_eq!(result["exit_code"],0);
            return ResponseTemplate::new(200).set_body_json(json!({"choices":[{"message":{"role":"assistant","content":"Prepared action executed and verified."},"finish_reason":"stop"}]}));
        }
        let (id,name,args,text) = if last["tool_call_id"] == "prepare" {
            let intent = messages[messages.len()-2]["content"].as_str().unwrap_or("");
            if !intent.starts_with("Prepared repair.py; next execute it and verify result.txt.") || intent.len()>2048 {
                return ResponseTemplate::new(422).set_body_string("completed action lost its bounded continuation intent");
            }
            let receipt: Value = serde_json::from_str(last["content"].as_str().unwrap()).unwrap();
            assert_eq!(receipt["success"],true);
            ("execute","shell",json!({"command":"python3","args":["repair.py"],"cwd":""}),String::new())
        } else {
            ("prepare","write_file",json!({"file_path":"repair.py","content":"from pathlib import Path\nPath('result.txt').write_text('verified')\n","append":false}),format!("Prepared repair.py; next execute it and verify result.txt. {}", "old narration ".repeat(8000)))
        };
        ResponseTemplate::new(200).set_body_json(json!({"choices":[{"message":{"role":"assistant","content":text,"tool_calls":[{"id":id,"type":"function","function":{"name":name,"arguments":args.to_string()}}]},"finish_reason":"tool_calls"}]}))
    }).mount(&provider).await;
    let state = AppState::new_for_test(data.path().join("settings.json"));
    state
        .set_secret("go".into(), "test-key".into())
        .await
        .unwrap();
    state.set_go_base_url_override(Some(provider.uri()));
    let server = Server::bind(state, data.path()).await.unwrap();
    let serving = tokio::spawn(server.run());
    let client = Client::new(data.path().into());
    let mut events = client.subscribe().await.unwrap();
    let thread = cli(
        data.path(),
        "create_thread",
        json!({"projectRoot":project.path(),"provider":"go","model":"test-model"}),
    );
    let id = thread["id"].as_str().unwrap();
    cli(
        data.path(),
        "set_thread_approval_mode",
        json!({"threadId":id,"mode":"yolo"}),
    );
    cli(
        data.path(),
        "send_message",
        json!({"threadId":id,"text":"Repair then execute and verify."}),
    );
    let mut compactions = 0;
    let terminal = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            let event = Client::next_event(&mut events).await.unwrap();
            if event["name"] != "thread-event" || event["payload"]["thread_id"] != id {
                continue;
            }
            let event = &event["payload"]["event"];
            compactions += usize::from(event["kind"] == "context_compacting");
            if event["kind"] == "finished" || event["kind"] == "failed" {
                break event.clone();
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(
        terminal["result"], "Prepared action executed and verified.",
        "{terminal}"
    );
    assert_eq!(
        compactions, 0,
        "cheap maintenance preserves intent without invoking the summarizer"
    );
    assert_eq!(
        std::fs::read_to_string(project.path().join("result.txt")).unwrap(),
        "verified"
    );
    client.call("shutdown", json!({})).await.unwrap();
    serving.await.unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn cli_recent_script_receipt_survives_stale_summary_without_replaying_edits() {
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir(project.path().join(".themis")).unwrap();
    // Generate actual context pressure after one completed write before the bounded handoff.
    std::fs::write(
        project.path().join(".themis/config.jsonc"),
        r#"{"context_token_budget":24000,"recent_messages":2,"total_turns":1}"#,
    )
    .unwrap();
    let provider = MockServer::builder().start().await;
    let script = format!(
        "# {}\nfrom pathlib import Path\nPath('result.txt').write_text('x')\nprint('written')",
        "padding".repeat(600)
    );
    let original_args = json!({"command":"python3","args":["-c",script],"cwd":""}).to_string();
    let expected_args = original_args.clone();
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let observed = calls.clone();
    Mock::given(method("POST")).respond_with(move |request: &Request| {
        let body: Value = request.body_json().unwrap();
        let messages = body["messages"].as_array().unwrap();
        if messages[0]["content"].as_str().unwrap().contains("You summarize agent context") {
            return ResponseTemplate::new(200).set_body_json(json!({"choices":[{"message":{"content":"Goal/constraints: preserve task. Confirmed findings: original references retained. Completed changes/tests: see observed actions. Unresolved work: reconcile current task. Next action: Completed Changes: None yet. Inspect requirements."},"finish_reason":"stop"}]}));
        }
        let index = observed.fetch_add(1,std::sync::atomic::Ordering::SeqCst);
        if index == 0 {
            return ResponseTemplate::new(200).set_body_json(json!({"choices":[{"message":{"content":format!("Patch complete; next verify result.txt. {}", "old narration ".repeat(8000)),"tool_calls":[{"id":"completed-script","type":"function","function":{"name":"shell","arguments":original_args}}]},"finish_reason":"tool_calls"}]}));
        }
        assert_eq!(index,1,"completed edit must not be replayed");
        let context=messages.iter().filter_map(|m|m["content"].as_str()).collect::<Vec<_>>().join("\n");
        assert!(context.contains("Completed Changes: None yet."));
        assert!(context.contains("Task ledger"));
        assert!(context.contains("Patch complete; next verify result.txt."));
        assert!(context.contains("written"));
        assert!(context.contains("the task ledger records observed actions"));
        let invocation=messages.iter().find(|m|m["tool_calls"][0]["id"]=="completed-script").unwrap();
        let arguments=invocation["tool_calls"][0]["function"]["arguments"].as_str().unwrap();
        assert!(arguments.len()<2048);
        let receipt:Value=serde_json::from_str(arguments).unwrap();
        assert_eq!(receipt["arguments_offloaded"],true);
        let original=std::fs::read_to_string(receipt["original_snapshot"].as_str().unwrap()).unwrap();
        let record:Value=serde_json::from_str(original.lines().nth(receipt["archive_record"].as_u64().unwrap() as usize-1).unwrap()).unwrap();
        assert_eq!(record.pointer(receipt["archive_pointer"].as_str().unwrap()).unwrap(),&json!(expected_args));
        ResponseTemplate::new(200).set_body_json(json!({"choices":[{"message":{"content":"Verified edits survived."},"finish_reason":"stop"}]}))
    }).mount(&provider).await;
    let state = AppState::new_for_test(data.path().join("settings.json"));
    state
        .set_secret("go".into(), "test-key".into())
        .await
        .unwrap();
    state.set_go_base_url_override(Some(provider.uri()));
    let server = Server::bind(state, data.path()).await.unwrap();
    let serving = tokio::spawn(server.run());
    let client = Client::new(data.path().into());
    let mut events = client.subscribe().await.unwrap();
    let thread = cli(
        data.path(),
        "create_thread",
        json!({"projectRoot":project.path(),"provider":"go","model":"test-model"}),
    );
    let id = thread["id"].as_str().unwrap();
    cli(
        data.path(),
        "set_thread_approval_mode",
        json!({"threadId":id,"mode":"yolo"}),
    );
    cli(
        data.path(),
        "send_message",
        json!({"threadId":id,"text":"Patch and verify."}),
    );
    let mut compactions = 0;
    let terminal = tokio::time::timeout(std::time::Duration::from_secs(60), async {
        loop {
            let item = Client::next_event(&mut events).await.unwrap();
            if item["name"] != "thread-event" || item["payload"]["thread_id"] != id {
                continue;
            }
            let event = &item["payload"]["event"];
            compactions += usize::from(event["kind"] == "context_compacting");
            if event["kind"] == "incomplete" || event["kind"] == "failed" {
                break event.clone();
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(terminal["result"], "Verified edits survived.", "{terminal}");
    assert_eq!(terminal["kind"], "incomplete");
    assert_eq!(compactions, 1);
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2);
    assert_eq!(
        std::fs::read_to_string(project.path().join("result.txt")).unwrap(),
        "x"
    );
    client.call("shutdown", json!({})).await.unwrap();
    serving.await.unwrap().unwrap();
}
