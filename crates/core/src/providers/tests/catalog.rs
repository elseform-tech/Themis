//! The model catalog, summaries, Gemini, configured providers, model lists and images.

use super::*;

#[test]
fn the_catalog_names_each_models_protocol_and_endpoint() {
    let catalog: CatalogRoot = serde_json::from_value(json!({
        "opencode-go": {
            "npm": "@ai-sdk/openai-compatible",
            "api": "https://opencode.ai/inference/go/openai/v1",
            "models": {
                "plain": {},
                "muse-like": {"provider": {"npm": "@ai-sdk/openai", "api": "https://opencode.ai/inference/go/openai/v1"}},
                "qwen-like": {"provider": {"npm": "@ai-sdk/anthropic", "api": "https://opencode.ai/inference/go/anthropic/v1/"}},
                "argon": {"provider": {"npm": "@ai-sdk/google", "api": "https://opencode.ai/inference/go/google/v1beta"}},
                "strange": {"provider": {"npm": "@ai-sdk/xyz", "api": "https://opencode.ai/inference/go/xyz/v1"}},
                "elsewhere": {"provider": {"npm": "@ai-sdk/anthropic", "api": "https://example.com/v1"}},
                "package-only": {"provider": {"npm": "@ai-sdk/anthropic"}}
            }
        }
    }))
    .unwrap();
    let ids = [
        "plain",
        "muse-like",
        "qwen-like",
        "argon",
        "strange",
        "elsewhere",
        "gemini-new",
        "package-only",
    ];
    let models = join_go_catalog(ids.map(String::from).to_vec(), catalog);
    let got: Vec<_> = models
        .iter()
        .map(|m| {
            (
                m.id.as_str(),
                m.protocol,
                m.endpoint.as_deref(),
                m.unavailable_reason.is_some(),
            )
        })
        .collect();
    assert_eq!(
        got,
        vec![
            (
                "plain",
                Some(Protocol::ChatCompletions),
                Some("https://opencode.ai/inference/go/openai/v1"),
                false
            ),
            (
                "muse-like",
                Some(Protocol::Responses),
                Some("https://opencode.ai/inference/go/openai/v1"),
                false
            ),
            (
                "qwen-like",
                Some(Protocol::Messages),
                Some("https://opencode.ai/inference/go/anthropic/v1"),
                false
            ),
            (
                "argon",
                Some(Protocol::Gemini),
                Some("https://opencode.ai/inference/go/google/v1beta"),
                false
            ),
            ("strange", Some(Protocol::Unsupported), None, true),
            // An endpoint off opencode.ai would receive the API key: Go's default is used.
            ("elsewhere", Some(Protocol::Messages), None, false),
            // Listed by the account before the catalog knows it: the name decides.
            ("gemini-new", None, None, false),
            // Another package without its own endpoint: not the provider's OpenAI endpoint.
            ("package-only", Some(Protocol::Messages), None, false),
        ]
    );
    assert_eq!(models[6].protocol(), Protocol::Gemini);
    assert!(models[4]
        .unavailable_reason
        .as_deref()
        .unwrap()
        .contains("@ai-sdk/xyz"));
}

#[tokio::test]
async fn requests_follow_the_catalog_protocol_and_endpoint_not_the_name() {
    let catalog = [
        GoModel {
            id: "muse-renamed".into(),
            protocol: Some(Protocol::Messages),
            endpoint: Some("https://opencode.ai/inference/go/anthropic/v1".into()),
            ..Default::default()
        },
        GoModel {
            id: "argon".into(),
            protocol: Some(Protocol::Gemini),
            ..Default::default()
        },
    ];
    with_catalog_limits(&catalog, async {
        let provider = CompatibleProvider::go("k".into(), "muse-renamed".into(), None, None);
        assert_eq!(provider.protocol(), Protocol::Messages);
        assert_eq!(
            provider.endpoint(Protocol::Messages),
            "https://opencode.ai/inference/go/anthropic/v1"
        );
        // Another protocol for the same model (embeddings, completions) uses Go's default.
        assert_eq!(
            provider.endpoint(Protocol::ChatCompletions),
            "https://opencode.ai/inference/go/openai/v1"
        );
        let argon = CompatibleProvider::go("k".into(), "argon".into(), None, None);
        assert_eq!(
            argon.endpoint(Protocol::Gemini),
            "https://opencode.ai/inference/go/google/v1beta"
        );
        // Not in the catalog: the name decides, at Go's default endpoint.
        let unknown = CompatibleProvider::go("k".into(), "gpt-9".into(), None, None);
        assert_eq!(unknown.protocol(), Protocol::Responses);
        // A test server serves every protocol from its one root.
        let local = CompatibleProvider::go(
            "k".into(),
            "argon".into(),
            Some("http://127.0.0.1:9/".into()),
            None,
        );
        assert_eq!(local.endpoint(Protocol::Gemini), "http://127.0.0.1:9");
    })
    .await;
}

#[tokio::test]
async fn an_unsupported_model_is_refused_before_any_request() {
    let catalog = [GoModel {
        id: "strange".into(),
        protocol: Some(Protocol::Unsupported),
        ..Default::default()
    }];
    let server = MockServer::builder().start().await;
    with_catalog_limits(&catalog, async {
        let provider =
            CompatibleProvider::go("k".into(), "strange".into(), Some(server.uri()), None);
        let error = provider
            .chat_with_tools(&[ChatMessage::user().content("hi").build()], None, None)
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("does not support yet"),
            "{error}"
        );
    })
    .await;
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn summaries_use_the_model_itself_once_go_retires_the_fallback_summarizer() {
    let with = |ids: &[&str]| {
        ids.iter()
            .map(|id| GoModel::listed(id.to_string()))
            .collect::<Vec<_>>()
    };
    let listed = with(&["glm-5.3", "longcat-2.5-preview-free"]);
    let retired = with(&["glm-5.3"]);
    assert_eq!(
        with_catalog_limits(&listed, async { summary_model("glm-5.3").to_owned() }).await,
        "longcat-2.5-preview-free"
    );
    assert_eq!(
        with_catalog_limits(&retired, async { summary_model("glm-5.3").to_owned() }).await,
        "glm-5.3"
    );
    let mut unusable = with(&["glm-5.3", "longcat-2.5-preview-free"]);
    unusable[1].protocol = Some(Protocol::Unsupported);
    assert_eq!(
        with_catalog_limits(&unusable, async { summary_model("glm-5.3").to_owned() }).await,
        "glm-5.3"
    );
}

