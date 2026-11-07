//! Streaming, reasoning and thinking replay, Go session settings and tool-call round trips.

use super::*;

#[tokio::test]
async fn streaming_keeps_public_text_and_assembles_tool_arguments() {
    use futures_util::StreamExt;
    let server = MockServer::builder().start().await;
    let events = [
        json!({"choices":[{"delta":{"content":"I’ll read ","reasoning_content":"private"},"finish_reason":null}]}),
        json!({"choices":[{"delta":{"content":"the file.","tool_calls":[{"index":0,"id":"call1","function":{"name":"read_file","arguments":"{\"file_path\":"}}]},"finish_reason":null}]}),
        json!({"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"check.txt\"}"}}]},"finish_reason":"tool_calls"}]}),
    ];
    let body = events
        .iter()
        .map(|event| format!("data: {event}\r\n\r\n"))
        .collect::<String>()
        + "data: [DONE]\n\n";
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(body, "text/event-stream"))
        .mount(&server)
        .await;
    let provider = CompatibleProvider::go("test".into(), "mock".into(), Some(server.uri()), None);
    let mut stream = provider
        .chat_stream_with_tools(&[user_message("read")], None, None)
        .await
        .unwrap();
    let mut text = String::new();
    let mut calls = Vec::new();
    while let Some(chunk) = stream.next().await {
        match chunk.unwrap() {
            StreamChunk::Text(delta) => text.push_str(&delta),
            StreamChunk::ToolUseComplete { tool_call, .. } => calls.push(tool_call),
            _ => {}
        }
    }
    assert_eq!(text, "I’ll read the file.");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].function.arguments, "{\"file_path\":\"check.txt\"}");
    assert_eq!(
        server.received_requests().await.unwrap()[0]
            .body_json::<Value>()
            .unwrap()["stream"],
        true
    );
}

proptest::proptest! {
    /// Any sequence of tool-call pieces lands on calls in order, never more calls than
    /// pieces, and a piece with an index always keeps it.
    #[test]
    fn streamed_call_indexes_are_bounded(pieces in proptest::collection::vec((proptest::option::of(0u64..4), proptest::option::of("[ab]{0,2}")), 0..12)) {
        let mut calls = std::collections::BTreeMap::new();
        for (index, id) in &pieces {
            let mut call = json!({"function": {"name": "t", "arguments": ""}});
            if let Some(index) = index { call["index"] = json!(index); }
            if let Some(id) = id { call["id"] = json!(id); }
            let at = super::streamed_call_index(&calls, &call);
            if let Some(index) = index { proptest::prop_assert_eq!(at as u64, *index); }
            let entry = calls.entry(at).or_insert_with(|| ToolCall { id: String::new(), call_type: "function".into(), function: autoagents::llm::FunctionCall { name: String::new(), arguments: String::new() } });
            if let Some(id) = id { if entry.id != *id { entry.id.push_str(id); } }
        }
        proptest::prop_assert!(calls.len() <= pieces.len().max(1));
    }
}

/// Calls streamed whole with their own ids and no `index` (iceman-contributor-free on Go)
/// stay separate calls; a later piece without an id continues the last one.
#[tokio::test]
async fn streamed_calls_without_an_index_stay_separate() {
    use futures_util::StreamExt;
    let server = MockServer::builder().start().await;
    let events = [
        json!({"choices":[{"delta":{"tool_calls":[{"id":"a","type":"function","function":{"name":"list_dir","arguments":"{\"directory_path\": \"src\"}"}}]},"finish_reason":null}]}),
        json!({"choices":[{"delta":{"tool_calls":[{"id":"b","type":"function","function":{"name":"shell","arguments":"{\"command\":"}}]},"finish_reason":null}]}),
        json!({"choices":[{"delta":{"tool_calls":[{"function":{"arguments":" \"ls\"}"}}]},"finish_reason":null}]}),
        json!({"choices":[{"delta":{"tool_calls":[{"id":"b","function":{"arguments":""}}]},"finish_reason":"tool_calls"}]}),
    ];
    let body = events
        .iter()
        .map(|event| format!("data: {event}\r\n\r\n"))
        .collect::<String>()
        + "data: [DONE]\n\n";
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(body, "text/event-stream"))
        .mount(&server)
        .await;
    let provider = CompatibleProvider::go("test".into(), "mock".into(), Some(server.uri()), None);
    let mut stream = provider
        .chat_stream_with_tools(&[user_message("look")], None, None)
        .await
        .unwrap();
    let mut calls = Vec::new();
    while let Some(chunk) = stream.next().await {
        if let StreamChunk::ToolUseComplete { tool_call, .. } = chunk.unwrap() {
            calls.push(tool_call);
        }
    }
    let seen: Vec<(&str, &str, &str)> = calls
        .iter()
        .map(|call| {
            (
                call.id.as_str(),
                call.function.name.as_str(),
                call.function.arguments.as_str(),
            )
        })
        .collect();
    assert_eq!(
        seen,
        [
            ("a", "list_dir", "{\"directory_path\": \"src\"}"),
            ("b", "shell", "{\"command\": \"ls\"}")
        ]
    );
}

