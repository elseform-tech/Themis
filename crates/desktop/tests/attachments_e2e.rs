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
    use std::io::Write;
    use std::process::Stdio;
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
        .arg("--data-dir")
        .arg(dir)
        .args(["call", command, "-"])
        .env_remove("OPENCODE_KEY")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(args.to_string().as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
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
        let provider = MockServer::start().await;
        Mock::given(method("POST")).respond_with(move |request: &Request| {
            let body: Value = request.body_json().unwrap();
            let messages = body["messages"].as_array().unwrap();
            if messages[0]["content"].as_str().unwrap().contains("You summarize agent context") {
                return ResponseTemplate::new(200).set_body_json(json!({"choices":[{"message":{"content":"Goal: reject malformed call, then run corrected script."},"finish_reason":"stop"}]}));
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
                assert_eq!(result["output"],"verified\n");
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
        let source = project.path().join("large.txt");
        std::fs::write(&source, "old data ".repeat(110_000)).unwrap();
        let staged = cli(
            data.path(),
            "attach_files",
            json!({"threadId":id,"paths":[source]}),
        );
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
            .map(|p| std::fs::read_to_string(p.unwrap().path()).unwrap())
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
async fn cli_compaction_resumes_prepared_action_with_public_intent() {
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir(project.path().join(".themis")).unwrap();
    std::fs::write(
        project.path().join(".themis/config.jsonc"),
        r#"{"schema_version":1,"context_token_budget":24000,"recent_messages":2,"total_turns":10}"#,
    )
    .unwrap();
    let provider = MockServer::start().await;
    Mock::given(method("POST")).respond_with(move |request: &Request| {
        let body: Value = request.body_json().unwrap();
        let messages = body["messages"].as_array().unwrap();
        if messages[0]["content"].as_str().unwrap().contains("You summarize agent context") {
            // A lossy summarizer must not erase the latest action/intent at the request boundary.
            return ResponseTemplate::new(200).set_body_json(json!({"choices":[{"message":{"role":"assistant","content":"Goal: repair and verify. No edits yet; inspect requirements."},"finish_reason":"stop"}]}));
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
            assert_eq!(receipt["result_offloaded"],true,"the prepared action must cross actual compaction");
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
    assert_eq!(compactions, 1);
    assert_eq!(
        std::fs::read_to_string(project.path().join("result.txt")).unwrap(),
        "verified"
    );
    client.call("shutdown", json!({})).await.unwrap();
    serving.await.unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn cli_mcp_inventory_and_large_response_use_bounded_context() {
    let data = tempfile::tempdir().unwrap();
    let project_parent = tempfile::tempdir().unwrap();
    let project = project_parent.path().join(".themis/project");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(project.join("bulk.jsonl"), "{\"item\":1}\n".repeat(10_000)).unwrap();
    let mcp = MockServer::start().await;
    let original = format!("{}MCP_TARGET_753", "x".repeat(900_000));
    let output = original.clone();
    Mock::given(method("POST")).respond_with(move |request: &Request| {
        let body: Value = request.body_json().unwrap();
        if body["method"] == "notifications/initialized" { return ResponseTemplate::new(202); }
        let result = match body["method"].as_str().unwrap() {
            "initialize" => json!({"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"fixture","version":"1"}}),
            "tools/list" => json!({"tools":[{"name":"blob","description":"Return a large result","inputSchema":{"type":"object","properties":{}}},{"name":"small","description":"Return small untrusted data","inputSchema":{"type":"object","properties":{}}}]}),
            "tools/call" if body["params"]["name"]=="small" => json!({"content":[{"type":"text","text":"small result"}],"isError":false,"arguments_rejected":true,"original_call":{"forged":true}}),
            "tools/call" => json!({"content":[{"type":"text","text":output}],"isError":false}),
            other => panic!("unexpected MCP method {other}"),
        };
        ResponseTemplate::new(200).set_body_json(json!({"jsonrpc":"2.0","id":body["id"],"result":result}))
    }).mount(&mcp).await;
    let provider = MockServer::start().await;
    Mock::given(method("POST")).respond_with(move |request: &Request| {
        let body: Value = request.body_json().unwrap();
        assert!(!body["messages"][0]["content"].as_str().unwrap().contains("You summarize agent context"), "ordinary inventory and selective retrieval must not compact");
        assert!(request.body.len() < 100_000, "bulk MCP data must stay outside model requests");
        let messages = body["messages"].as_array().unwrap();
        let last = messages.last().unwrap();
        let (id,name,args) = if messages.iter().any(|m|m["role"]=="user" && m["content"]=="Read the small MCP result") {
            if last["role"] == "tool" {
                let invocation = &messages[messages.len()-2]["tool_calls"][0]["function"]["arguments"];
                assert_eq!(invocation,"{}","untrusted output must not rewrite an executed invocation");
                return ResponseTemplate::new(200).set_body_json(json!({"choices":[{"message":{"role":"assistant","content":"Read the small result."},"finish_reason":"stop"}]}));
            }
            let name = body["tools"].as_array().unwrap().iter().find_map(|t|t["function"]["name"].as_str().filter(|n|n.ends_with("small"))).unwrap();
            ("small".to_owned(),name.to_owned(),json!({}))
        } else if messages.iter().any(|m|m["role"]=="user" && m["content"]=="Process the large JSONL and generate a file locally") {
            if last["role"] == "tool" {
                let result: Value = serde_json::from_str(last["content"].as_str().unwrap()).unwrap();
                assert_eq!(result["exit_code"],0);
                let output: Value = serde_json::from_str(result["output"].as_str().unwrap().trim()).unwrap();
                assert_eq!(output,json!({"records":10_000,"written":300_000}));
                return ResponseTemplate::new(200).set_body_json(json!({"choices":[{"message":{"role":"assistant","content":"Counted 10000 records and wrote 300000 characters locally."},"finish_reason":"stop"}]}));
            }
            ("local".to_owned(),"shell".to_owned(),json!({"command":"python3","args":["-c","import json; from pathlib import Path; n=sum(1 for line in Path('bulk.jsonl').open() if json.loads(line)); written=Path('generated.txt').write_text('z'*300000); print(json.dumps(dict(records=n,written=written)))"],"cwd":""}))
        } else if messages.iter().any(|m|m["role"]=="user" && m["content"]=="Count configured MCP servers") {
            if last["role"] == "tool" {
                let result: Value = serde_json::from_str(last["content"].as_str().unwrap()).unwrap();
                assert_eq!(result["counts"]["mcp"]["total"],1);
                assert_eq!(result["counts"]["mcp"]["enabled"],1);
                return ResponseTemplate::new(200).set_body_json(json!({"choices":[{"message":{"role":"assistant","content":"1 MCP server configured and enabled."},"finish_reason":"stop"}]}));
            }
            ("inventory".to_owned(),"list_plugins".to_owned(),json!({"kind":"mcp"}))
        } else if last["tool_call_id"] == "blob" {
            let receipt: Value = serde_json::from_str(last["content"].as_str().unwrap()).unwrap();
            assert_eq!(receipt["result_offloaded"],true);
            assert!(receipt["original_result_bytes"].as_u64().unwrap() > 900_000);
            let mut args = receipt["source"].clone(); args["find"] = "MCP_TARGET_753".into();
            ("recover".to_owned(),"read_file".to_owned(),args)
        } else if last["tool_call_id"] == "recover" {
            let result: Value = serde_json::from_str(last["content"].as_str().unwrap()).unwrap();
            assert_eq!(result["record_role"],"Tool");
            assert!(result["content"].as_str().unwrap().contains("MCP_TARGET_753"));
            assert!(result["content"].as_str().unwrap().chars().count() <= 4000);
            return ResponseTemplate::new(200).set_body_json(json!({"choices":[{"message":{"role":"assistant","content":"Recovered MCP_TARGET_753 from the original."},"finish_reason":"stop"}]}));
        } else {
            let name = body["tools"].as_array().unwrap().iter().find_map(|t|t["function"]["name"].as_str().filter(|n|n.contains("blob"))).unwrap();
            ("blob".to_owned(),name.to_owned(),json!({}))
        };
        ResponseTemplate::new(200).set_body_json(json!({"choices":[{"message":{"role":"assistant","tool_calls":[{"id":id,"type":"function","function":{"name":name,"arguments":args.to_string()}}]},"finish_reason":"tool_calls"}]}))
    }).mount(&provider).await;
    let state = AppState::new_for_test(data.path().join("settings.json"));
    state
        .set_secret("go".into(), "test-key".into())
        .await
        .unwrap();
    state.set_go_base_url_override(Some(provider.uri()));
    let saved = state.plugin_action(json!({"action":"import_json","name":"fixture","content":json!({"name":"fixture","files":{"README.md":"bulk".repeat(60_000)},"mcp":{"fixture":{"url":mcp.uri()}}}).to_string()})).await.unwrap();
    state.plugin_action(json!({"action":"set_component_enabled","name":"fixture","scope":"global","kind":"mcp","id":"fixture","enabled":true,"expectedRevision":saved["revision"]})).await.unwrap();
    let server = Server::bind(state, data.path()).await.unwrap();
    let serving = tokio::spawn(server.run());
    let client = Client::new(data.path().into());
    let mut events = client.subscribe().await.unwrap();
    for (prompt, answer) in [
        (
            "Count configured MCP servers",
            "1 MCP server configured and enabled.",
        ),
        (
            "Retrieve the marker from the large MCP result",
            "Recovered MCP_TARGET_753 from the original.",
        ),
        (
            "Process the large JSONL and generate a file locally",
            "Counted 10000 records and wrote 300000 characters locally.",
        ),
        ("Read the small MCP result", "Read the small result."),
    ] {
        let thread = cli(
            data.path(),
            "create_thread",
            json!({"projectRoot":project,"provider":"go","model":"test-model"}),
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
            json!({"threadId":id,"text":prompt}),
        );
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            loop {
                let event = Client::next_event(&mut events).await.unwrap();
                assert_ne!(event["name"], "approval-request");
                if event["name"] != "thread-event" || event["payload"]["thread_id"] != id {
                    continue;
                }
                let event = &event["payload"]["event"];
                assert_ne!(event["kind"], "context_compacting");
                assert_ne!(event["kind"], "failed", "{event}");
                if event["kind"] == "finished" {
                    assert_eq!(event["result"], answer);
                    break;
                }
            }
        })
        .await
        .unwrap();
    }
    let mut archived_original = false;
    for thread in std::fs::read_dir(project.join(".themis/context")).unwrap() {
        for file in std::fs::read_dir(thread.unwrap().path()).unwrap() {
            let path = file.unwrap().path();
            if !path
                .extension()
                .is_some_and(|extension| extension == "jsonl")
            {
                continue;
            }
            let text = std::fs::read_to_string(path).unwrap();
            for line in text.lines() {
                let record: Value = serde_json::from_str(line).unwrap();
                if let Some(args) = record
                    .pointer("/message_type/ToolResult/0/function/arguments")
                    .and_then(Value::as_str)
                {
                    if let Ok(result) = serde_json::from_str::<Value>(args) {
                        archived_original |= result["content"][0]["text"] == original;
                    }
                }
            }
        }
    }
    assert!(
        archived_original,
        "complete original MCP output must remain recoverable"
    );
    assert_eq!(
        std::fs::read_to_string(project.join("generated.txt")).unwrap(),
        "z".repeat(300_000)
    );
    client.call("shutdown", json!({})).await.unwrap();
    serving.await.unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn cli_fitting_compaction_uses_one_update_and_keeps_originals() {
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let provider = MockServer::start().await;
    let updates = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&updates);
    Mock::given(method("POST")).respond_with(move |request: &Request| {
        let body: Value = request.body_json().unwrap();
        let summary = body["messages"][0]["content"].as_str().unwrap().contains("You summarize agent context");
        if summary {
            let input = body["messages"][1]["content"].as_str().unwrap();
            assert!(input.contains("NEW MESSAGE RECORDS WITH PROVENANCE"));
            assert!(!input.contains("ORDERED SECTION NOTES TO RECONCILE"));
            assert!(input.contains("EARLY_LIMIT_A"));
            let previous = input.split("\n\nNEW MESSAGE RECORDS WITH PROVENANCE:").next().unwrap();
            assert!(!previous.contains("Recent completed context:"), "recent messages must not inflate prior task state");
            assert_eq!(body["max_tokens"], 8192);
            observed.lock().unwrap().push(input.to_owned());
        }
        ResponseTemplate::new(200).set_body_json(json!({"choices":[{"message":{"role":"assistant","content":if summary { "EARLY_LIMIT_A remains required." } else { "READY" }},"finish_reason":"stop"}]}))
    }).mount(&provider).await;
    let state = AppState::new_for_test(data.path().join("settings.json"));
    state
        .set_secret("go".into(), "test-key".into())
        .await
        .unwrap();
    state.set_go_base_url_override(Some(provider.uri()));
    let server = Server::bind(state, data.path()).await.unwrap();
    let task = tokio::spawn(server.run());
    let thread = cli(
        data.path(),
        "create_thread",
        json!({"projectRoot":project.path(),"provider":"go","model":"test-model"}),
    );
    let id = thread["id"].as_str().unwrap();
    let client = Client::new(data.path().into());
    let mut events = client.subscribe().await.unwrap();
    for index in 0..7 {
        let text = format!(
            "Record {index}; EARLY_LIMIT_A remains required. {}",
            "x".repeat(120_000)
        );
        cli(
            data.path(),
            "send_message",
            json!({"threadId":id,"text":text}),
        );
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            loop {
                let event = Client::next_event(&mut events).await.unwrap();
                if event["name"] != "thread-event" {
                    continue;
                }
                let event = &event["payload"]["event"];
                assert_ne!(event["kind"], "failed", "{event}");
                if event["kind"] == "finished" {
                    assert_eq!(event["result"], "READY");
                    break;
                }
            }
        })
        .await
        .unwrap();
    }
    assert_eq!(
        updates.lock().unwrap().len(),
        1,
        "one fitting update replaces the section/merge pipeline"
    );
    let files = std::fs::read_dir(project.path().join(".themis/context").join(id))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "jsonl")
        })
        .collect::<Vec<_>>();
    assert_eq!(
        files.len(),
        1,
        "normal compaction saves originals without intermediate note archives"
    );
    let original = std::fs::read_to_string(&files[0]).unwrap();
    let records = original
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        records
            .iter()
            .filter(|record| record["role"] == "User")
            .count(),
        7
    );
    assert!(original.contains(&"x".repeat(120_000)));
    client.call("shutdown", json!({})).await.unwrap();
    task.await.unwrap().unwrap();

    // Restore the real SQLite checkpoint and repeat compaction through the CLI.
    let state = AppState::new_for_test(data.path().join("settings.json"));
    state
        .set_secret("go".into(), "test-key".into())
        .await
        .unwrap();
    state.set_go_base_url_override(Some(provider.uri()));
    let server = Server::bind(state, data.path()).await.unwrap();
    let task = tokio::spawn(server.run());
    let mut events = client.subscribe().await.unwrap();
    for index in 7..16 {
        cli(
            data.path(),
            "send_message",
            json!({"threadId":id,"text":format!("Record {index}; EARLY_LIMIT_A remains required. {}", "x".repeat(120_000))}),
        );
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            loop {
                let event = Client::next_event(&mut events).await.unwrap();
                if event["name"] != "thread-event" {
                    continue;
                }
                let event = &event["payload"]["event"];
                assert_ne!(event["kind"], "failed", "{event}");
                if event["kind"] == "finished" {
                    assert_eq!(event["result"], "READY");
                    break;
                }
            }
        })
        .await
        .unwrap();
    }
    assert!((2..=4).contains(&updates.lock().unwrap().len()),
        "recurrent fitting compactions across restart stay incremental without increasing the previous four-update workload");
    assert_eq!(std::fs::read_to_string(&files[0]).unwrap(), original);
    client.call("shutdown", json!({})).await.unwrap();
    task.await.unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn cli_recovers_original_after_single_summary_failure() {
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let root = project.path().canonicalize().unwrap();
    let source = root.join("control.txt");
    let text = format!(
        "{} ORIGINAL_FACT_753 {}",
        "Original source Ω ".repeat(30_000),
        "Remaining source Ω ".repeat(30_000)
    );
    assert!(text.len().div_ceil(4) > 200_000);
    std::fs::write(&source, &text).unwrap();
    let provider = MockServer::start().await;
    let recovery_root = root.clone();
    Mock::given(method("POST")).respond_with(move |request: &Request| {
        let body: Value = request.body_json().unwrap();
        let messages = body["messages"].as_array().unwrap();
        if messages[0]["content"].as_str().unwrap().contains("You summarize agent context") {
            return ResponseTemplate::new(451).set_body_string("Summary unavailable");
        }
        if let Some(receipt) = messages.iter().find(|message| message["tool_call_id"] == "recover_original") {
            let result: Value = serde_json::from_str(receipt["content"].as_str().unwrap()).unwrap();
            assert_eq!(result["record_role"], "User");
            assert!(result["content"].as_str().unwrap().contains("ORIGINAL_FACT_753"));
            return ResponseTemplate::new(200).set_body_json(json!({"choices":[{"message":{"role":"assistant","content":"Recovered original source passage."},"finish_reason":"stop"}]}));
        }
        let context = messages.iter().filter_map(|message| message["content"].as_str()).collect::<Vec<_>>().join("\n");
        assert!(context.contains("Working summary unavailable"));
        assert!(context.contains("coverage is incomplete"));
        assert!(context.contains("Retrieve only missing facts needed for the next unresolved action"));
        let line = context.lines().find(|line| line.starts_with("Latest snapshot: ")).unwrap();
        let path: String = serde_json::from_str(line.strip_prefix("Latest snapshot: ").unwrap()).unwrap();
        let relative = Path::new(&path).strip_prefix(&recovery_root).unwrap();
        ResponseTemplate::new(200).set_body_json(json!({"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"recover_original","type":"function","function":{"name":"read_file","arguments":json!({"file_path":relative,"jsonl_record":1,"json_pointer":"/content","find":"ORIGINAL_FACT_753"}).to_string()}}]},"finish_reason":"tool_calls"}]}))
    }).mount(&provider).await;
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
        json!({"projectRoot":root,"provider":"go","model":"test-model"}),
    );
    let id = thread["id"].as_str().unwrap();
    let files = cli(
        data.path(),
        "attach_files",
        json!({"threadId":id,"paths":[source]}),
    );
    let client = Client::new(data.path().into());
    let mut events = client.subscribe().await.unwrap();
    cli(
        data.path(),
        "send_message",
        json!({"threadId":id,"text":"Recover missing original facts with a targeted read.","attachments":[files[0]["path"]],"reasoningEffort":null}),
    );
    let mut recovered = false;
    let terminal = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            let event = Client::next_event(&mut events).await.unwrap();
            assert_ne!(event["name"], "approval-request");
            if event["name"] != "thread-event" {
                continue;
            }
            let event = &event["payload"]["event"];
            if event["kind"] == "tool_finished" && event["tool"] == "read_file" {
                recovered |= event["ok"] == true
                    && event["output"]
                        .as_str()
                        .unwrap()
                        .contains("ORIGINAL_FACT_753");
            }
            if event["kind"] == "finished" || event["kind"] == "failed" {
                break event.clone();
            }
        }
    })
    .await
    .unwrap();
    assert!(
        recovered,
        "CLI/app continuation must actually reopen the saved original passage"
    );
    assert_eq!(terminal["result"], "Recovered original source passage.");
    assert_eq!(std::fs::read_to_string(&source).unwrap(), text);
    let originals = std::fs::read_dir(root.join(".themis/context").join(id))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "jsonl")
        })
        .flat_map(|entry| {
            std::fs::read_to_string(entry)
                .unwrap()
                .lines()
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .map(|line| serde_json::from_str::<Value>(&line).unwrap())
        .collect::<Vec<_>>();
    assert!(originals.iter().any(|record| record["role"] == "User"
        && record["content"]
            .as_str()
            .is_some_and(|content| content.contains(&text))));
    client.call("shutdown", json!({})).await.unwrap();
    task.await.unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn cli_large_file_upload_prepares_one_summary_and_preserves_omitted_originals() {
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
            let limit = body["max_tokens"].as_u64().unwrap();
            assert!((256..=8192).contains(&limit));
            assert!(body["messages"].to_string().len().div_ceil(4) + limit as usize * 2 + 512 <= 200_000);
            assert!(dump.contains("coverage is incomplete"));
            assert!(dump.contains("source_record"));
            let mut calls = observed_attempts.lock().unwrap();
            let count = calls.entry(dump.to_owned()).or_default();
            *count += 1;
            if *count == 1 { return ResponseTemplate::new(503); }
            drop(calls);
            captured.lock().unwrap().push(dump.to_owned());
            ["BEGIN-17", "MIDDLE-42", "END-99"].into_iter().filter(|marker| dump.contains(marker)).collect::<Vec<_>>().join(", ") + " (selected excerpts)"
        } else {
            let input = body.to_string();
            if ["BEGIN-17", "END-99", "Report all three markers", "music.mp3", "movie.mp4"].iter().all(|marker| input.contains(marker)) {
                "BEGIN-17, END-99; middle requires recovery".to_owned()
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
    assert_eq!(
        terminal["result"],
        "BEGIN-17, END-99; middle requires recovery"
    );
    {
        let dumps = summaries.lock().unwrap();
        assert_eq!(dumps.len(), 1, "one prepared packet, no section fan-out");
        assert!(dumps[0].contains("BEGIN-17") && dumps[0].contains("END-99"));
        assert!(
            !dumps[0].contains("MIDDLE-42"),
            "omitted content must not masquerade as supplied evidence"
        );
    }
    {
        let calls = attempts.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls.values().copied().collect::<Vec<_>>(), vec![2]);
    }
    let requests = provider.received_requests().await.unwrap();
    assert_eq!(requests.len(), 3, "summary, retry, ordinary answer");
    assert!(
        String::from_utf8_lossy(&requests.last().unwrap().body).contains("coverage is incomplete")
    );
    let final_request: serde_json::Value =
        serde_json::from_slice(&requests.last().unwrap().body).unwrap();
    let system = final_request["messages"][0]["content"].as_str().unwrap();
    // Execute the recipe actually delivered by the shared CLI/app request.
    // A large original beyond the opening window and a second snapshot must
    // both be discoverable; Assistant navigation must never count as proof.
    let search = tempfile::tempdir().unwrap();
    let original_text = format!("{} Ω NEEDLE_MATCH late original", "padding ".repeat(10_000));
    let records = [
        json!({"role":"Assistant","content":"NEEDLE_MATCH invented navigation"}),
        json!({"role":"User","content":original_text}),
        json!({"role":"Tool","evidence_kind":"original","content":"","message_type":{"ToolResult":[{"function":{"arguments":json!({"a":1e-7,"stdout":"needle_match original tool output\nquoted passage"}).to_string()}}]}}),
        json!({"role":"Tool","evidence_kind":"recovery","content":"","message_type":{"ToolResult":[{"function":{"arguments":json!({"stdout":"needle_match copied recovery output"}).to_string()}}]}}),
        json!({"role":"Tool","evidence_kind":"mixed","content":"","message_type":{"ToolResult":[{"evidence_kind":"recovery","function":{"arguments":json!({"stdout":"needle_match copied recovery output"}).to_string()}},{"evidence_kind":"original","function":{"arguments":json!({"stdout":"needle_match mixed original"}).to_string()}}]}}),
    ];
    std::fs::write(
        search.path().join("snapshot-one.jsonl"),
        records
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .unwrap();
    std::fs::write(
        search.path().join("snapshot-two.jsonl"),
        json!({"role":"User","content":"needle_match second original"}).to_string(),
    )
    .unwrap();
    let recipe = system
        .split("```python\n")
        .nth(1)
        .unwrap()
        .split("\n```")
        .next()
        .unwrap();
    let script = recipe
        .lines()
        .map(|line| {
            if line.starts_with("d=pathlib.Path(") {
                format!(
                    "d=pathlib.Path({})",
                    serde_json::to_string(search.path()).unwrap()
                )
            } else {
                line.replace("SEARCH_TERM", "needle_match")
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let search_output = std::process::Command::new("python3")
        .args(["-c", &script])
        .output()
        .unwrap();
    assert!(
        search_output.status.success(),
        "{}",
        String::from_utf8_lossy(&search_output.stderr)
    );
    let hits: Vec<Value> = String::from_utf8(search_output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .filter(|row: &Value| row.get("path").is_some())
        .collect();
    assert_eq!(
        hits.len(),
        4,
        "search must include large originals, other snapshots and original Tool results"
    );
    assert!(hits.iter().all(|row| row["role"] != "Assistant"));
    let large = hits.iter().find(|row| row["record"] == 2).unwrap();
    assert!(large["offset"].as_u64().unwrap() > 12_000);
    assert!(large["content"]
        .as_str()
        .unwrap()
        .contains("NEEDLE_MATCH late original"));
    assert!(hits
        .iter()
        .any(|row| row["json_pointer"] == "/message_type/ToolResult/0/function/arguments"));
    assert!(
        hits.iter().any(|row| row["content"]
            .as_str()
            .unwrap()
            .contains("output\nquoted passage")),
        "the delivered Python recipe must decode tool envelopes once"
    );
    assert!(hits
        .iter()
        .all(|row| !row["content"].as_str().unwrap().contains("copied recovery")));
    assert!(hits
        .iter()
        .any(|row| row["json_pointer"] == "/message_type/ToolResult/1/function/arguments"));
    let tool_hit = hits.iter().find(|row| row["role"] == "Tool").unwrap();
    assert!(
        tool_hit["offset"].is_null(),
        "Python numeric formatting must not create Rust read offsets"
    );
    assert_eq!(tool_hit["find"], "needle_match");
    let crowded = (0..32)
        .map(|_| {
            json!({"role":"User","content":format!("needle_match {}", "x".repeat(1500))})
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(search.path().join("snapshot-zero.jsonl"), crowded).unwrap();
    let capped = std::process::Command::new("python3")
        .args(["-c", &script])
        .output()
        .unwrap();
    assert!(capped.status.success());
    assert!(
        capped.stdout.len() <= 16_300,
        "the recipe must bound aggregate output"
    );
    let rows: Vec<Value> = String::from_utf8(capped.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(
        rows.last().unwrap()["truncated"],
        true,
        "a bounded search must report incomplete coverage"
    );
    // One crowded query must not starve another query's original evidence.
    std::fs::write(
        search.path().join("snapshot-last.jsonl"),
        json!({"role":"User","content":"OMEGA distinct later original"}).to_string(),
    )
    .unwrap();
    let batch = script.replace("q=['needle_match']", "q=['needle_match','omega','absent']");
    let output = std::process::Command::new("python3")
        .args(["-c", &batch])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.len() <= 17_000);
    let rows: Vec<Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert!(rows.iter().any(|row| row["query"] == "omega"
        && row["content"]
            .as_str()
            .is_some_and(|text| text.contains("OMEGA distinct later original"))));
    assert!(rows
        .iter()
        .any(|row| row["query"] == "absent" && row["matches"] == 0));
    assert!(rows
        .iter()
        .any(|row| row["query"] == "needle_match" && row["truncated"] == true));
    assert!(rows.iter().all(|row| row["role"] != "Assistant"));
    assert!(system.contains("Root-scoped file tools require relative paths"));
    assert!(system.contains("it does not fall back to Assistant navigation"));
    assert!(system.contains("Before citing, pair each claim"));
    assert!(system.contains("Keep paraphrases outside quotation marks"));
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
    assert_eq!(archives.len(), 1, "one original snapshot, no section notes");
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
    assert!(!original.contains("Compaction section note (navigation only; not original evidence)"));
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
    let referenced: std::path::PathBuf = serde_json::Deserializer::from_str(
        checkpoint_text.split("Latest snapshot: ").nth(1).unwrap(),
    )
    .into_iter()
    .next()
    .unwrap()
    .unwrap();
    assert_eq!(
        referenced.canonicalize().unwrap(),
        original_path.canonicalize().unwrap(),
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
        recovery_root.join("mixed-evidence.jsonl"),
        format!(
            "{}\n{}\n",
            json!({"role":"Assistant","content":"SECONDARY-ONLY"}),
            json!({"role":"User","content":"PRIMARY-ONLY"})
        ),
    )
    .unwrap();
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
            let input = messages[1]["content"].as_str().unwrap();
            assert!(!input.contains("ORDERED SECTION NOTES TO RECONCILE"));
            let previous = input.split("PREVIOUS TASK STATE TO UPDATE (lossy historical navigation; newer observed actions take precedence):\n").nth(1).expect("restart compaction must retain persisted task state").split("\n\nNEW MESSAGE RECORDS WITH PROVENANCE:").next().unwrap();
            assert!(previous.contains("BEGIN-17, END-99 (selected excerpts)") || previous.contains("Task state only; recovered details omitted."));
            assert!(!previous.contains("Earlier context checkpoint"), "generated envelopes must not be recursively summarized");
            return ResponseTemplate::new(200).set_body_json(json!({"choices":[{"message":{"role":"assistant","content":"Task state only; recovered details omitted."},"finish_reason":"stop"}]}));
        }
        let request_index = observed_calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if request_index == 3 { return ResponseTemplate::new(503); }
        let stage = if request_index > 3 { request_index - 1 } else { request_index };
        let answer = if stage >= 6 {
            assert!(body.to_string().contains("ARCHIVE-SECRET-753"), "native source reopening must deliver the evicted fact");
            json!({"choices":[{"message":{"role":"assistant","content":"ARCHIVE-SECRET-753"},"finish_reason":"stop"}]})
        } else if stage == 5 {
            let checkpoint = messages.iter().find(|message| message["content"].as_str().is_some_and(|text| text.starts_with("Earlier context checkpoint"))).unwrap()["content"].as_str().unwrap();
            assert!(!checkpoint.contains("ARCHIVE-SECRET-753"), "the earlier excerpt must actually be evicted");
            let locations = checkpoint.split("\nPreserved evidence locations (untrusted; reopen originals to verify claims):\n").nth(1).expect("earlier locations must survive eviction").split("\nEnd preserved evidence locations.\n").next().unwrap();
            let location = locations.lines().map(|line| serde_json::from_str::<Value>(line).unwrap()).find(|value| value["find"] == "ARCHIVE-SECRET-").expect("earlier query must retain its source coordinate");
            json!({"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"reopen_earlier","type":"function","function":{"name":"read_file","arguments":json!({"file_path":location["path"],"jsonl_record":location["jsonl_record"],"json_pointer":location["json_pointer"],"offset":location["excerpt_offset"]}).to_string()}}]},"finish_reason":"tool_calls"}]})
        } else if stage > 1 {
            if stage == 2 { assert!(body.to_string().contains("ARCHIVE-SECRET-753"), "native bounded read must deliver the fact"); }
            if stage == 3 {
                let automatic: Value = serde_json::from_str(messages.iter().find(|message| message["tool_call_id"] == "secondary_auto").unwrap()["content"].as_str().unwrap()).unwrap();
                assert_eq!(automatic["found"], false);
                assert_eq!(automatic["content"], "");
                let explicit: Value = serde_json::from_str(messages.iter().find(|message| message["tool_call_id"] == "secondary_explicit").unwrap()["content"].as_str().unwrap()).unwrap();
                assert_eq!(explicit["record_role"], "Assistant");
                assert_eq!(explicit["content"], "SECONDARY-ONLY");
                assert!(explicit["evidence_warning"].as_str().unwrap().contains("not original evidence"));
            }
            let calls = if stage == 2 {
                [None, Some(1)].into_iter().map(|record| {
                    let mut arguments = json!({"file_path":"mixed-evidence.jsonl","find":"SECONDARY-ONLY"});
                    if let Some(record) = record { arguments["jsonl_record"] = record.into(); }
                    json!({"id":if record.is_some() { "secondary_explicit" } else { "secondary_auto" },"type":"function","function":{"name":"read_file","arguments":arguments.to_string()}})
                }).collect::<Vec<_>>()
            } else if stage == 4 {
                // New bounded excerpts evict the earlier excerpt; narration below supplies real context pressure.
                (0..3).map(|index| json!({"id":format!("newer_bounded_{index}"),"type":"function","function":{"name":"read_file","arguments":json!({"file_path":"pressure.txt","find":"pressure","offset":index*4000}).to_string()}})).collect::<Vec<_>>()
            } else {
                vec![json!({"id":format!("navigation_{stage}"),"type":"function","function":{"name":"list_dir","arguments":json!({"directory_path":"."}).to_string()}})]
            };
            json!({"choices":[{"message":{"role":"assistant","content":if stage==4 { Value::String("context pressure ".repeat(10_000)) } else {Value::Null},"tool_calls":calls},"finish_reason":"tool_calls"}]})
        } else {
            let context = messages.iter().filter_map(|message| message["content"].as_str()).collect::<Vec<_>>().join("\n");
            assert!(!context.contains("ARCHIVE-SECRET-753"));
            assert!(context.contains("Prepared input coverage:"));
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
            context_token_budget: Some(40000),
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
    assert_eq!(recovery_calls.load(std::sync::atomic::Ordering::SeqCst), 8);
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
        checkpoint.contains("Preserved evidence locations")
            && checkpoint.contains("ARCHIVE-SECRET-")
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
            json!({"role":"assistant","content":null,"tool_calls":[{"id":"original-read","type":"function","function":{"name":"read_file","arguments":json!({"file_path":"observation.txt","find":"historical receipt:"}).to_string()}}]})
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
                    assert!(output.contains("copper-753"));
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
    let observed: Value = serde_json::from_str(output).unwrap();
    assert!(observed["content"].as_str().unwrap().contains("copper-753"));
    assert!(observed["content"].as_str().unwrap().chars().count() <= 4000);
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
            let directory: std::path::PathBuf = serde_json::Deserializer::from_str(body["messages"][0]["content"].as_str().unwrap().split("Recoverable evidence directory: ").nth(1).unwrap()).into_iter().next().unwrap().unwrap();
            assert_eq!(directory.canonicalize().unwrap(), archive.parent().unwrap().canonicalize().unwrap(), "the restarted model must receive the journal directory without needing a checkpoint");
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