#[tokio::test]
async fn a_summary_falls_back_to_the_chat_model_when_go_refuses_the_listed_summarizer() {
    // Seen live: Go listed longcat-2.5-preview-free while answering every request with
    // 402 "Endpoint is unavailable", so every context checkpoint failed.
    for (status, falls_back) in [
        (402, true),
        (404, true),
        (410, true),
        (500, false),
        (429, false),
    ] {
        let server = MockServer::builder().start().await;
        Mock::given(method("POST"))
            .respond_with(move |request: &wiremock::Request| {
                let body: Value = serde_json::from_slice(&request.body).unwrap();
                if body["model"] == "longcat-2.5-preview-free" {
                    ResponseTemplate::new(status).set_body_json(json!({"error":{"message":"Upstream request failed: Endpoint is unavailable."}}))
                } else {
                    ResponseTemplate::new(200).set_body_json(json!({"choices":[{"message":{"content":"checkpoint"},"finish_reason":"stop"}]}))
                }
            })
            .mount(&server)
            .await;
        let mut provider = CompatibleProvider::go(
            "synthetic".into(),
            "glm-5.3".into(),
            Some(server.uri()),
            Some("thread-session".into()),
        );
        provider.reasoning_effort = Some("high".into());
        let dir = tempfile::tempdir().unwrap();
        let log = Arc::new(crate::diagnostics::DiagnosticLog::new(dir.path().into()));
        let summary = crate::diagnostics::ACTIVE
            .scope(
                crate::diagnostics::DiagnosticContext {
                    log: Arc::clone(&log),
                    thread_id: "thread".into(),
                    run_id: "run".into(),
                    level: "info".into(),
                },
                COMPACTION_REQUEST.scope(
                    (),
                    provider.chat_and_sampling(&[user_message("summarize")], None, None),
                ),
            )
            .await;
        let requests = server.received_requests().await.unwrap();
        let models: Vec<Value> = requests
            .iter()
            .map(|request| serde_json::from_slice::<Value>(&request.body).unwrap()["model"].clone())
            .collect();
        let events: Vec<String> = log
            .query(&crate::diagnostics::DiagnosticFilter::default())
            .unwrap()
            .into_iter()
            .map(|record| record.event)
            .collect();
        if falls_back {
            assert_eq!(
                summary.unwrap().text().as_deref(),
                Some("checkpoint"),
                "{status}"
            );
            assert_eq!(
                models,
                [json!("longcat-2.5-preview-free"), json!("glm-5.3")],
                "{status}"
            );
            let retry: Value = serde_json::from_slice(&requests[1].body).unwrap();
            assert!(
                retry.get("reasoning_effort").is_none(),
                "summaries never think"
            );
            assert_eq!(requests[1].headers[GO_SESSION_HEADER], "thread-session");
            assert!(
                events
                    .iter()
                    .any(|event| event == "summary_model_unavailable"),
                "{events:?}"
            );
        } else {
            // Transient errors stay with the summary retry loop, on the routed model.
            assert!(summary.is_err(), "{status}");
            assert_eq!(models, [json!("longcat-2.5-preview-free")], "{status}");
        }
    }
    // An ordinary chat request is never redirected.
    let server = MockServer::builder().start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(402))
        .mount(&server)
        .await;
    let provider = CompatibleProvider::go(
        "synthetic".into(),
        "glm-5.3".into(),
        Some(server.uri()),
        None,
    );
    assert!(provider
        .chat_and_sampling(&[user_message("hi")], None, None)
        .await
        .is_err());
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn a_summary_asks_for_the_lowest_effort_the_model_accepts() {
    // Seen live: muse-spark rejects reasoning effort "none" (its lowest is "minimal"), so a
    // checkpoint written with the chat's own model failed with HTTP 400.
    let listed = |id: &str, efforts: &[&str]| GoModel {
        effort_levels: efforts.iter().map(|effort| effort.to_string()).collect(),
        ..GoModel::listed(id.to_string())
    };
    let catalog = [
        listed(
            "muse-spark-1.3-contributor",
            &["minimal", "low", "medium", "high", "xhigh"],
        ),
        listed(
            "gpt-6-luna",
            &["none", "low", "medium", "high", "xhigh", "max"],
        ),
        listed("grok-4.7", &["low", "high"]),
    ];
    for (model, effort) in [
        ("muse-spark-1.3-contributor", "minimal"),
        ("gpt-6-luna", "none"),
        ("grok-4.7", "low"),
    ] {
        let server = MockServer::builder().start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "output":[{"type":"message","content":[{"type":"output_text","text":"checkpoint"}]}],
                "choices":[{"message":{"content":"checkpoint"},"finish_reason":"stop"}]})))
            .mount(&server)
            .await;
        let provider =
            CompatibleProvider::go("synthetic".into(), model.into(), Some(server.uri()), None);
        with_catalog_limits(
            &catalog,
            COMPACTION_REQUEST.scope(
                (),
                provider.chat_and_sampling(&[user_message("summarize")], None, None),
            ),
        )
        .await
        .unwrap();
        let body: Value =
            serde_json::from_slice(&server.received_requests().await.unwrap()[0].body).unwrap();
        assert_eq!(
            body["model"], model,
            "the catalog has no summarizer, so the model writes it"
        );
        if provider.uses_responses() {
            assert_eq!(body["reasoning"]["effort"], effort, "{model}");
        }
    }
    // A model the catalog does not describe keeps the plain off control.
    assert_eq!(summary_effort("gpt-5.6-luna"), "none");
}