#[tokio::test]
async fn chat_reasoning_is_replayed_with_its_own_tool_calls_but_not_into_summaries() {
    use futures_util::StreamExt;
    use wiremock::matchers::body_partial_json;
    let server = MockServer::builder().start().await;
    let events = [
        json!({"choices":[{"delta":{"reasoning_content":"The total is off by "},"finish_reason":null}]}),
        json!({"choices":[{"delta":{"reasoning_content":"one bundle.","tool_calls":[{"index":0,"id":"replay-call-1","function":{"name":"read_file","arguments":"{}"}}]},"finish_reason":"tool_calls"}]}),
    ];
    let body = events
        .iter()
        .map(|event| format!("data: {event}\n\n"))
        .collect::<String>()
        + "data: [DONE]\n\n";
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(body_partial_json(json!({"stream": true})))
        .respond_with(ResponseTemplate::new(200).set_body_raw(body, "text/event-stream"))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(body_partial_json(json!({"stream": false})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"choices":[{"message":{"content":"done","reasoning_content":"hidden","tool_calls":[{"id":"replay-call-2","type":"function","function":{"name":"list_dir","arguments":"{}"}}]},"finish_reason":"tool_calls"}]})))
        .mount(&server)
        .await;
    let provider = CompatibleProvider::go("test".into(), "mock".into(), Some(server.uri()), None);
    let call = |id: &str| ToolCall {
        id: id.into(),
        call_type: "function".into(),
        function: autoagents::llm::FunctionCall {
            name: "read_file".into(),
            arguments: "{}".into(),
        },
    };
    let history = |id: &str| {
        vec![
            user_message("check the total"),
            ChatMessage {
                role: ChatRole::Assistant,
                message_type: MessageType::ToolUse(vec![call(id)]),
                content: String::new(),
            },
            ChatMessage {
                role: ChatRole::Tool,
                message_type: MessageType::ToolResult(vec![call(id)]),
                content: String::new(),
            },
        ]
    };
    let mut stream = provider
        .chat_stream_with_tools(&[user_message("check")], None, None)
        .await
        .unwrap();
    while stream.next().await.is_some() {}
    provider
        .chat(&history("replay-call-1"), None)
        .await
        .unwrap();
    provider
        .chat(&history("replay-call-2"), None)
        .await
        .unwrap();
    // Switched off (an eval comparison), and in a summary request, it is not sent.
    reasoning::REPLAY_REASONING
        .scope(false, provider.chat(&history("replay-call-1"), None))
        .await
        .unwrap();
    COMPACTION_REQUEST
        .scope((), provider.chat(&history("replay-call-1"), None))
        .await
        .unwrap();
    let sent: Vec<Value> = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(|request| {
            request.body_json::<Value>().unwrap()["messages"][1]["reasoning_content"].clone()
        })
        .collect();
    // Streamed and non-streamed reasoning both return with their own calls.
    assert_eq!(sent[1], "The total is off by one bundle.");
    assert_eq!(sent[2], "hidden");
    assert_eq!(sent[3], Value::Null);
    assert_eq!(sent[4], Value::Null);
}

#[tokio::test]
async fn long_replies_get_room_when_the_model_window_is_far_beyond_the_budget() {
    let model = |id: &str, context, output| GoModel {
        id: id.into(),
        effort_levels: vec![],
        context_tokens: Some(context),
        output_tokens: Some(output),
        thinking_toggle: false,
        ..Default::default()
    };
    let catalog = [
        model("wide", 1_000_000, 131_072),
        model("short-output", 1_000_000, 16_000),
        model("narrow", 200_000, 131_072),
    ];
    let caps = with_catalog_limits(&catalog, async {
        [
            answer_output_cap("wide", 200_000),
            answer_output_cap("wide", 32_000),
            answer_output_cap("short-output", 200_000),
            answer_output_cap("narrow", 200_000),
            answer_output_cap("unknown", 200_000),
            answer_output_cap("unknown", 32_000),
        ]
    })
    .await;
    // Room beyond the budget: 32,768 (or the model's own limit); otherwise the old cap.
    // At a 32,000 budget a reply stays within a third of it, so it fits a later checkpoint.
    assert_eq!(caps, [32_768, 10_666, 16_000, 8_192, 8_192, 4_000]);
    assert_eq!(
        answer_output_cap("wide", 200_000),
        8_192,
        "no catalog loaded"
    );
}

#[tokio::test]
async fn thinking_is_on_by_default_for_answers_but_not_for_summaries() {
    let model = |id: &str, toggle: bool, efforts: &[&str]| GoModel {
        id: id.into(),
        effort_levels: efforts.iter().map(|e| e.to_string()).collect(),
        context_tokens: None,
        output_tokens: None,
        thinking_toggle: toggle,
        ..Default::default()
    };
    let catalog = [
        model("longcat-test", true, &[]),
        model("qwen-test", true, &["low", "medium"]),
        model("muse-test", false, &["minimal", "low", "medium", "high"]),
        model("deepseek-test", false, &["low", "high", "max"]),
        model("plain-test", false, &[]),
        model("claude-test", true, &["low", "medium", "high"]),
    ];
    let body = |id: &str, effort: Option<&str>, summary: bool| {
        let mut provider = CompatibleProvider::go("test".into(), id.into(), None, None);
        provider.reasoning_effort = effort.map(str::to_owned);
        let build = || {
            let request = provider
                .chat_request(&[user_message("hi")], None, true, None)
                .unwrap()
                .build()
                .unwrap();
            serde_json::from_slice::<Value>(request.body().unwrap().as_bytes().unwrap()).unwrap()
        };
        let scoped = || OUTPUT_LIMIT.sync_scope(8192, build);
        let run = || {
            if summary {
                COMPACTION_REQUEST.sync_scope((), scoped)
            } else {
                scoped()
            }
        };
        MODEL_THINKING.sync_scope(thinking_table(&catalog), run)
    };
    assert_eq!(
        body("longcat-test", None, false)["thinking"],
        json!({"type":"enabled"})
    );
    assert_eq!(
        body("qwen-test", None, false)["thinking"],
        json!({"type":"enabled","budget_tokens":4096})
    );
    assert_eq!(
        body("muse-test", None, false)["reasoning"],
        json!({"effort":"medium"})
    );
    assert_eq!(
        body("deepseek-test", None, false)["reasoning_effort"],
        "high"
    );
    // The person's own effort wins; a model not in the catalog keeps its own default.
    assert_eq!(
        body("muse-test", Some("low"), false)["reasoning"],
        json!({"effort":"low"})
    );
    let plain = body("plain-test", None, false);
    assert!(plain.get("thinking").is_none() && plain.get("reasoning_effort").is_none());
    let claude = body("claude-test", None, false);
    assert_eq!(claude["thinking"], json!({"type":"adaptive"}));
    assert_eq!(claude["output_config"], json!({"effort":"medium"}));
    assert_eq!(
        body("claude-test", Some("high"), false)["output_config"],
        json!({"effort":"high"})
    );
    // Summaries keep thinking off: with the switch where the model has one, else at its
    // lowest effort; a model with neither gets no thinking field (Go rejects unknown ones).
    assert_eq!(
        body("longcat-test", None, true)["thinking"],
        json!({"type":"disabled"})
    );
    let deepseek = body("deepseek-test", None, true);
    assert!(deepseek.get("thinking").is_none(), "{deepseek}");
    assert_eq!(deepseek["reasoning_effort"], "low");
    let plain = body("plain-test", None, true);
    assert!(plain.get("thinking").is_none() && plain.get("reasoning_effort").is_none());
}

#[tokio::test]
async fn a_final_answers_reasoning_goes_back_with_it_in_later_messages() {
    use futures_util::StreamExt;
    let server = MockServer::builder().start().await;
    let events = [
        json!({"choices":[{"delta":{"reasoning_content":"Bundles count per SKU."},"finish_reason":null}]}),
        json!({"choices":[{"delta":{"content":"The total is 42."},"finish_reason":"stop"}]}),
    ];
    let body = events
        .iter()
        .map(|event| format!("data: {event}\n\n"))
        .collect::<String>()
        + "data: [DONE]\n\n";
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(body, "text/event-stream"))
        .mount(&server)
        .await;
    let provider = CompatibleProvider::go("test".into(), "mock".into(), Some(server.uri()), None);
    let mut stream = provider
        .chat_stream_with_tools(&[user_message("total?")], None, None)
        .await
        .unwrap();
    while stream.next().await.is_some() {}
    let later = [
        user_message("total?"),
        ChatMessage {
            role: ChatRole::Assistant,
            message_type: MessageType::Text,
            content: "The total is 42.".into(),
        },
        user_message("and with tax?"),
    ];
    let request = provider
        .chat_request(&later, None, false, None)
        .unwrap()
        .build()
        .unwrap();
    let sent: Value = serde_json::from_slice(request.body().unwrap().as_bytes().unwrap()).unwrap();
    assert_eq!(
        sent["messages"][1]["reasoning_content"],
        "Bundles count per SKU."
    );
    assert!(sent["messages"][0].get("reasoning_content").is_none());
}

#[test]
fn messages_answers_keep_their_thinking_except_for_claude() {
    let calls = [];
    let blocks = vec![json!({"type":"thinking","thinking":"t","signature":"s"})];
    reasoning::remember(
        &calls,
        "Answer for the messages test.",
        reasoning::Reasoning {
            blocks,
            ..Default::default()
        },
    );
    let history = [
        user_message("q"),
        ChatMessage {
            role: ChatRole::Assistant,
            message_type: MessageType::Text,
            content: "Answer for the messages test.".into(),
        },
        user_message("next"),
    ];
    for (model, kept) in [("qwen-test", true), ("claude-test", false)] {
        let body = messages::messages_body(model, &history, None, false).unwrap();
        let first = &body["messages"][1]["content"][0]["type"];
        assert_eq!(first == "thinking", kept, "{model}: {body}");
    }
}

/// Claude caches only marked prefixes: the system prompt is a cached block and the newest block
/// carries the rolling mark, so each request reads the conversation before it from cache. Other
/// models on the Messages endpoint get no marks they may not take.
#[test]
fn claude_requests_mark_the_system_prompt_and_newest_block_for_caching() {
    let history = vec![
        ChatMessage {
            role: ChatRole::System,
            message_type: MessageType::Text,
            content: "rules".into(),
        },
        user_message("first"),
        ChatMessage {
            role: ChatRole::Assistant,
            message_type: MessageType::Text,
            content: "answer".into(),
        },
        user_message("next"),
    ];
    let body = messages::messages_body("claude-haiku-5-5", &history, None, false).unwrap();
    assert_eq!(body["system"][0]["text"], "rules", "{body}");
    assert_eq!(
        body["system"][0]["cache_control"]["type"], "ephemeral",
        "{body}"
    );
    let messages = body["messages"].as_array().unwrap();
    let newest = messages.last().unwrap()["content"].as_array().unwrap();
    assert_eq!(
        newest.last().unwrap()["cache_control"]["type"],
        "ephemeral",
        "{body}"
    );
    // Only the newest block is marked (the API allows at most four).
    assert_eq!(
        body.to_string().matches("cache_control").count(),
        2,
        "{body}"
    );
    let other = messages::messages_body("qwen3.8-flash", &history, None, false).unwrap();
    assert_eq!(other["system"], "rules", "{other}");
    assert!(!other.to_string().contains("cache_control"), "{other}");
}

#[tokio::test]
async fn a_streamed_responses_cut_off_is_reported_as_truncated() {
    use futures_util::StreamExt;
    let server = MockServer::builder().start().await;
    let body = format!(
        "data: {}\n\n",
        json!({"type":"response.incomplete","response":{"status":"incomplete","incomplete_details":{"reason":"max_output_tokens"}}})
    );
    Mock::given(method("POST"))
        .and(path("/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(body, "text/event-stream"))
        .mount(&server)
        .await;
    let provider =
        CompatibleProvider::go("test".into(), "gpt-test".into(), Some(server.uri()), None);
    let mut stream = provider
        .chat_stream_with_tools(&[user_message("write")], None, None)
        .await
        .unwrap();
    let mut error = None;
    while let Some(chunk) = stream.next().await {
        if let Err(failure) = chunk {
            error = Some(failure.to_string());
        }
    }
    assert!(error.unwrap().contains("Provider response was truncated"));
}

#[tokio::test]
async fn reasoning_and_its_details_go_back_in_the_fields_they_came_in() {
    use futures_util::StreamExt;
    let server = MockServer::builder().start().await;
    // As Kimi and Hunyuan stream: `reasoning` text plus `reasoning_details` fragments.
    let events = [
        json!({"choices":[{"delta":{"reasoning":"Add the ","reasoning_details":[{"type":"reasoning.text","text":"Add the ","format":"unknown","index":0}]}}]}),
        json!({"choices":[{"delta":{"reasoning":"numbers.","reasoning_details":[{"type":"reasoning.text","text":"numbers.","format":"unknown","index":0}]}}]}),
        json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"details-call-1","function":{"name":"add","arguments":"{}"}}]},"finish_reason":"tool_calls"}]}),
    ];
    let body = events
        .iter()
        .map(|event| format!("data: {event}\n\n"))
        .collect::<String>()
        + "data: [DONE]\n\n";
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(body, "text/event-stream"))
        .mount(&server)
        .await;
    let provider =
        CompatibleProvider::go("test".into(), "kimi-test".into(), Some(server.uri()), None);
    let mut stream = provider
        .chat_stream_with_tools(&[user_message("add")], None, None)
        .await
        .unwrap();
    let mut calls = Vec::new();
    while let Some(chunk) = stream.next().await {
        if let StreamChunk::ToolUseComplete { tool_call, .. } = chunk.unwrap() {
            calls.push(tool_call);
        }
    }
    let history = [
        user_message("add"),
        ChatMessage {
            role: ChatRole::Assistant,
            message_type: MessageType::ToolUse(calls.clone()),
            content: String::new(),
        },
        ChatMessage {
            role: ChatRole::Tool,
            message_type: MessageType::ToolResult(calls),
            content: String::new(),
        },
    ];
    let request = provider
        .chat_request(&history, None, false, None)
        .unwrap()
        .build()
        .unwrap();
    let sent: Value = serde_json::from_slice(request.body().unwrap().as_bytes().unwrap()).unwrap();
    let assistant = &sent["messages"][1];
    assert_eq!(assistant["reasoning"], "Add the numbers.");
    assert!(assistant.get("reasoning_content").is_none());
    assert_eq!(
        assistant["reasoning_details"],
        json!([{"type":"reasoning.text","text":"Add the numbers.","format":"unknown","index":0}])
    );
}