proptest::proptest! {
    #[test]
    fn catalog_endpoints_off_opencode_are_never_trusted(url in "\\PC{0,80}") {
        if let Some(trusted) = trusted_endpoint(&url) {
            proptest::prop_assert!(trusted.starts_with("https://opencode.ai/"));
            proptest::prop_assert!(!trusted.contains(".."));
            proptest::prop_assert!(!trusted["https://opencode.ai/".len()..].contains("//"));
        }
    }

    #[test]
    fn any_catalog_json_joins_without_panicking(raw in "\\PC{0,200}", ids in proptest::collection::vec("[a-z0-9.-]{0,12}", 0..4)) {
        if let Ok(catalog) = serde_json::from_str::<CatalogRoot>(&raw) {
            let models = join_go_catalog(ids.clone(), catalog);
            proptest::prop_assert_eq!(models.len(), ids.len());
        }
    }
}

#[test]
fn catalog_effort_levels_follow_go_metadata_and_live_ids() {
    let catalog: CatalogRoot = serde_json::from_value(json!({
        "opencode-go": {"models": {
            "model-a": {"limit":{"context":1000000,"output":65536},"reasoning_options": [{"type":"toggle"}, {"type":"effort", "values":["low","high","max"]}]},
            "model-b": {"reasoning_options": []},
            "not-available": {"reasoning_options": [{"type":"effort", "values":["low"]}]}
        }}
    })).unwrap();
    assert_eq!(
        join_go_catalog(vec!["model-a".into(), "model-b".into()], catalog),
        vec![
            GoModel {
                id: "model-a".into(),
                effort_levels: vec!["low".into(), "high".into(), "max".into()],
                context_tokens: Some(1000000),
                output_tokens: Some(65536),
                thinking_toggle: true,
                protocol: Some(Protocol::ChatCompletions),
                ..Default::default()
            },
            GoModel {
                id: "model-b".into(),
                effort_levels: vec![],
                context_tokens: None,
                output_tokens: None,
                thinking_toggle: false,
                protocol: Some(Protocol::ChatCompletions),
                ..Default::default()
            },
        ]
    );
}

fn gemini_reply(parts: Value) -> Value {
    json!({"candidates": [{"finishReason": "STOP", "content": {"role": "model", "parts": parts}}],
           "usageMetadata": {"promptTokenCount": 40, "candidatesTokenCount": 8, "thoughtsTokenCount": 5, "totalTokenCount": 53}})
}