/// Claude signs thinking against the conversation before it, and Themis shortens older history,
/// so only the latest assistant message carries its thinking back: a live goal lost seven turns
/// to "bound to a different conversation" when every older block was replayed. Other Messages
/// models keep every block.
#[test]
fn claude_replays_thinking_only_on_the_latest_assistant_message() {
    let call = |id: &str| -> ToolCall {
        serde_json::from_value(json!({"id": id, "type": "function", "function": {"name": "read_file", "arguments": "{}"}})).unwrap()
    };
    let (first, second) = (call("binding-call-1"), call("binding-call-2"));
    reasoning::remember_blocks_for_test(
        std::slice::from_ref(&first),
        vec![json!({"type":"thinking","thinking":"one","signature":"sig-1"})],
    );
    reasoning::remember_blocks_for_test(
        std::slice::from_ref(&second),
        vec![json!({"type":"thinking","thinking":"two","signature":"sig-2"})],
    );
    let turn = |call: &ToolCall| {
        [
            ChatMessage {
                role: ChatRole::Assistant,
                message_type: MessageType::ToolUse(vec![call.clone()]),
                content: String::new(),
            },
            ChatMessage {
                role: ChatRole::Tool,
                message_type: MessageType::ToolResult(vec![call.clone()]),
                content: String::new(),
            },
        ]
    };
    let mut history = vec![user_message("check")];
    history.extend(turn(&first));
    history.extend(turn(&second));
    let claude = messages::messages_body("claude-haiku-5-5", &history, None, false).unwrap();
    let text = claude.to_string();
    assert!(!text.contains("sig-1"), "{claude}");
    assert_eq!(
        claude["messages"][3]["content"][0]["signature"], "sig-2",
        "{claude}"
    );
    let other = messages::messages_body("qwen3.8-flash", &history, None, false).unwrap();
    let text = other.to_string();
    assert!(text.contains("sig-1") && text.contains("sig-2"), "{other}");
}

/// Even the latest thinking block breaks when context maintenance rewrote messages before it
/// mid-run: Claude answers 400 "bound to a different conversation". The request is sent once
/// more without thinking blocks; another 400, or another model, is not retried.
#[tokio::test]
async fn claude_retries_once_without_thinking_when_a_block_is_unbound() {
    let server = MockServer::builder().start().await;
    let refused = json!({"type":"error","error":{"type":"invalid_request_error","message":"Upstream request failed: [invalid_request_error] messages.3.content.0: Invalid `signature` in `thinking` block. The block is bound to a different conversation."}});
    Mock::given(method("POST"))
        .and(path("/messages"))
        .and(wiremock::matchers::body_string_contains("sig-unbound"))
        .respond_with(ResponseTemplate::new(400).set_body_json(refused))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"content":[{"type":"text","text":"done"}],"stop_reason":"end_turn"}),
        ))
        .mount(&server)
        .await;
    let call: ToolCall = serde_json::from_value(json!({"id": "unbound-call-1", "type": "function", "function": {"name": "read_file", "arguments": "{}"}})).unwrap();
    reasoning::remember_blocks_for_test(
        std::slice::from_ref(&call),
        vec![json!({"type":"thinking","thinking":"t","signature":"sig-unbound"})],
    );
    let history = [
        user_message("check"),
        ChatMessage {
            role: ChatRole::Assistant,
            message_type: MessageType::ToolUse(vec![call.clone()]),
            content: String::new(),
        },
        ChatMessage {
            role: ChatRole::Tool,
            message_type: MessageType::ToolResult(vec![call]),
            content: String::new(),
        },
    ];
    let claude = CompatibleProvider::go(
        "test".into(),
        "claude-haiku-5-5".into(),
        Some(server.uri()),
        None,
    );
    let answer = claude.chat_with_tools(&history, None, None).await.unwrap();
    assert_eq!(answer.text().as_deref(), Some("done"));
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 2);
    assert!(String::from_utf8_lossy(&requests[0].body).contains("sig-unbound"));
    assert!(!String::from_utf8_lossy(&requests[1].body).contains("thinking"));
    // Another model gets the error as it is.
    let qwen = CompatibleProvider::go(
        "test".into(),
        "qwen3.8-flash".into(),
        Some(server.uri()),
        None,
    );
    assert!(qwen.chat_with_tools(&history, None, None).await.is_err());
    assert_eq!(server.received_requests().await.unwrap().len(), 3);
}

#[tokio::test]
async fn messages_thinking_blocks_return_first_and_unchanged_with_their_tool_uses() {
    use futures_util::StreamExt;
    let server = MockServer::builder().start().await;
    let events = [
        json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"","signature":""}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"Check the "}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"bundle."}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"sig-1"}}),
        json!({"type":"content_block_start","index":1,"content_block":{"type":"redacted_thinking","data":"opaque"}}),
        json!({"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"thinking-call-1","name":"read_file","input":{}}}),
        json!({"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"{}"}}),
        json!({"type":"message_delta","delta":{"stop_reason":"tool_use"}}),
        json!({"type":"message_stop"}),
    ];
    let body = events
        .iter()
        .map(|event| format!("data: {event}\n\n"))
        .collect::<String>();
    Mock::given(method("POST"))
        .and(path("/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(body, "text/event-stream"))
        .mount(&server)
        .await;
    let provider =
        CompatibleProvider::go("test".into(), "qwen-test".into(), Some(server.uri()), None);
    let mut stream = provider
        .chat_stream_with_tools(&[user_message("check")], None, None)
        .await
        .unwrap();
    let mut calls = Vec::new();
    let mut text = String::new();
    while let Some(chunk) = stream.next().await {
        match chunk.unwrap() {
            StreamChunk::ToolUseComplete { tool_call, .. } => calls.push(tool_call),
            StreamChunk::Text(delta) => text.push_str(&delta),
            _ => {}
        }
    }
    assert_eq!(text, "", "thinking never becomes public text");
    let history = [
        user_message("check"),
        ChatMessage {
            role: ChatRole::Assistant,
            message_type: MessageType::ToolUse(calls.clone()),
            content: "Reading.".into(),
        },
        ChatMessage {
            role: ChatRole::Tool,
            message_type: MessageType::ToolResult(calls),
            content: String::new(),
        },
    ];
    let body = provider
        .chat_request(&history, None, false, None)
        .unwrap()
        .build()
        .unwrap();
    let body: Value = serde_json::from_slice(body.body().unwrap().as_bytes().unwrap()).unwrap();
    let content = &body["messages"][1]["content"];
    assert_eq!(
        content[0],
        json!({"type":"thinking","thinking":"Check the bundle.","signature":"sig-1"})
    );
    assert_eq!(
        content[1],
        json!({"type":"redacted_thinking","data":"opaque"})
    );
    assert_eq!(content[2]["type"], "text");
    assert_eq!(content[3]["id"], "thinking-call-1");
}

#[tokio::test]
async fn responses_reasoning_items_return_before_their_function_calls() {
    let server = MockServer::builder().start().await;
    Mock::given(method("POST"))
        .and(path("/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"output":[
            {"type":"reasoning","id":"rs_1","summary":[],"encrypted_content":"enc-1"},
            {"type":"function_call","call_id":"responses-call-1","name":"read_file","arguments":"{}"}]})))
        .mount(&server)
        .await;
    let provider =
        CompatibleProvider::go("test".into(), "gpt-test".into(), Some(server.uri()), None);
    let reply = provider.chat(&[user_message("check")], None).await.unwrap();
    let calls = reply.tool_calls().unwrap();
    assert_eq!(reply.text().unwrap_or_default(), "");
    let first: Value = server.received_requests().await.unwrap()[0]
        .body_json()
        .unwrap();
    assert_eq!(first["include"], json!(["reasoning.encrypted_content"]));
    let history = [
        user_message("check"),
        ChatMessage {
            role: ChatRole::Assistant,
            message_type: MessageType::ToolUse(calls.clone()),
            content: String::new(),
        },
        ChatMessage {
            role: ChatRole::Tool,
            message_type: MessageType::ToolResult(calls),
            content: String::new(),
        },
    ];
    let body = provider
        .chat_request(&history, None, false, None)
        .unwrap()
        .build()
        .unwrap();
    let body: Value = serde_json::from_slice(body.body().unwrap().as_bytes().unwrap()).unwrap();
    let input = body["input"].as_array().unwrap();
    assert_eq!(input[1]["type"], "reasoning");
    assert_eq!(input[1]["encrypted_content"], "enc-1");
    assert_eq!(input[2]["type"], "function_call");
    assert_eq!(input[2]["call_id"], "responses-call-1");
}

#[tokio::test]
async fn a_repeated_answer_sends_its_reasoning_item_once() {
    // A goal chat repeated one final answer word for word; replaying its
    // reasoning with each copy made the Responses API reject every request
    // ("Duplicate item found with id rs_...").
    let answer = "Every planetarium check still passes; nothing left to change.";
    let server = MockServer::builder().start().await;
    Mock::given(method("POST"))
        .and(path("/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"output":[
            {"type":"reasoning","id":"rs_repeat","summary":[],"encrypted_content":"enc-repeat"},
            {"type":"message","content":[{"type":"output_text","text":answer}]}]})))
        .mount(&server)
        .await;
    let provider =
        CompatibleProvider::go("test".into(), "gpt-test".into(), Some(server.uri()), None);
    let reply = provider.chat(&[user_message("check")], None).await.unwrap();
    assert_eq!(reply.text().unwrap_or_default(), answer);
    let said = || ChatMessage {
        role: ChatRole::Assistant,
        message_type: MessageType::Text,
        content: answer.to_owned(),
    };
    let history = [
        user_message("check"),
        said(),
        user_message("continue"),
        said(),
        user_message("continue"),
    ];
    let body = provider
        .chat_request(&history, None, false, None)
        .unwrap()
        .build()
        .unwrap();
    let body: Value = serde_json::from_slice(body.body().unwrap().as_bytes().unwrap()).unwrap();
    let ids: Vec<&str> = body["input"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|item| item["id"].as_str())
        .collect();
    assert_eq!(ids, ["rs_repeat"]);
}