#[tokio::test]
async fn gemini_round_trip_keeps_the_signature_with_its_call() {
    let server = MockServer::builder().start().await;
    Mock::given(method("POST"))
        .and(path("/models/gemini-test:generateContent"))
        .respond_with(ResponseTemplate::new(200).set_body_json(gemini_reply(json!([
            {"thoughtSignature": "SIG-ROUND-TRIP", "functionCall": {"id": "call_g_round", "name": "read_file", "args": {"path": "a"}}}
        ]))))
        .mount(&server)
        .await;
    let provider = resolve(
        &ProviderConfig::new(ProviderKind::Go, "synthetic-key")
            .with_model("gemini-test")
            .with_base_url(server.uri()),
    )
    .await
    .unwrap();
    let tools = [test_tool()];
    let system = ChatMessage {
        role: ChatRole::System,
        message_type: MessageType::Text,
        content: "Be helpful.".into(),
    };
    let response = provider
        .chat_with_tools(&[system, user_message("read it")], Some(&tools), None)
        .await
        .unwrap();
    let calls = response.tool_calls().unwrap();
    assert_eq!(calls[0].id, "call_g_round");
    assert_eq!(calls[0].function.arguments, r#"{"path":"a"}"#);
    assert_eq!(response.usage().unwrap().completion_tokens, 13);
    let mut result = calls[0].clone();
    result.function.arguments = "contents".into();
    provider
        .chat_with_tools(
            &[
                user_message("read it"),
                ChatMessage {
                    role: ChatRole::Assistant,
                    message_type: MessageType::ToolUse(calls),
                    content: String::new(),
                },
                ChatMessage {
                    role: ChatRole::Tool,
                    message_type: MessageType::ToolResult(vec![result]),
                    content: String::new(),
                },
            ],
            Some(&tools),
            None,
        )
        .await
        .unwrap();
    let requests = server.received_requests().await.unwrap();
    for request in &requests {
        assert_eq!(request.headers["x-goog-api-key"], "synthetic-key");
        assert!(!request.headers.contains_key("authorization"));
        assert!(request.headers.contains_key(GO_SESSION_HEADER));
    }
    let first: Value = requests[0].body_json().unwrap();
    assert_eq!(
        first["systemInstruction"]["parts"][0]["text"],
        "Be helpful."
    );
    assert_eq!(
        first["contents"][0],
        json!({"role": "user", "parts": [{"text": "read it"}]})
    );
    let declaration = &first["tools"][0]["functionDeclarations"][0];
    assert_eq!(declaration["name"], "read_file");
    assert_eq!(declaration["parametersJsonSchema"]["type"], "object");
    let second: Value = requests[1].body_json().unwrap();
    let call = &second["contents"][1];
    assert_eq!(call["role"], "model");
    assert_eq!(call["parts"][0]["thoughtSignature"], "SIG-ROUND-TRIP");
    assert_eq!(
        call["parts"][0]["functionCall"]["args"],
        json!({"path": "a"})
    );
    assert_eq!(
        second["contents"][2]["parts"][0]["functionResponse"],
        json!({"id": "call_g_round", "name": "read_file", "response": {"result": "contents"}})
    );
}

#[test]
fn a_gemini_call_without_its_kept_signature_sends_googles_placeholder() {
    let call = |id: &str| ToolCall {
        id: id.into(),
        call_type: "function".into(),
        function: FunctionCall {
            name: "read_file".into(),
            arguments: r#"{"path":"a"}"#.into(),
        },
    };
    let calls = vec![call("never-seen-1"), call("never-seen-2")];
    let mut results = calls.clone();
    for result in &mut results {
        result.function.arguments = "done".into();
    }
    let body = gemini::gemini_body(
        &[
            user_message("read both"),
            ChatMessage {
                role: ChatRole::Assistant,
                message_type: MessageType::ToolUse(calls),
                content: String::new(),
            },
            ChatMessage {
                role: ChatRole::Tool,
                message_type: MessageType::ToolResult(results),
                content: String::new(),
            },
        ],
        None,
    )
    .unwrap();
    let parts = &body["contents"][1]["parts"];
    assert_eq!(
        parts[0]["thoughtSignature"],
        "skip_thought_signature_validator"
    );
    // Only the first call of a step carries the signature.
    assert!(parts[1].get("thoughtSignature").is_none());
    // Parallel results go back in one user turn.
    assert_eq!(body["contents"].as_array().unwrap().len(), 3);
    assert_eq!(body["contents"][2]["parts"].as_array().unwrap().len(), 2);
    assert!(body.get("tools").is_none());
    // A user message after the results starts its own turn.
    let later = [
        user_message("read it"),
        ChatMessage {
            role: ChatRole::Assistant,
            message_type: MessageType::ToolUse(vec![call("never-seen-3")]),
            content: String::new(),
        },
        ChatMessage {
            role: ChatRole::Tool,
            message_type: MessageType::ToolResult(vec![call("never-seen-3")]),
            content: String::new(),
        },
        user_message("thanks, now stop"),
    ];
    let body = gemini::gemini_body(&later, None).unwrap();
    let contents = body["contents"].as_array().unwrap();
    assert_eq!(contents.len(), 4);
    assert!(contents[2]["parts"][0]["functionResponse"].is_object());
    assert_eq!(
        contents[3],
        json!({"role": "user", "parts": [{"text": "thanks, now stop"}]})
    );
}

#[tokio::test]
async fn gemini_streams_text_and_whole_tool_calls() {
    use futures_util::StreamExt;
    let server = MockServer::builder().start().await;
    let events = [
        json!({"candidates": [{"content": {"role": "model", "parts": [{"text": "Look"}]}}]}),
        json!({"candidates": [{"content": {"role": "model", "parts": [{"text": "ing.", "thought": false}, {"text": "private summary", "thought": true}]}}]}),
        json!({"candidates": [{"content": {"role": "model", "parts": [
            {"thoughtSignature": "SIG-STREAM", "functionCall": {"id": "call_s1", "name": "read_file", "args": {"path": "a"}}},
            {"functionCall": {"name": "read_file", "args": {"path": "b"}}}
        ]}}]}),
        json!({"candidates": [{"finishReason": "STOP", "content": {"role": "model", "parts": [{"text": ""}]}}],
               "usageMetadata": {"promptTokenCount": 10, "candidatesTokenCount": 4, "totalTokenCount": 14}}),
    ]
    .iter()
    .map(|event| format!("data: {event}\r\n\r\n"))
    .collect::<String>();
    Mock::given(method("POST"))
        .and(path("/models/gemini-test:streamGenerateContent"))
        .and(wiremock::matchers::query_param("alt", "sse"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(events, "text/event-stream"))
        .mount(&server)
        .await;
    let provider = CompatibleProvider::go(
        "synthetic".into(),
        "gemini-test".into(),
        Some(server.uri()),
        None,
    );
    let mut stream = provider
        .chat_stream_with_tools(&[user_message("hi")], Some(&[test_tool()]), None)
        .await
        .unwrap();
    let (mut text, mut calls, mut indexes, mut stop) =
        (String::new(), Vec::new(), Vec::new(), None);
    while let Some(chunk) = stream.next().await {
        match chunk.unwrap() {
            StreamChunk::Text(delta) => text.push_str(&delta),
            StreamChunk::ToolUseComplete { index, tool_call } => {
                indexes.push(index);
                calls.push(tool_call);
            }
            StreamChunk::Done { stop_reason } => stop = Some(stop_reason),
            _ => {}
        }
    }
    assert_eq!(text, "Looking.");
    assert_eq!(indexes, vec![0, 1]);
    assert_eq!(calls[0].id, "call_s1");
    assert!(calls[1].id.starts_with("gemini_call_"), "{}", calls[1].id);
    assert_eq!(calls[1].function.arguments, r#"{"path":"b"}"#);
    assert_eq!(stop.as_deref(), Some("stop"));
    // The streamed signature goes back with the first call of the step.
    let body = gemini::gemini_body(
        &[ChatMessage {
            role: ChatRole::Assistant,
            message_type: MessageType::ToolUse(calls),
            content: String::new(),
        }],
        None,
    )
    .unwrap();
    assert_eq!(
        body["contents"][0]["parts"][0]["thoughtSignature"],
        "SIG-STREAM"
    );
}

#[tokio::test]
async fn gemini_settings_go_in_generation_config() {
    let body = |effort: Option<&str>| {
        let mut provider = CompatibleProvider::go("k".into(), "gemini-test".into(), None, None);
        provider.reasoning_effort = effort.map(str::to_owned);
        let request = provider
            .chat_request(&[user_message("hi")], None, false, None)
            .unwrap()
            .build()
            .unwrap();
        assert!(request
            .url()
            .as_str()
            .ends_with("/models/gemini-test:generateContent"));
        assert_eq!(
            request.url().host_str(),
            Some("opencode.ai"),
            "Go's Gemini endpoint by default"
        );
        serde_json::from_slice::<Value>(request.body().unwrap().as_bytes().unwrap()).unwrap()
    };
    let capped = OUTPUT_LIMIT.scope(4096, async { body(Some("high")) }).await;
    assert_eq!(capped["generationConfig"]["maxOutputTokens"], 4096);
    assert_eq!(
        capped["generationConfig"]["thinkingConfig"]["thinkingLevel"],
        "high"
    );
    let catalog = [GoModel {
        id: "gemini-test".into(),
        effort_levels: vec!["low".into(), "medium".into(), "high".into()],
        ..Default::default()
    }];
    let default = with_catalog_limits(&catalog, async { body(None) }).await;
    assert_eq!(
        default["generationConfig"]["thinkingConfig"]["thinkingLevel"],
        "medium"
    );
    let summary = COMPACTION_REQUEST
        .scope((), async { body(Some("high")) })
        .await;
    assert_eq!(
        summary["generationConfig"]["thinkingConfig"]["thinkingLevel"],
        "low"
    );
    assert!(gemini::gemini_path("a/b", false).is_err());
    assert!(gemini::gemini_path("x?alt=json", true).is_err());
}

#[test]
fn gemini_cut_off_and_blocked_replies_are_errors() {
    let reply = |reason: &str| json!({"candidates": [{"finishReason": reason, "content": {"parts": [{"text": "partial"}]}}]});
    let truncated = gemini::gemini_response(reply("MAX_TOKENS")).err().unwrap();
    assert!(truncated.to_string().contains("truncated"), "{truncated}");
    let blocked = gemini::gemini_response(reply("SAFETY")).err().unwrap();
    assert!(blocked.to_string().contains("SAFETY"), "{blocked}");
    assert!(gemini::gemini_stream_event(reply("RECITATION"), 0).is_err());
    let empty = gemini::gemini_response(gemini_reply(json!([])))
        .err()
        .unwrap();
    assert!(empty.to_string().contains("no text or tools"), "{empty}");
    let error = gemini::gemini_response(json!({"error": {"code": 400}}))
        .err()
        .unwrap();
    assert!(error.to_string().contains("400"), "{error}");
}

proptest::proptest! {
    #[test]
    fn any_gemini_parts_convert_without_panicking(
        parts in proptest::collection::vec(
            (0usize..5, "[ -~]{0,10}", proptest::option::of("[A-Z_]{0,10}")),
            0..6,
        )
    ) {
        let parts: Vec<Value> = parts
            .iter()
            .map(|(kind, text, _)| match kind {
                0 => json!({"text": text}),
                1 => json!({"text": text, "thought": true}),
                2 => json!({"functionCall": {"name": text, "args": {"x": text}}}),
                3 => json!({"functionCall": {"id": text, "name": text, "args": text}}),
                _ => json!({"thoughtSignature": text}),
            })
            .collect();
        let reason = parts.len().is_multiple_of(2);
        let value = json!({"candidates": [{"finishReason": if reason { json!("STOP") } else { Value::Null }, "content": {"parts": parts}}]});
        if let Ok(event) = gemini::gemini_stream_event(value.clone(), 3) {
            for call in event["choices"][0]["delta"]["tool_calls"].as_array().into_iter().flatten() {
                proptest::prop_assert!(call["index"].as_u64().unwrap() >= 3);
                let args: Value = serde_json::from_str(call["function"]["arguments"].as_str().unwrap()).unwrap();
                proptest::prop_assert!(args.is_object());
            }
        }
        let _ = gemini::gemini_response(value);
    }
}

fn local_endpoint(server: &MockServer) -> CustomEndpoint {
    CustomEndpoint {
        id: "local".into(),
        name: "Local server".into(),
        base_url: format!("{}/v1/", server.uri()),
    }
}

#[tokio::test]
async fn a_configured_provider_gets_its_own_model_id_key_and_no_opencode_fields() {
    let server = MockServer::builder().start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(chat_ok_body("hello")))
        .mount(&server)
        .await;
    let mut config = ProviderConfig::new(ProviderKind::Go, "local-key")
        .with_model("local/qwen/coder")
        .with_base_url("http://ignored.invalid");
    config.endpoint = Some(local_endpoint(&server));
    config.session_id = Some("thread-1".into());
    let llm = resolve(&config).await.unwrap();
    assert_eq!(
        llm.model(),
        "local/qwen/coder",
        "the catalog id stays qualified"
    );
    let catalog = [GoModel {
        id: "local/qwen/coder".into(),
        protocol: Some(Protocol::ChatCompletions),
        provider: Some("local".into()),
        output_tokens: Some(2048),
        ..Default::default()
    }];
    let messages = [user_message("hi")];
    with_catalog_limits(&catalog, async {
        llm.chat(&messages, None).await.unwrap();
        COMPACTION_REQUEST
            .scope(
                (),
                llm.chat_and_sampling(
                    &messages,
                    None,
                    Some(&SamplingOverrides::with_max_tokens(4096)),
                ),
            )
            .await
            .unwrap();
    })
    .await;
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 2);
    for request in &requests {
        assert_eq!(request.headers["authorization"], "Bearer local-key");
        assert!(!request.headers.contains_key(GO_SESSION_HEADER));
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        assert_eq!(body["model"], "qwen/coder");
        assert!(body.get("thinking").is_none(), "{body}");
        assert!(body.get("reasoning_effort").is_none(), "{body}");
    }
    let summary: Value = serde_json::from_slice(&requests[1].body).unwrap();
    assert_eq!(
        summary["max_tokens"], 2048,
        "capped by the declared output limit"
    );
}

#[tokio::test]
async fn a_summary_model_on_another_provider_uses_that_provider_and_key() {
    let go = MockServer::builder().start().await;
    mount_chat(&go, chat_ok_body("answer")).await;
    let local = MockServer::builder().start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(chat_ok_body("summary")))
        .mount(&local)
        .await;
    let mut summary = ProviderConfig::new(ProviderKind::Go, "local-key").with_model("local/sum");
    summary.endpoint = Some(local_endpoint(&local));
    let mut config = ProviderConfig::new(ProviderKind::Go, "go-key")
        .with_model("glm-5.3")
        .with_base_url(go.uri());
    config.summary = Some(Box::new(summary));
    let llm = resolve(&config).await.unwrap();
    let catalog = [
        GoModel::listed("glm-5.3".into()),
        GoModel {
            id: "local/sum".into(),
            provider: Some("local".into()),
            protocol: Some(Protocol::ChatCompletions),
            ..Default::default()
        },
    ];
    let compaction = serde_json::from_value::<crate::configuration::CompactionConfig>(
        json!({"summary_model":"local/sum"}),
    )
    .unwrap();
    let messages = [user_message("Summarize")];
    with_catalog_limits(
        &catalog,
        crate::runtime::with_configuration(compaction, async {
            COMPACTION_REQUEST
                .scope((), llm.chat(&messages, None))
                .await
                .unwrap();
            llm.chat(&messages, None).await.unwrap();
        }),
    )
    .await;
    let summary = &local.received_requests().await.unwrap()[0];
    assert_eq!(summary.headers["authorization"], "Bearer local-key");
    let body: Value = serde_json::from_slice(&summary.body).unwrap();
    assert_eq!(body["model"], "sum");
    let answer = &go.received_requests().await.unwrap()[0];
    assert_eq!(answer.headers["authorization"], "Bearer go-key");
    assert_eq!(
        serde_json::from_slice::<Value>(&answer.body).unwrap()["model"],
        "glm-5.3"
    );
}