#[tokio::test]
async fn reasoning_effort_reaches_wire() {
    let server = MockServer::builder().start().await;
    Mock::given(method("POST")).and(path("/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"output": [{"type": "message", "content": [{"type": "output_text", "text": "done"}]}]})))
        .mount(&server).await;
    let mut config = ProviderConfig::new(ProviderKind::Go, "synthetic-test-key")
        .with_model("gpt-5.6-luna")
        .with_base_url(server.uri());
    config.reasoning_effort = Some("high".to_owned());
    resolve(&config)
        .await
        .unwrap()
        .chat(&[user_message("hello")], None)
        .await
        .unwrap();
    let requests = server.received_requests().await.unwrap();
    let body: Value = requests[0].body_json().unwrap();
    assert_eq!(body["reasoning"]["effort"], "high");
}

#[tokio::test]
async fn go_prefers_explicit_model_over_env() {
    let _guard = ENV_LOCK.lock().unwrap();
    std::env::set_var(GO_MODEL_ENV_VAR, "env-model");
    let server = MockServer::builder().start().await;
    mount_chat(&server, chat_ok_body("hi")).await;

    let config = ProviderConfig::new(ProviderKind::Go, "key")
        .with_model("explicit-model")
        .with_base_url(server.uri());
    let handle: Arc<dyn LLMProvider> = resolve(&config).await.unwrap();
    handle.chat(&[user_message("hello")], None).await.unwrap();

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    let body: Value = requests[0].body_json().unwrap();
    assert_eq!(body["model"], "explicit-model");
    std::env::remove_var(GO_MODEL_ENV_VAR);
}

#[tokio::test]
async fn go_model_falls_back_to_env_then_default() {
    let _guard = ENV_LOCK.lock().unwrap();
    let server = MockServer::builder().start().await;
    mount_chat(&server, chat_ok_body("hi")).await;
    Mock::given(method("POST"))
        .and(path("/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "output": [{"type": "message", "content": [{"type": "output_text", "text": "hi"}]}]
        })))
        .mount(&server)
        .await;

    std::env::set_var(GO_MODEL_ENV_VAR, "env-model");
    let config = ProviderConfig::new(ProviderKind::Go, "key").with_base_url(server.uri());
    let handle = resolve(&config).await.unwrap();
    handle.chat(&[user_message("hello")], None).await.unwrap();
    let requests = server.received_requests().await.unwrap();
    let body: Value = requests[0].body_json().unwrap();
    assert_eq!(body["model"], "env-model");

    std::env::remove_var(GO_MODEL_ENV_VAR);
    let handle = resolve(&config).await.unwrap();
    handle.chat(&[user_message("hello")], None).await.unwrap();
    let requests = server.received_requests().await.unwrap();
    let body: Value = requests[1].body_json().unwrap();
    assert_eq!(body["model"], GO_DEFAULT_MODEL);
}

#[tokio::test]
async fn go_sends_themis_user_agent_and_session() {
    let _guard = ENV_LOCK.lock().unwrap();
    std::env::set_var(GO_SESSION_ENV_VAR, "thread-42");
    std::env::remove_var(GO_MODEL_ENV_VAR);
    let server = MockServer::builder().start().await;
    mount_chat(&server, chat_ok_body("hi")).await;

    let config = ProviderConfig::new(ProviderKind::Go, "key")
        .with_model("m")
        .with_base_url(server.uri());
    let handle = resolve(&config).await.unwrap();
    let response = handle.chat(&[user_message("hello")], None).await.unwrap();
    assert_eq!(response.text().as_deref(), Some("hi"));

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    let headers = &requests[0].headers;
    assert_eq!(
        headers.get("user-agent").unwrap().to_str().unwrap(),
        themis_user_agent()
    );
    assert_eq!(
        headers.get(GO_SESSION_HEADER).unwrap().to_str().unwrap(),
        "thread-42"
    );
    std::env::remove_var(GO_SESSION_ENV_VAR);
}

#[tokio::test]
async fn go_session_defaults_when_env_absent() {
    let _guard = ENV_LOCK.lock().unwrap();
    std::env::remove_var(GO_SESSION_ENV_VAR);
    std::env::remove_var(GO_MODEL_ENV_VAR);
    let server = MockServer::builder().start().await;
    mount_chat(&server, chat_ok_body("hi")).await;

    let config = ProviderConfig::new(ProviderKind::Go, "key")
        .with_model("m")
        .with_base_url(server.uri());
    resolve(&config)
        .await
        .unwrap()
        .chat(&[user_message("hello")], None)
        .await
        .unwrap();

    let requests = server.received_requests().await.unwrap();
    assert_eq!(
        requests[0]
            .headers
            .get(GO_SESSION_HEADER)
            .unwrap()
            .to_str()
            .unwrap(),
        GO_DEFAULT_SESSION
    );
}

#[tokio::test]
async fn go_per_instance_session_beats_env() {
    let _guard = ENV_LOCK.lock().unwrap();
    std::env::set_var(GO_SESSION_ENV_VAR, "env-session");
    let server = MockServer::builder().start().await;
    mount_chat(&server, chat_ok_body("hi")).await;

    // Per-conversation IDs pass through the constructor (spike option 1).
    let provider = CompatibleProvider::go(
        "key".to_owned(),
        "m".to_owned(),
        Some(server.uri()),
        Some("thread-7".to_owned()),
    );
    provider.chat(&[user_message("hello")], None).await.unwrap();

    let requests = server.received_requests().await.unwrap();
    assert_eq!(
        requests[0]
            .headers
            .get(GO_SESSION_HEADER)
            .unwrap()
            .to_str()
            .unwrap(),
        "thread-7"
    );
    std::env::remove_var(GO_SESSION_ENV_VAR);
}

#[tokio::test]
async fn go_tool_call_roundtrip_through_dyn_handle() {
    let _guard = ENV_LOCK.lock().unwrap();
    std::env::remove_var(GO_MODEL_ENV_VAR);
    std::env::remove_var(GO_SESSION_ENV_VAR);
    let server = MockServer::builder().start().await;
    mount_chat(&server, tool_call_body()).await;

    // Explicit `Arc<dyn LLMProvider>` annotation: this is the exact handle
    // type `AgentBuilder::llm` accepts, and `chat_with_tools` returning
    // parsed tool calls is the ReAct loop's core LLM primitive.
    let config = ProviderConfig::new(ProviderKind::Go, "key")
        .with_model("m")
        .with_base_url(server.uri());
    let handle: Arc<dyn LLMProvider> = resolve(&config).await.unwrap();
    let tools = [test_tool()];
    let response = handle
        .chat_with_tools(&[user_message("read it")], Some(&tools), None)
        .await
        .unwrap();
    let calls = response.tool_calls().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].id, "call_1");
    assert_eq!(calls[0].function.name, "read_file");
    assert_eq!(calls[0].function.arguments, "{\"path\": \"src/main.rs\"}");

    // The tool schema must have reached the wire.
    let requests = server.received_requests().await.unwrap();
    let body: Value = requests[0].body_json().unwrap();
    assert_eq!(body["tools"][0]["function"]["name"], "read_file");
}