#[tokio::test]
async fn a_configured_providers_model_summarizes_with_itself() {
    let catalog = [
        GoModel::listed("longcat-2.5-preview-free".into()),
        GoModel {
            id: "local/m".into(),
            provider: Some("local".into()),
            ..Default::default()
        },
    ];
    let (local, go) = with_catalog_limits(&catalog, async {
        (summary_model("local/m"), summary_model("glm-5.3"))
    })
    .await;
    assert_eq!(local, "local/m");
    assert_eq!(go, "longcat-2.5-preview-free");
}

#[test]
fn model_overrides_cap_limits_and_choose_thinking_where_the_catalog_allows() {
    let overrides: std::collections::BTreeMap<String, crate::configuration::ModelOverride> =
        serde_json::from_value(json!({
            "glm-5.3": {"context_tokens": 100000, "output_tokens": 4096, "thinking": false,
                        "reasoning_effort": "max"},
            "go/muse": {"reasoning_effort": "high", "thinking": true},
            "local/m": {"context_tokens": 50000, "reasoning_effort": "low"},
        }))
        .unwrap();
    let catalog = vec![
        GoModel {
            id: "glm-5.3".into(),
            context_tokens: Some(200000),
            output_tokens: Some(2048),
            thinking_toggle: true,
            effort_levels: vec!["high".into()],
            ..Default::default()
        },
        GoModel {
            id: "muse".into(),
            effort_levels: vec!["low".into(), "high".into()],
            ..Default::default()
        },
        GoModel {
            id: "local/m".into(),
            provider: Some("local".into()),
            ..Default::default()
        },
    ];
    let applied = apply_model_overrides(&catalog, &overrides);
    assert_eq!(applied[0].context_tokens, Some(100000), "lowered");
    assert_eq!(applied[0].output_tokens, Some(2048), "never raised");
    assert_eq!(applied[0].thinking, Some(false));
    assert_eq!(applied[0].default_effort, None, "max is not offered");
    assert_eq!(applied[1].default_effort.as_deref(), Some("high"));
    assert_eq!(applied[1].thinking, None, "muse has no thinking switch");
    assert_eq!(
        applied[2].context_tokens,
        Some(50000),
        "an unknown limit takes the cap"
    );
    assert_eq!(applied[2].default_effort.as_deref(), Some("low"));
    // A run refuses what its model cannot do, and only for its own model.
    assert!(check_model_override(&catalog, "glm-5.3", &overrides)
        .unwrap_err()
        .contains("'max'"));
    assert!(check_model_override(&catalog, "muse", &overrides)
        .unwrap_err()
        .contains("no thinking switch"));
    assert!(check_model_override(&catalog, "local/m", &overrides).is_ok());
    assert!(check_model_override(&catalog, "unlisted", &overrides).is_ok());

    // The thinking switch turned off goes out as disabled, with no default effort.
    let (glm, muse) = MODEL_THINKING.sync_scope(thinking_table(&applied), || {
        (thinking_default("glm-5.3"), thinking_default("muse"))
    });
    assert!(glm.switch_off && !glm.switch_on && glm.effort.is_none());
    assert_eq!(muse.effort.as_deref(), Some("high"));
    let body = MODEL_THINKING.sync_scope(thinking_table(&applied), || {
        let provider = CompatibleProvider::go("k".into(), "glm-5.3".into(), None, None);
        let request = provider
            .chat_request(&[user_message("hi")], None, false, None)
            .unwrap()
            .build()
            .unwrap();
        serde_json::from_slice::<Value>(request.body().unwrap().as_bytes().unwrap()).unwrap()
    });
    assert_eq!(body["thinking"], json!({"type":"disabled"}));

    // A short extraction (the memory pass) gets the least thinking, on the model it asked for:
    // no summary-model swap, unlike a checkpoint summary.
    let quiet = |model: &str| {
        MODEL_THINKING.sync_scope(thinking_table(&applied), || {
            super::LEAST_THINKING.sync_scope((), || {
                let provider = CompatibleProvider::go("k".into(), model.into(), None, None);
                let request = provider
                    .chat_request(&[user_message("hi")], None, false, None)
                    .unwrap()
                    .build()
                    .unwrap();
                serde_json::from_slice::<Value>(request.body().unwrap().as_bytes().unwrap())
                    .unwrap()
            })
        })
    };
    let glm = quiet("glm-5.3");
    assert_eq!(glm["model"], "glm-5.3");
    assert_eq!(glm["thinking"], json!({"type":"disabled"}));
    let muse = quiet("muse");
    assert_eq!(muse["model"], "muse");
    assert_ne!(muse["reasoning_effort"], "high", "{muse}");
}

#[test]
fn a_models_list_keeps_unique_usable_ids_only() {
    let body = json!({"object":"list","data":[
        {"id":"a"},{"id":"a"},{"id":"has space"},{"id":7},{"name":"x"},{"id":"b/c"},{"id":""}
    ]})
    .to_string();
    assert_eq!(parse_model_ids(&body).unwrap(), ["a", "b/c"]);
    for bad in ["", "not json", "{}", r#"{"data":{"x":"leak"}}"#, "[]"] {
        let error = parse_model_ids(bad).unwrap_err().to_string();
        assert!(
            !error.contains("leak") && !error.contains("not json"),
            "{error}"
        );
    }
    let many =
        json!({"data": (0..2000).map(|n| json!({"id": format!("m{n}")})).collect::<Vec<_>>()});
    assert_eq!(
        parse_model_ids(&many.to_string()).unwrap().len(),
        crate::configuration::MAX_PROVIDER_MODELS
    );
}

#[tokio::test]
async fn a_providers_models_list_is_read_with_its_key_and_errors_hide_the_body() {
    let server = MockServer::builder().start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .and(wiremock::matchers::header(
            "authorization",
            "Bearer local-key",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data":[{"id":"m1"}]})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(401).set_body_string("secret-body-text"))
        .mount(&server)
        .await;
    let base = format!("{}/v1", server.uri());
    assert_eq!(
        refresh_provider_models(&base, "local-key").await.unwrap(),
        ["m1"]
    );
    let error = refresh_provider_models(&base, "wrong")
        .await
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("401") && !error.contains("secret-body-text"),
        "{error}"
    );
    let provider: crate::configuration::ProviderDefinition = serde_json::from_value(json!({
        "id":"local","name":"Local","base_url":base,"secret":"${K}"
    }))
    .unwrap();
    let listed = provider_models(&provider, &["m1".into()]);
    assert_eq!(listed[0].id, "local/m1");
    assert_eq!(listed[0].provider.as_deref(), Some("local"));
}

proptest::proptest! {
    /// Any `/models` body parses or fails without panicking; kept ids are unique, usable as
    /// model ids, and come from the body.
    #[test]
    fn any_models_list_body_is_parsed_safely(
        body in "\\PC{0,200}",
        ids in proptest::collection::vec("\\PC{0,20}", 0..20),
    ) {
        let _ = parse_model_ids(&body);
        let wrapped = json!({"data": ids.iter().map(|id| json!({"id": id})).collect::<Vec<_>>()}).to_string();
        let kept = parse_model_ids(&wrapped).unwrap();
        let unique: std::collections::HashSet<_> = kept.iter().collect();
        proptest::prop_assert_eq!(unique.len(), kept.len());
        for id in &kept {
            proptest::prop_assert!(crate::configuration::valid_model_id(id));
            proptest::prop_assert!(ids.contains(id));
        }
    }
}

#[test]
fn the_usage_ledger_records_a_configured_providers_own_id() {
    let go = CompatibleProvider::go("k".into(), "glm-5.3".into(), None, None);
    assert_eq!(go.provider_id(), GO_PROVIDER_ID);
    let endpoint = CustomEndpoint {
        id: "local".into(),
        name: "Local server".into(),
        base_url: "http://127.0.0.1:9/v1".into(),
    };
    let local = CompatibleProvider::custom(&endpoint, "k".into(), "local/qwen3".into());
    assert_eq!(local.provider_id(), "local");
}

#[test]
fn the_catalog_reads_which_models_accept_images() {
    let catalog: CatalogRoot = serde_json::from_value(json!({
        "opencode-go": {
            "npm": "@ai-sdk/openai-compatible",
            "models": {
                "muse-spark-1.3-contributor": {"modalities": {"input": ["text", "image"], "output": ["text"]}},
                "iceman-contributor-free": {"modalities": {"input": ["text"], "output": ["text"]}},
                "odd": {"modalities": {"input": "image"}},
                "broken": {"modalities": 7},
                "bare": {}
            }
        }
    }))
    .unwrap();
    let ids = [
        "muse-spark-1.3-contributor",
        "iceman-contributor-free",
        "odd",
        "broken",
        "bare",
        "unlisted",
    ];
    let models = join_go_catalog(ids.map(String::from).to_vec(), catalog);
    let images: Vec<_> = models.iter().map(|m| (m.id.as_str(), m.images)).collect();
    assert_eq!(
        images,
        vec![
            ("muse-spark-1.3-contributor", true),
            ("iceman-contributor-free", false),
            // Anything malformed reads as text only, and never hides the model.
            ("odd", false),
            ("broken", false),
            ("bare", false),
            ("unlisted", false),
        ]
    );
    assert!(model_accepts_images(&models, "muse-spark-1.3-contributor"));
    assert!(!model_accepts_images(&models, "iceman-contributor-free"));
    assert!(!model_accepts_images(&models, "not-in-the-catalog"));
    // Inside a run the loaded catalog answers; with none loaded nothing accepts images.
    assert!(!accepts_images("muse-spark-1.3-contributor"));
    let scoped = MODEL_ROUTES.sync_scope(route_table(&models), || {
        (
            accepts_images("muse-spark-1.3-contributor"),
            accepts_images("iceman-contributor-free"),
            accepts_images("unknown"),
        )
    });
    assert_eq!(scoped, (true, false, false));
    // The person's `models` override may turn image input off, or on for a model the
    // catalog does not describe.
    let overrides: std::collections::BTreeMap<String, crate::configuration::ModelOverride> =
        serde_json::from_value(json!({
            "muse-spark-1.3-contributor": {"images": false},
            "go/bare": {"images": true},
        }))
        .unwrap();
    let applied = apply_model_overrides(&models, &overrides);
    assert!(!applied[0].images && applied[4].images && !applied[1].images);
}

/// A browser `screenshot` call, its result, and the image message after it.
fn screenshot_history(image: &[u8]) -> Vec<ChatMessage> {
    let call = ToolCall {
        id: "shot-1".into(),
        call_type: "function".into(),
        function: FunctionCall {
            name: "browser".into(),
            arguments: r#"{"action":"screenshot"}"#.into(),
        },
    };
    vec![
        ChatMessage {
            role: ChatRole::System,
            message_type: MessageType::Text,
            content: "system".into(),
        },
        user_message("look at the page"),
        ChatMessage {
            role: ChatRole::Assistant,
            message_type: MessageType::ToolUse(vec![call.clone()]),
            content: String::new(),
        },
        ChatMessage {
            role: ChatRole::Tool,
            message_type: MessageType::ToolResult(vec![ToolCall {
                function: FunctionCall {
                    name: "browser".into(),
                    arguments: r#"{"path":"/p/shot.png"}"#.into(),
                },
                ..call
            }]),
            content: String::new(),
        },
        ChatMessage {
            role: ChatRole::User,
            message_type: MessageType::Image((
                autoagents::llm::chat::ImageMime::JPEG,
                image.to_vec(),
            )),
            content: "The screenshot.".into(),
        },
    ]
}

fn request_body(model: &str, images: bool, messages: &[ChatMessage]) -> Result<Value, LLMError> {
    let catalog = [GoModel {
        id: model.into(),
        images,
        ..GoModel::default()
    }];
    MODEL_ROUTES.sync_scope(route_table(&catalog), || {
        let provider = CompatibleProvider::go("k".into(), model.into(), None, None);
        let request = provider
            .chat_request(messages, None, false, None)?
            .build()
            .unwrap();
        Ok(serde_json::from_slice(request.body().unwrap().as_bytes().unwrap()).unwrap())
    })
}

#[test]
fn an_image_goes_to_an_image_model_in_each_protocols_own_shape() {
    use base64::Engine;
    let image = b"\xff\xd8\xff-jpeg-bytes".to_vec();
    let data = base64::engine::general_purpose::STANDARD.encode(&image);
    let url = format!("data:image/jpeg;base64,{data}");
    let messages = screenshot_history(&image);

    // Chat Completions: the tool result stays a string; the image follows in a user message.
    let body = request_body("mimo-v2.6-flash", true, &messages).unwrap();
    let wire = body["messages"].as_array().unwrap();
    assert_eq!(wire.len(), 5);
    assert_eq!(wire[3]["role"], "tool");
    assert_eq!(wire[3]["content"], r#"{"path":"/p/shot.png"}"#);
    assert_eq!(
        wire[4],
        json!({"role":"user","content":[
            {"type":"text","text":"The screenshot."},
            {"type":"image_url","image_url":{"url":url}}
        ]})
    );

    // Responses: an input_image part in a user message after the function output.
    let body = request_body("muse-spark-1.3-contributor", true, &messages).unwrap();
    let input = body["input"].as_array().unwrap();
    let last = input.last().unwrap();
    assert_eq!(input[input.len() - 2]["type"], "function_call_output");
    assert_eq!(
        *last,
        json!({"role":"user","content":[
            {"type":"input_text","text":"The screenshot."},
            {"type":"input_image","image_url":url}
        ]})
    );

    // Messages: the image joins the user turn that carries the tool result, after it.
    let body = request_body("claude-haiku-4-5", true, &messages).unwrap();
    let turns = body["messages"].as_array().unwrap();
    let last = turns.last().unwrap();
    assert_eq!(last["role"], "user");
    assert_eq!(last["content"][0]["type"], "tool_result");
    assert_eq!(
        last["content"][1],
        json!({"type":"text","text":"The screenshot."})
    );
    assert_eq!(
        last["content"][2],
        // The newest block carries Claude's rolling cache mark.
        json!({"type":"image","source":{"type":"base64","media_type":"image/jpeg","data":data},"cache_control":{"type":"ephemeral"}})
    );

    // Gemini: inline data in a user turn after the function response.
    let body = request_body("gemini-4-argon-contributor", true, &messages).unwrap();
    let contents = body["contents"].as_array().unwrap();
    let last = contents.last().unwrap();
    assert_eq!(
        *last,
        json!({"role":"user","parts":[
            {"text":"The screenshot."},
            {"inlineData":{"mimeType":"image/jpeg","data":data}}
        ]})
    );
    assert!(contents[contents.len() - 2]["parts"][0]["functionResponse"].is_object());
}

#[test]
fn a_model_that_cannot_see_images_is_never_sent_one() {
    let messages = screenshot_history(b"jpeg");
    for model in [
        "mimo-v2.6-flash",
        "muse-spark-1.3-contributor",
        "claude-haiku-4-5",
        "gemini-4-argon-contributor",
    ] {
        let error = request_body(model, false, &messages).unwrap_err();
        assert!(
            error.to_string().contains("cannot read images"),
            "{model}: {error}"
        );
    }
    // Without images the same model's request is unchanged.
    let body = request_body("mimo-v2.6-flash", false, &messages[..4]).unwrap();
    let wire = body["messages"].as_array().unwrap();
    assert_eq!(wire.len(), 4);
    assert!(wire
        .iter()
        .all(|m| m["content"].is_string() || m["content"].is_null()));
    // PDFs are still refused everywhere.
    let mut pdf = messages.clone();
    pdf[4].message_type = MessageType::Pdf(b"%PDF".to_vec());
    assert!(request_body("mimo-v2.6-flash", true, &pdf).is_err());
}