/// Builds one request carrying a model tool call with `raw` arguments and
/// returns the outgoing tool input, decoded as JSON.
fn outgoing_tool_input(model: &str, raw: &str) -> Value {
    let call = |arguments: &str| ToolCall {
        id: "call".into(),
        call_type: "function".into(),
        function: FunctionCall {
            name: "shell".into(),
            arguments: arguments.into(),
        },
    };
    let messages = [
        ChatMessage {
            role: ChatRole::Assistant,
            message_type: MessageType::ToolUse(vec![call(raw)]),
            content: String::new(),
        },
        ChatMessage {
            role: ChatRole::Tool,
            message_type: MessageType::ToolResult(vec![call("result")]),
            content: String::new(),
        },
    ];
    let provider = CompatibleProvider::go("synthetic".into(), model.into(), None, None);
    let request = provider
        .chat_request(&messages, None, true, None)
        .unwrap()
        .build()
        .unwrap();
    let body: Value = serde_json::from_slice(request.body().unwrap().as_bytes().unwrap()).unwrap();
    if provider.protocol() == Protocol::Gemini {
        body["contents"][0]["parts"][0]["functionCall"]["args"].clone()
    } else if provider.uses_responses() {
        serde_json::from_str(body["input"][0]["arguments"].as_str().unwrap()).unwrap()
    } else if provider.uses_messages() {
        body["messages"][0]["content"][0]["input"].clone()
    } else {
        serde_json::from_str(
            body["messages"][0]["tool_calls"][0]["function"]["arguments"]
                .as_str()
                .unwrap(),
        )
        .unwrap()
    }
}

proptest::proptest! {
    #![proptest_config(proptest::prelude::ProptestConfig::with_cases(256))]
    /// Whatever a provider streams, reasoning parts are collected without panicking and
    /// only from the fields that carry reasoning; deltas with no block are dropped.
    #[test]
    fn any_streamed_reasoning_parts_collect_without_panicking(
        parts in proptest::collection::vec((0usize..6, "[ -~]{0,8}"), 0..12)
    ) {
        let fields = ["reasoning_content", "reasoning", "thinking_delta", "signature_delta", "content", "thinking_block"];
        let mut collected = reasoning::StreamedReasoning::default();
        let mut expected_text = String::new();
        let mut blocks = 0;
        for (field, text) in &parts {
            let delta = if fields[*field] == "thinking_block" {
                blocks += 1;
                json!({"thinking_block": {"type": "thinking", "thinking": text}})
            } else {
                json!({fields[*field]: text})
            };
            if *field < 2 {
                expected_text.push_str(text);
            }
            collected.take(&delta);
        }
        let got = collected.peek();
        proptest::prop_assert_eq!(&got.text, &expected_text);
        proptest::prop_assert_eq!(got.blocks.len(), blocks);
    }
}

proptest::proptest! {
    #![proptest_config(proptest::prelude::ProptestConfig::with_cases(128))]
    /// Whatever a model sends as tool arguments, every protocol's next
    /// request carries a JSON object: valid objects unchanged, anything
    /// else wrapped with its exact original text.
    #[test]
    fn any_tool_arguments_encode_as_a_json_object(raw in proptest::prelude::any::<String>(), object in proptest::option::of(proptest::collection::btree_map("[a-z]{1,6}", "[ -~]{0,12}", 0..4))) {
        let raw = object.map_or(raw, |object| serde_json::to_string(&object).unwrap());
        for model in ["mimo-v2.6-flash", "muse-spark-1.3-contributor", "claude-haiku-4-5", "gemini-4-argon-contributor"] {
            let input = outgoing_tool_input(model, &raw);
            proptest::prop_assert!(input.is_object(), "{model}: {input}");
            match serde_json::from_str::<Value>(&raw) {
                Ok(original) if original.is_object() => proptest::prop_assert_eq!(&input, &original),
                _ => proptest::prop_assert_eq!(&input["_themis_rejected_tool_arguments"], &Value::String(raw.clone())),
            }
        }
    }
}

#[tokio::test]
async fn rejected_tool_arguments_do_not_poison_history_in_any_protocol() {
    for model in [
        "mimo-v2.6-flash",
        "muse-spark-1.3-contributor",
        "claude-haiku-4-5",
        "gemini-4-argon-contributor",
    ] {
        for raw in [
            r#"{"command":"python3","args":["-c","print(\'x\')"]}"#,
            "[]",
        ] {
            let call = ToolCall {
                id: "bad-call".into(),
                call_type: "function".into(),
                function: FunctionCall {
                    name: "shell".into(),
                    arguments: raw.into(),
                },
            };
            let messages = [
                ChatMessage {
                    role: ChatRole::Assistant,
                    message_type: MessageType::ToolUse(vec![call]),
                    content: String::new(),
                },
                ChatMessage {
                    role: ChatRole::Tool,
                    message_type: MessageType::ToolResult(vec![ToolCall {
                        id: "bad-call".into(),
                        call_type: "function".into(),
                        function: FunctionCall {
                            name: "shell".into(),
                            arguments: "Arguments rejected; nothing executed".into(),
                        },
                    }]),
                    content: String::new(),
                },
            ];
            let provider = CompatibleProvider::go("synthetic".into(), model.into(), None, None);
            let request = provider
                .chat_request(&messages, None, true, None)
                .unwrap()
                .build()
                .unwrap();
            let body: Value =
                serde_json::from_slice(request.body().unwrap().as_bytes().unwrap()).unwrap();
            let input = if provider.protocol() == Protocol::Gemini {
                body["contents"][0]["parts"][0]["functionCall"]["args"].clone()
            } else if provider.uses_responses() {
                serde_json::from_str::<Value>(body["input"][0]["arguments"].as_str().unwrap())
                    .unwrap()
            } else if provider.uses_messages() {
                body["messages"][0]["content"][0]["input"].clone()
            } else {
                serde_json::from_str::<Value>(
                    body["messages"][0]["tool_calls"][0]["function"]["arguments"]
                        .as_str()
                        .unwrap(),
                )
                .unwrap()
            };
            assert_eq!(input["_themis_rejected_tool_arguments"], raw);
            if let MessageType::ToolUse(calls) = &messages[0].message_type {
                assert_eq!(
                    calls[0].function.arguments, raw,
                    "original history stays exact"
                );
            }
        }
    }
}

#[tokio::test]
async fn tool_use_and_result_messages_reach_wire() {
    let _guard = ENV_LOCK.lock().unwrap();
    std::env::remove_var(GO_MODEL_ENV_VAR);
    std::env::remove_var(GO_SESSION_ENV_VAR);
    let server = MockServer::builder().start().await;
    mount_chat(&server, chat_ok_body("done")).await;

    let config = ProviderConfig::new(ProviderKind::Go, "key")
        .with_model("m")
        .with_base_url(server.uri());
    let handle = resolve(&config).await.unwrap();
    let call = ToolCall {
        id: "call_1".to_owned(),
        call_type: "function".to_owned(),
        function: FunctionCall {
            name: "read_file".to_owned(),
            arguments: "{\"path\": \"a\"}".to_owned(),
        },
    };
    let result = ToolCall {
        id: "call_1".to_owned(),
        call_type: "function".to_owned(),
        function: FunctionCall {
            name: "read_file".to_owned(),
            arguments: "file contents".to_owned(),
        },
    };
    let messages = [
        user_message("read it"),
        ChatMessage {
            role: ChatRole::Assistant,
            message_type: MessageType::ToolUse(vec![call]),
            content: String::new(),
        },
        ChatMessage {
            role: ChatRole::Tool,
            message_type: MessageType::ToolResult(vec![result]),
            content: String::new(),
        },
    ];
    handle.chat(&messages, None).await.unwrap();

    let requests = server.received_requests().await.unwrap();
    let body: Value = requests[0].body_json().unwrap();
    let sent = body["messages"].as_array().unwrap();
    assert_eq!(sent.len(), 3);
    assert_eq!(sent[1]["role"], "assistant");
    assert_eq!(sent[1]["tool_calls"][0]["id"], "call_1");
    assert_eq!(sent[2]["role"], "tool");
    assert_eq!(sent[2]["tool_call_id"], "call_1");
    assert_eq!(sent[2]["content"], "file contents");
}

/// Scripted assertion against the Go chat-completions route.
#[tokio::test]
async fn provider_matrix_openai_shaped() {
    let _guard = ENV_LOCK.lock().unwrap();
    for var in [GO_MODEL_ENV_VAR, GO_SESSION_ENV_VAR] {
        std::env::remove_var(var);
    }
    let kind = ProviderKind::Go;
    let server = MockServer::builder().start().await;
    mount_chat(&server, chat_ok_body("matrix-hi")).await;

    let config = ProviderConfig::new(kind, "key")
        .with_model("matrix-model")
        .with_base_url(server.uri());
    let handle: Arc<dyn LLMProvider> = resolve(&config).await.unwrap();
    let response = handle.chat(&[user_message("ping")], None).await.unwrap();
    assert_eq!(response.text().as_deref(), Some("matrix-hi"), "{kind}");

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1, "{kind}");
    let body: Value = requests[0].body_json().unwrap();
    assert_eq!(body["model"], "matrix-model", "{kind}");
    assert_eq!(body["messages"][0]["content"], "ping", "{kind}");
}

#[tokio::test]
async fn refresh_go_models_success() {
    let server = MockServer::builder().start().await;
    Mock::given(method("GET"))
        .and(path("/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "object": "list",
            "data": [
                {"id": "model-a", "object": "model"},
                {"id": "model-b", "created": 1}
            ]
        })))
        .mount(&server)
        .await;

    let models = refresh_go_models(&server.uri(), "key").await.unwrap();
    assert_eq!(models, vec!["model-a".to_owned(), "model-b".to_owned()]);
    assert!(server.received_requests().await.unwrap()[0]
        .headers
        .contains_key(GO_SESSION_HEADER));

    let requests = server.received_requests().await.unwrap();
    assert_eq!(
        requests[0]
            .headers
            .get("user-agent")
            .unwrap()
            .to_str()
            .unwrap(),
        themis_user_agent()
    );
}

#[tokio::test]
async fn refresh_go_models_malformed() {
    let server = MockServer::builder().start().await;
    Mock::given(method("GET"))
        .and(path("/models"))
        .respond_with(ResponseTemplate::new(200).set_body_string("not json{"))
        .mount(&server)
        .await;

    let err = refresh_go_models(&server.uri(), "key").await.unwrap_err();
    assert!(err.to_string().contains("failed to parse"), "{err}");
}

#[tokio::test]
async fn refresh_go_models_http_error() {
    let server = MockServer::builder().start().await;
    Mock::given(method("GET"))
        .and(path("/models"))
        .respond_with(ResponseTemplate::new(401).set_body_string("bad key"))
        .mount(&server)
        .await;

    let err = refresh_go_models(&server.uri(), "key").await.unwrap_err();
    assert!(err.to_string().contains("401"), "{err}");
    assert!(err.to_string().contains("bad key"), "{err}");
}

#[tokio::test]
async fn completion_and_embedding_roundtrip() {
    let _guard = ENV_LOCK.lock().unwrap();
    std::env::remove_var(GO_MODEL_ENV_VAR);
    std::env::remove_var(GO_SESSION_ENV_VAR);
    let server = MockServer::builder().start().await;
    mount_chat(&server, chat_ok_body("completed")).await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [{"embedding": [0.1, 0.2], "index": 0}]
        })))
        .mount(&server)
        .await;

    let config = ProviderConfig::new(ProviderKind::Go, "key")
        .with_model("m")
        .with_base_url(server.uri());
    let handle: Arc<dyn LLMProvider> = resolve(&config).await.unwrap();

    let completion = handle
        .complete(
            &CompletionRequest {
                prompt: "say it".to_owned(),
                max_tokens: Some(8),
                temperature: Some(0.5),
            },
            None,
        )
        .await
        .unwrap();
    assert_eq!(completion.text, "completed");

    let embeddings = handle.embed(vec!["hello".to_owned()]).await.unwrap();
    assert_eq!(embeddings, vec![vec![0.1_f32, 0.2_f32]]);
}

#[tokio::test]
async fn empty_go_key_rejected() {
    let config = ProviderConfig::new(ProviderKind::Go, "").with_model("m");
    assert!(resolve(&config).await.is_err());
}
