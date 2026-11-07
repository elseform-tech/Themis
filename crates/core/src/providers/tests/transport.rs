//! Transport, usage recording, summary-model routing and each protocol's request shape.

use super::*;

#[tokio::test]
async fn a_slow_steady_stream_outlasts_the_read_timeout_but_silence_does_not() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    // A local server: headers, then one chunk every 300 ms for 2.4 s, or nothing at all.
    async fn serve(chunks: usize) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 4096];
            let _ = socket.read(&mut request).await;
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n")
                .await
                .unwrap();
            if chunks == 0 {
                tokio::time::sleep(Duration::from_secs(5)).await;
                return;
            }
            for _ in 0..chunks {
                tokio::time::sleep(Duration::from_millis(300)).await;
                socket.write_all(b"4\r\ndata\r\n").await.unwrap();
            }
            socket.write_all(b"0\r\n\r\n").await.unwrap();
        });
        format!("http://{address}")
    }
    let client = http_client_with(Duration::from_secs(1));
    let started = std::time::Instant::now();
    let body = client
        .get(serve(8).await)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(body.len(), 32);
    assert!(
        started.elapsed() > Duration::from_secs(2),
        "longer than the read timeout"
    );
    let silent = client
        .get(serve(0).await)
        .send()
        .await
        .unwrap()
        .text()
        .await;
    assert!(silent.unwrap_err().is_timeout());
}

#[tokio::test]
async fn http_errors_preserve_retry_after_headers() {
    let server = MockServer::builder().start().await;
    let client = reqwest::Client::new();
    for (index, (status, header, expected)) in [
        (429, Some("45"), Some(45)),
        (503, Some("45"), Some(45)),
        (429, Some("Thu, 01 Jan 2099 00:00:00 GMT"), None),
        (429, Some("invalid"), None),
        (503, Some("Thu, 01 Jan 1970 00:00:00 GMT"), None),
        (429, None, None),
    ]
    .into_iter()
    .enumerate()
    {
        let mut response = ResponseTemplate::new(status);
        if let Some(value) = header {
            response = response.insert_header("Retry-After", value);
        }
        let endpoint = format!("/{index}");
        Mock::given(method("GET"))
            .and(path(&endpoint))
            .respond_with(response)
            .mount(&server)
            .await;
        let response = client
            .get(format!("{}{endpoint}", server.uri()))
            .send()
            .await
            .unwrap();
        let error = ensure_success(response, "test").await.unwrap_err();
        let delay = match error {
            LLMError::RateLimitError { retry_after, .. }
            | LLMError::HttpStatusError { retry_after, .. } => retry_after,
            error => panic!("unexpected error: {error}"),
        };
        if index == 2 {
            assert!(delay.is_some_and(|delay| delay.as_secs() > 60));
        } else {
            assert_eq!(delay.map(|delay| delay.as_secs()), expected, "{header:?}");
        }
    }
}

#[tokio::test]
async fn transport_failures_keep_native_retry_classification() {
    let server = MockServer::builder().start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(1)))
        .mount(&server)
        .await;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(20))
        .build()
        .unwrap();
    let timeout = client.get(server.uri()).send().await.unwrap_err();
    assert!(
        transport_error("chat request failed", timeout).is_retryable(),
        "native timeout must reach the summarization retry predicate"
    );
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let connect = client
        .get(format!("http://{address}"))
        .send()
        .await
        .unwrap_err();
    assert!(
        transport_error("chat request failed", connect).is_retryable(),
        "native connection failure must remain retryable"
    );
    let malformed = reqwest::Client::new()
        .get("://invalid")
        .send()
        .await
        .unwrap_err();
    assert!(
        !transport_error("chat request failed", malformed).is_retryable(),
        "invalid URLs are not transient failures"
    );
}

#[tokio::test]
async fn responses_reject_incomplete_streams() {
    use futures_util::StreamExt;
    for ending in ["response.failed", "response.incomplete", "response.created"] {
        let server = MockServer::builder().start().await;
        Mock::given(method("POST"))
            .and(path("/responses"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                format!("data: {{\"type\":\"{ending}\"}}\n\n"),
                "text/event-stream",
            ))
            .mount(&server)
            .await;
        let provider = CompatibleProvider::go(
            "synthetic".into(),
            "muse-spark-1.3-contributor".into(),
            Some(server.uri()),
            Some("thread-123".into()),
        );
        let mut stream = provider
            .chat_stream_with_tools(&[user_message("hi")], None, None)
            .await
            .unwrap();
        assert!(stream.next().await.unwrap().is_err(), "{ending}");
        assert_eq!(
            server.received_requests().await.unwrap()[0].headers[GO_SESSION_HEADER],
            "thread-123"
        );
    }
}

#[tokio::test]
async fn completed_requests_reach_the_usage_recorder_at_any_metrics_level() {
    use futures_util::StreamExt;
    let server = MockServer::builder().start().await;
    let events=concat!(
        "data: {\"choices\":[{\"delta\":{\"content\":\"PRIVATE-CONTENT-911\"},\"finish_reason\":\"stop\"}]}\n\n",
        "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":100,\"completion_tokens\":20,\"prompt_tokens_details\":{\"cached_tokens\":80},\"completion_tokens_details\":{\"reasoning_tokens\":12}}}\n\n",
        "data: [DONE]\n\n");
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(events, "text/event-stream"))
        .mount(&server)
        .await;
    let provider = CompatibleProvider::go(
        "synthetic".into(),
        "test-model".into(),
        Some(server.uri()),
        None,
    );
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = std::sync::Arc::clone(&seen);
    let recorder: crate::context_metrics::UsageRecorder =
        std::sync::Arc::new(move |record| sink.lock().unwrap().push(record));
    crate::context_metrics::with_usage_recorder(
        recorder,
        crate::context_metrics::with_level(crate::configuration::ContextMetricsLevel::Off, async {
            let chunks = provider
                .chat_stream_with_tools(&[user_message("PRIVATE-PROMPT-911")], None, None)
                .await
                .unwrap()
                .collect::<Vec<_>>()
                .await;
            assert!(chunks.iter().all(Result::is_ok));
            crate::context_metrics::with_usage_kind(
                crate::context_metrics::UsageKind::Summary,
                async {
                    // A request that never completes is not recorded.
                    let _ = provider.chat(&[user_message("x")], None).await;
                },
            )
            .await;
        }),
    )
    .await;
    let seen = seen.lock().unwrap();
    assert_eq!(
        seen[0],
        crate::context_metrics::UsageRecord {
            provider: "opencode-go".into(),
            model: "test-model".into(),
            kind: crate::context_metrics::UsageKind::Answer,
            input_tokens: Some(100),
            cached_input_tokens: Some(80),
            output_tokens: Some(20),
            reasoning_tokens: Some(12),
        }
    );
    assert!(seen
        .iter()
        .skip(1)
        .all(|record| record.kind == crate::context_metrics::UsageKind::Summary));
}

#[test]
fn catalog_prices_are_read_leniently_and_estimate_with_cache_and_tiers() {
    let catalog: CatalogRoot = serde_json::from_value(json!({
        "opencode-go": {"models": {
            "priced": {"cost": {"input": 2, "output": 10, "cache_read": 0.2,
                "tiers": [{"input": 4, "output": 20, "cache_read": 0.4, "tier": {"type": "context", "size": 200000}}]}},
            "free": {"cost": {"input": 0, "output": 0, "cache_read": 0}},
            "broken": {"cost": {"input": "two", "output": -1}},
            "unpriced": {}
        }}
    })).unwrap();
    let models = join_go_catalog(
        ["priced", "free", "broken", "unpriced"]
            .map(String::from)
            .to_vec(),
        catalog,
    );
    let priced = models[0].price.clone().unwrap();
    assert!(!priced.is_free());
    // 100k input of which 50k cached, 10k output: 0.05×2 + 0.05×0.2 + 0.01×10.
    assert!((priced.request_cost(100_000, 50_000, 10_000) - 0.21).abs() < 1e-9);
    // Under the tier size the base price applies; above it the tier's.
    assert!((priced.request_cost(100_000, 0, 0) - 0.2).abs() < 1e-9);
    assert!((priced.request_cost(300_000, 0, 0) - 1.2).abs() < 1e-9);
    assert!(models[1].price.as_ref().unwrap().is_free());
    assert_eq!(models[2].price, None);
    assert_eq!(models[3].price, None);
}

#[tokio::test]
async fn streaming_usage_after_finish_is_captured_without_retaining_content() {
    use futures_util::StreamExt;
    let server = MockServer::builder().start().await;
    let events=concat!(
        "data: {\"choices\":[{\"delta\":{\"content\":\"PRIVATE-CONTENT-173\"},\"finish_reason\":\"stop\"}]}\n\n",
        "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":100,\"completion_tokens\":20,\"prompt_tokens_details\":{\"cached_tokens\":80},\"completion_tokens_details\":{\"reasoning_tokens\":12}}}\n\n",
        "data: [DONE]\n\n");
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(events, "text/event-stream"))
        .mount(&server)
        .await;
    let provider = CompatibleProvider::go(
        "synthetic".into(),
        "test-model".into(),
        Some(server.uri()),
        None,
    );
    crate::context_metrics::collect(async {
        let chunks = provider
            .chat_stream_with_tools(&[user_message("PRIVATE-PROMPT-337")], None, None)
            .await
            .unwrap()
            .collect::<Vec<_>>()
            .await;
        assert!(chunks.iter().all(Result::is_ok));
        let usage = crate::context_metrics::snapshot().unwrap();
        assert_eq!(usage["provider_input_tokens"], 100);
        assert_eq!(usage["provider_output_tokens"], 20);
        assert_eq!(usage["requests_missing_input_usage"], 0);
        assert!(!usage.to_string().contains("PRIVATE-"));
    })
    .await;
    assert_eq!(
        server.received_requests().await.unwrap()[0]
            .body_json::<Value>()
            .unwrap()["stream_options"]["include_usage"],
        true
    );
}

#[tokio::test]
async fn sampling_output_limit_reaches_each_protocol_without_changing_defaults() {
    use autoagents::llm::chat::SamplingOverrides;
    for (model, endpoint, key, response, default_limit) in [
        (
            "mimo-v2.6-flash",
            "/chat/completions",
            "max_tokens",
            json!({"choices":[{"message":{"content":"State"},"finish_reason":"stop"}]}),
            None,
        ),
        (
            "muse-spark-1.3-contributor",
            "/responses",
            "max_output_tokens",
            json!({"output":[{"type":"message","content":[{"type":"output_text","text":"State"}]}]}),
            None,
        ),
        (
            "claude-haiku-4-5",
            "/messages",
            "max_tokens",
            json!({"content":[{"type":"text","text":"State"}],"stop_reason":"end_turn"}),
            Some(16384),
        ),
    ] {
        let server = MockServer::builder().start().await;
        Mock::given(method("POST"))
            .and(path(endpoint))
            .respond_with(ResponseTemplate::new(200).set_body_json(response))
            .expect(2)
            .mount(&server)
            .await;
        let provider: Arc<dyn LLMProvider> = Arc::new(CompatibleProvider::go(
            "synthetic".into(),
            model.into(),
            Some(server.uri()),
            Some("summary-test".into()),
        ));
        let messages = [user_message("Summarize")];
        provider
            .chat_and_sampling(
                &messages,
                None,
                Some(&SamplingOverrides::with_max_tokens(4096)),
            )
            .await
            .unwrap();
        provider.chat(&messages, None).await.unwrap();
        let requests = server.received_requests().await.unwrap();
        let bounded: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
        let ordinary: serde_json::Value = serde_json::from_slice(&requests[1].body).unwrap();
        assert_eq!(bounded[key], 4096, "{model}");
        assert_eq!(ordinary[key].as_u64(), default_limit, "{model}");
        assert_eq!(requests[0].headers[GO_SESSION_HEADER], "summary-test");
    }
}

#[tokio::test]
async fn configured_summary_model_routes_without_changing_ordinary_model() {
    let server = MockServer::builder().start().await;
    Mock::given(method("POST"))
        .respond_with(|request: &wiremock::Request| {
            let response = if request.url.path() == "/responses" {
                json!({"output":[{"type":"message","content":[{"type":"output_text","text":"checkpoint"}]}]})
            } else {
                json!({"choices":[{"message":{"content":"answer"},"finish_reason":"stop"}]})
            };
            ResponseTemplate::new(200).set_body_json(response)
        }).mount(&server).await;
    let catalog = vec![
        GoModel {
            id: "gpt-6-luna".into(),
            effort_levels: vec!["none".into()],
            context_tokens: Some(100000),
            output_tokens: Some(1000),
            thinking_toggle: false,
            ..Default::default()
        },
        GoModel {
            id: "longcat-2.5-preview-free".into(),
            effort_levels: vec![],
            context_tokens: Some(200000),
            output_tokens: Some(8192),
            thinking_toggle: false,
            ..Default::default()
        },
    ];
    let provider = CompatibleProvider::go(
        "synthetic".into(),
        "step-5-preview-free".into(),
        Some(server.uri()),
        Some("override-run".into()),
    );
    let config = serde_json::from_value::<crate::configuration::CompactionConfig>(
        json!({"summary_model":"gpt-6-luna"}),
    )
    .unwrap();
    let messages = [user_message("Summarize")];
    with_catalog_limits(
        &catalog,
        crate::runtime::with_configuration(config, async {
            assert_eq!(summary_context_limit("step-5-preview-free"), Some(100000));
            COMPACTION_REQUEST
                .scope(
                    (),
                    provider.chat_and_sampling(
                        &messages,
                        None,
                        Some(&SamplingOverrides::with_max_tokens(1654)),
                    ),
                )
                .await
                .unwrap();
            provider.chat(&messages, None).await.unwrap();
        }),
    )
    .await;
    COMPACTION_REQUEST
        .scope((), provider.chat(&messages, None))
        .await
        .unwrap();
    let requests = server.received_requests().await.unwrap();
    let bodies: Vec<serde_json::Value> = requests
        .iter()
        .map(|r| serde_json::from_slice(&r.body).unwrap())
        .collect();
    assert_eq!(requests[0].url.path(), "/responses");
    assert_eq!(bodies[0]["model"], "gpt-6-luna");
    assert_eq!(bodies[0]["reasoning"]["effort"], "none");
    assert_eq!(bodies[0]["max_output_tokens"], 1000);
    assert_eq!(bodies[1]["model"], "step-5-preview-free");
    assert_eq!(bodies[2]["model"], "longcat-2.5-preview-free");
    assert_eq!(bodies[2]["thinking"], json!({"type":"disabled"}));
    assert_eq!(requests[0].headers[GO_SESSION_HEADER], "override-run");
}

#[tokio::test]
async fn summary_model_overrides_are_isolated_between_concurrent_runs() {
    let server = MockServer::builder().start().await;
    Mock::given(method("POST")).respond_with(|request:&wiremock::Request| {
        let response=if request.url.path()=="/responses" {json!({"output":[{"type":"message","content":[{"type":"output_text","text":"summary"}]}]})} else {json!({"choices":[{"message":{"content":"answer"},"finish_reason":"stop"}]})};
        ResponseTemplate::new(200).set_body_json(response)
    }).mount(&server).await;
    let barrier = Arc::new(tokio::sync::Barrier::new(2));
    let issue = |model: &'static str, session: &'static str| {
        let barrier = barrier.clone();
        let provider = CompatibleProvider::go(
            "synthetic".into(),
            "step-5-preview-free".into(),
            Some(server.uri()),
            Some(session.into()),
        );
        async move {
            let config = serde_json::from_value(json!({"summary_model":model})).unwrap();
            crate::runtime::with_configuration(config, async {
                barrier.wait().await;
                COMPACTION_REQUEST
                    .scope((), provider.chat(&[user_message("summarize")], None))
                    .await
                    .unwrap();
                provider
                    .chat(&[user_message("continue")], None)
                    .await
                    .unwrap();
            })
            .await;
        }
    };
    tokio::join!(
        issue("gpt-6-luna", "luna-run"),
        issue("longcat-2.5-preview-free", "longcat-run")
    );
    let requests = server.received_requests().await.unwrap();
    for (session, expected) in [
        ("luna-run", "gpt-6-luna"),
        ("longcat-run", "longcat-2.5-preview-free"),
    ] {
        let bodies: Vec<serde_json::Value> = requests
            .iter()
            .filter(|r| r.headers[GO_SESSION_HEADER] == session)
            .map(|r| serde_json::from_slice(&r.body).unwrap())
            .collect();
        assert_eq!(bodies.len(), 2);
        assert_eq!(bodies[0]["model"], expected);
        assert_eq!(bodies[1]["model"], "step-5-preview-free");
    }
}

#[tokio::test]
async fn compaction_disables_reasoning_or_uses_nonthinking_fallback() {
    for model in [
        "longcat-2.5-preview-free",
        "gpt-5.6-luna",
        "gpt-6-luna",
        "mimo-v2.6-flash",
        "muse-spark-1.3-contributor",
        "grok-4.7",
        "qwen3.8-max",
        "minimax-m3",
        "claude-haiku-5-5",
        "unknown-model",
    ] {
        let server = MockServer::builder().start().await;
        Mock::given(method("POST"))
            .respond_with(|request: &wiremock::Request| {
                let response = match request.url.path() {
                    "/responses" => json!({"output":[{"type":"message","content":[{"type":"output_text","text":"checkpoint"}]}]}),
                    "/messages" => json!({"content":[{"type":"text","text":"checkpoint"}],"stop_reason":"end_turn"}),
                    _ => json!({"choices":[{"message":{"content":"checkpoint"},"finish_reason":"stop"}],
                        "usage":{"prompt_tokens":100,"completion_tokens":5,"total_tokens":105,"completion_tokens_details":{"reasoning_tokens":0}}}),
                };
                ResponseTemplate::new(200).set_body_json(response)
            })
            .mount(&server).await;
        let mut provider = CompatibleProvider::go(
            "synthetic".into(),
            model.into(),
            Some(server.uri()),
            Some("thread-session".into()),
        );
        provider.reasoning_effort = Some("high".into());
        let messages = [user_message("summarize")];
        let sampling = SamplingOverrides::with_max_tokens(1654);
        let dir = tempfile::tempdir().unwrap();
        let log = Arc::new(crate::diagnostics::DiagnosticLog::new(dir.path().into()));
        crate::diagnostics::ACTIVE
            .scope(
                crate::diagnostics::DiagnosticContext {
                    log: Arc::clone(&log),
                    thread_id: "thread".into(),
                    run_id: "run".into(),
                    level: "info".into(),
                },
                COMPACTION_REQUEST.scope(
                    (),
                    provider.chat_and_sampling(&messages, None, Some(&sampling)),
                ),
            )
            .await
            .unwrap();
        provider
            .chat_and_sampling(&messages, None, None)
            .await
            .unwrap();
        let requests = server.received_requests().await.unwrap();
        let summary: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
        let answer: serde_json::Value = serde_json::from_slice(&requests[1].body).unwrap();
        if model.starts_with("gpt-") {
            assert_eq!(summary["model"], model);
            assert_eq!(summary["reasoning"]["effort"], "none");
            assert_eq!(summary["max_output_tokens"], 1654);
        } else {
            assert_eq!(summary["model"], "longcat-2.5-preview-free");
            assert_eq!(summary["thinking"], json!({"type":"disabled"}));
            assert!(summary.get("reasoning_effort").is_none());
            assert_eq!(summary["max_tokens"], 1654);
        }
        assert_eq!(answer["model"], model);
        assert!(answer.get("thinking").is_none());
        if provider.uses_responses() {
            assert_eq!(answer["reasoning"]["effort"], "high");
        } else if !provider.uses_messages() {
            assert_eq!(answer["reasoning_effort"], "high");
        }
        assert_eq!(requests[0].headers[GO_SESSION_HEADER], "thread-session");
        let records = log
            .query(&crate::diagnostics::DiagnosticFilter::default())
            .unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].event, "summary_usage");
        assert_eq!(records[0].details["model"], summary["model"]);
        if !model.starts_with("gpt-") {
            assert_eq!(records[0].details["reasoning_tokens"], 0);
        }
    }
}

#[tokio::test]
async fn truncated_output_diagnostics_keep_only_usage_and_size_metadata() {
    let server = MockServer::builder().start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices":[{"message":{"content":null,"reasoning_content":"private reasoning sentinel"},"finish_reason":"length"}],
            "usage":{"prompt_tokens":100,"completion_tokens":1654,"total_tokens":1754,
                "completion_tokens_details":{"reasoning_tokens":1654}}
        })))
        .mount(&server).await;
    let dir = tempfile::tempdir().unwrap();
    let log = Arc::new(crate::diagnostics::DiagnosticLog::new(dir.path().into()));
    let provider = CompatibleProvider::go(
        "synthetic".into(),
        "mimo-v2.6-flash".into(),
        Some(server.uri()),
        None,
    );
    crate::diagnostics::ACTIVE
        .scope(
            crate::diagnostics::DiagnosticContext {
                log: Arc::clone(&log),
                thread_id: "thread".into(),
                run_id: "run".into(),
                level: "info".into(),
            },
            async {
                assert!(provider
                    .chat_and_sampling(
                        &[user_message("private prompt sentinel")],
                        None,
                        Some(&SamplingOverrides::with_max_tokens(1654))
                    )
                    .await
                    .is_err());
            },
        )
        .await;
    let records = log
        .query(&crate::diagnostics::DiagnosticFilter::default())
        .unwrap();
    let record = records
        .iter()
        .find(|record| record.event == "output_truncated")
        .expect("truncation must retain diagnostic metadata");
    assert_eq!(record.details["completion_tokens"], 1654);
    assert_eq!(record.details["reasoning_tokens"], 1654);
    assert_eq!(record.details["public_text_chars"], 0);
    assert_eq!(record.details["output_limit_tokens"], 1654);
    assert!(!serde_json::to_string(&records)
        .unwrap()
        .contains("sentinel"));
}

#[tokio::test]
async fn providers_identify_output_truncation_without_exposing_response_content() {
    for (model, response) in [
        (
            "mimo-v2.6-flash",
            json!({"choices":[{"message":{"content":"Partial private state"},"finish_reason":"length"}]}),
        ),
        (
            "muse-spark-1.3-contributor",
            json!({"status":"incomplete","incomplete_details":{"reason":"max_output_tokens"},"output":[{"type":"message","content":[{"type":"output_text","text":"Partial private state"}]}]}),
        ),
    ] {
        let server = MockServer::builder().start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(response))
            .mount(&server)
            .await;
        let provider =
            CompatibleProvider::go("synthetic".into(), model.into(), Some(server.uri()), None);
        let result = provider.chat(&[user_message("Summarize")], None).await;
        let error = match result {
            Err(error) => error,
            Ok(_) => panic!("partial checkpoint must be rejected"),
        };
        assert!(
            matches!(&error, LLMError::Generic(message) if message == "Provider response was truncated"),
            "{error}"
        );
        assert!(!error.to_string().contains("private state"));
        // stream=true may still return JSON; it must enforce the same stop reason.
        let error = provider
            .chat_stream_with_tools(&[user_message("Summarize")], None, None)
            .await
            .err()
            .expect("JSON fallback must reject truncated output before publishing it");
        assert!(error.to_string().contains("truncated"));
        assert!(!error.to_string().contains("private state"));
    }
}

#[test]
fn responses_keep_optional_read_arguments_optional() {
    let root = tempfile::tempdir().unwrap();
    let tools =
        crate::tools::boxed_tools(root.path(), Arc::new(crate::tools::AllowAllHook)).unwrap();
    let reader = tools
        .iter()
        .find(|tool| tool.name() == "read_file")
        .unwrap();
    let mut tool = test_tool();
    tool.function.parameters = reader.args_schema();
    assert_eq!(tool.function.parameters["required"], json!(["file_path"]));
    for model in ["gpt-6-luna", "muse-spark-1.3-contributor"] {
        let provider = CompatibleProvider::go("synthetic".into(), model.into(), None, None);
        for stream in [false, true] {
            let request = provider
                .chat_request(
                    &[user_message("Read source.py")],
                    Some(std::slice::from_ref(&tool)),
                    stream,
                    None,
                )
                .unwrap()
                .build()
                .unwrap();
            let body: Value =
                serde_json::from_slice(request.body().unwrap().as_bytes().unwrap()).unwrap();
            assert_eq!(
                body["tools"][0]["strict"], false,
                "optional selectors must remain omittable in Responses"
            );
            assert_eq!(
                body["tools"][0]["parameters"]["required"],
                json!(["file_path"])
            );
            assert!(body["tools"][0]["parameters"]["properties"]["jsonl_record"].is_object());
        }
    }
}

#[tokio::test]
async fn muse_uses_responses_for_text_and_tool_continuations() {
    let server = MockServer::builder().start().await;
    Mock::given(method("POST"))
        .and(path("/zen/go/v1/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "output": [
                {"type": "message", "content": [{"type": "output_text", "text": "Reading."}]},
                {"type": "function_call", "call_id": "call_1", "name": "read_file", "arguments": "{}"}
            ]
        })))
        .expect(2)
        .mount(&server).await;
    let mut config = ProviderConfig::new(ProviderKind::Go, "synthetic-key")
        .with_model("muse-spark-1.3-contributor")
        .with_base_url(format!("{}/zen/go/v1", server.uri()));
    config.session_id = Some("conversation-123".into());
    let provider = resolve(&config).await.unwrap();
    let response = provider
        .chat_with_tools(&[user_message("read it")], Some(&[test_tool()]), None)
        .await
        .unwrap();
    assert_eq!(response.text().as_deref(), Some("Reading."));
    let calls = response.tool_calls().unwrap();
    assert_eq!(calls[0].function.name, "read_file");
    let mut result = calls[0].clone();
    result.function.arguments = "file contents".into();
    provider
        .chat(
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
            None,
        )
        .await
        .unwrap();
    let requests = server.received_requests().await.unwrap();
    assert!(requests
        .iter()
        .all(|request| request.headers.contains_key(GO_SESSION_HEADER)));
    assert_eq!(requests[0].headers[GO_SESSION_HEADER], "conversation-123");
    assert_eq!(requests[1].headers[GO_SESSION_HEADER], "conversation-123");
    let first: Value = requests[0].body_json().unwrap();
    assert_eq!(first["model"], "muse-spark-1.3-contributor");
    assert_eq!(first["input"][0]["content"], "read it");
    assert_eq!(first["tools"][0]["name"], "read_file");
    assert_eq!(first["store"], false);
    let second: Value = requests[1].body_json().unwrap();
    assert_eq!(second["input"][1]["type"], "function_call");
    assert_eq!(second["input"][2]["type"], "function_call_output");
    assert_eq!(second["input"][2]["call_id"], "call_1");
    assert_eq!(second["input"][2]["output"], "file contents");
}

#[tokio::test]
async fn muse_streams_responses_text_and_tools() {
    use futures_util::StreamExt;
    let server = MockServer::builder().start().await;
    let events = [
        json!({"type": "response.output_text.delta", "delta": "Reading."}),
        json!({"type": "response.output_item.added", "output_index": 0, "item": {"type": "function_call", "id": "fc_1", "call_id": "call_1", "name": "read_file", "arguments": ""}}),
        json!({"type": "response.function_call_arguments.delta", "output_index": 0, "delta": "{"}),
        json!({"type": "response.function_call_arguments.delta", "output_index": 0, "delta": "}"}),
        json!({"type": "response.function_call_arguments.done", "output_index": 0, "arguments": "{}"}),
        json!({"type": "response.completed", "response": {}}),
    ].iter().map(|event| format!("data: {event}\n\n")).collect::<String>();
    Mock::given(method("POST"))
        .and(path("/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(events, "text/event-stream"))
        .expect(1)
        .mount(&server)
        .await;
    let provider = resolve(
        &ProviderConfig::new(ProviderKind::Go, "synthetic-key")
            .with_model("muse-spark-1.2-contributor")
            .with_base_url(server.uri()),
    )
    .await
    .unwrap();
    let mut stream = provider
        .chat_stream_with_tools(&[user_message("read it")], Some(&[test_tool()]), None)
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
    assert!(server.received_requests().await.unwrap()[0]
        .headers
        .contains_key(GO_SESSION_HEADER));
    assert_eq!(text, "Reading.");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].function.name, "read_file");
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests[0].body_json::<Value>().unwrap()["stream"], true);
}

#[tokio::test]
async fn qwen_uses_messages_for_tools_and_streaming() {
    use futures_util::StreamExt;
    let server = MockServer::builder().start().await;
    Mock::given(method("POST")).and(path("/zen/go/v1/messages"))
        .and(wiremock::matchers::body_partial_json(json!({"stream": false})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "content": [{"type": "text", "text": "Reading."}, {"type": "tool_use", "id": "call_1", "name": "read_file", "input": {"path": "a"}}],
            "stop_reason": "tool_use"
        }))).expect(2).mount(&server).await;
    let events = [
        json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}),
        json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "Done."}}),
        json!({"type": "content_block_start", "index": 1, "content_block": {"type": "tool_use", "id": "call_2", "name": "read_file", "input": {}}}),
        json!({"type": "content_block_delta", "index": 1, "delta": {"type": "input_json_delta", "partial_json": "{}"}}),
        json!({"type": "content_block_start", "index": 2, "content_block": {"type": "tool_use", "id": "call_3", "name": "read_file", "input": {}}}),
        json!({"type": "message_delta", "delta": {"stop_reason": "tool_use"}}),
        json!({"type": "message_stop"}),
    ].iter().map(|event| format!("data: {event}\n\n")).collect::<String>();
    Mock::given(method("POST"))
        .and(path("/zen/go/v1/messages"))
        .and(wiremock::matchers::body_partial_json(
            json!({"stream": true}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_raw(events, "text/event-stream"))
        .expect(1)
        .mount(&server)
        .await;
    let provider = resolve(
        &ProviderConfig::new(ProviderKind::Go, "synthetic-key")
            .with_model("qwen3.7-max")
            .with_base_url(format!("{}/zen/go/v1", server.uri())),
    )
    .await
    .unwrap();
    let tools = [test_tool()];
    let messages = [
        ChatMessage {
            role: ChatRole::System,
            message_type: MessageType::Text,
            content: "Be helpful.".into(),
        },
        user_message("read it"),
    ];
    let response = provider
        .chat_with_tools(&messages, Some(&tools), None)
        .await
        .unwrap();
    assert_eq!(response.text().as_deref(), Some("Reading."));
    let calls = response.tool_calls().unwrap();
    assert_eq!(calls[0].function.arguments, r#"{"path":"a"}"#);
    let mut result = calls[0].clone();
    result.function.arguments = "contents".into();
    provider
        .chat(
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
            None,
        )
        .await
        .unwrap();
    let mut stream = provider
        .chat_stream_with_tools(&messages, Some(&tools), None)
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
    assert_eq!(text, "Done.");
    assert_eq!(calls[0].id, "call_2");
    assert_eq!(calls[1].function.arguments, "{}");
    assert_eq!(calls[0].function.arguments, "{}");
    let requests = server.received_requests().await.unwrap();
    assert!(requests
        .iter()
        .all(|request| request.headers.contains_key(GO_SESSION_HEADER)));
    let first: Value = requests[0].body_json().unwrap();
    assert_eq!(first["model"], "qwen3.7-max");
    assert_eq!(first["system"], "Be helpful.");
    assert_eq!(first["tools"][0]["input_schema"]["type"], "object");
    assert!(first["max_tokens"].as_u64().unwrap() > 0);
    assert_eq!(requests[0].headers["anthropic-version"], "2023-06-01");
    assert!(requests[0].headers.contains_key(GO_SESSION_HEADER));
    let second: Value = requests[1].body_json().unwrap();
    assert_eq!(second["messages"][1]["content"][0]["type"], "tool_use");
    assert_eq!(second["messages"][2]["content"][0]["tool_use_id"], "call_1");
    assert_eq!(second["messages"][2]["content"][0]["content"], "contents");
}

#[tokio::test]
async fn messages_reject_truncated_or_interrupted_responses() {
    use futures_util::StreamExt;
    for (body, streaming, expected) in [
        (json!({"content": [{"type": "text", "text": "partial"}], "stop_reason": "max_tokens"}).to_string(), false, "truncated"),
        ("data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"max_tokens\"}}\n\n".into(), true, "truncated"),
        ("data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"partial\"}}\n\n".into(), true, "ended before completion"),
    ] {
        let server = MockServer::builder().start().await;
        Mock::given(method("POST")).and(path("/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(body, if streaming { "text/event-stream" } else { "application/json" }))
            .mount(&server).await;
        let provider = resolve(&ProviderConfig::new(ProviderKind::Go, "synthetic-key").with_model("minimax-m3").with_base_url(server.uri())).await.unwrap();
        if streaming {
            let chunks = provider.chat_stream_with_tools(&[user_message("hello")], None, None).await.unwrap().collect::<Vec<_>>().await;
            assert!(chunks.iter().any(|chunk| chunk.as_ref().err().is_some_and(|error| error.to_string().contains(expected))));
        } else {
            assert!(provider.chat(&[user_message("hello")], None).await.err().unwrap().to_string().contains(expected));
        }
    }
}

// Snapshot of GET /zen/go/v1/models, updated against docs/go on 2026-10-07.
#[tokio::test]
async fn every_go_catalog_model_uses_its_wire_protocol() {
    let server = MockServer::builder().start().await;
    for (path_name, body) in [
        (
            "/responses",
            json!({"output": [{"type": "message", "content": [{"type": "output_text", "text": "ok"}]}]}),
        ),
        (
            "/messages",
            json!({"content": [{"type": "text", "text": "ok"}], "stop_reason": "end_turn"}),
        ),
        ("/chat/completions", chat_ok_body("ok")),
    ] {
        Mock::given(method("POST"))
            .and(path(path_name))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&server)
            .await;
    }
    for (endpoint, models) in [
        (
            "/responses",
            &[
                "gpt-5.6-luna",
                "gpt-6-luna",
                "grok-4.5",
                "grok-4.6",
                "grok-4.7",
                "muse-spark-1.2-contributor",
                "muse-spark-1.3-contributor",
            ][..],
        ),
        (
            "/messages",
            &[
                "claude-haiku-5-5",
                "minimax-m3",
                "minimax-m2.7",
                "minimax-m2.5",
                "qwen3.7-max",
                "qwen3.8-max",
                "qwen3.8-flash",
                "qwen3.7-plus",
                "qwen3.6-plus",
                "qwen3.5-plus",
            ][..],
        ),
        (
            "/chat/completions",
            &[
                "kimi-k3",
                "kimi-k2.7-code",
                "kimi-k2.6",
                "longcat-2.0",
                "kimi-k2.5",
                "glm-5.2",
                "glm-5.3-flash",
                "glm-5.3",
                "glm-5.1",
                "glm-5",
                "deepseek-v4-pro",
                "deepseek-v4-flash",
                "deepseek-flash",
                "deepseek-v4.1-flash",
                "deepseek-v4-flash-vision-exp",
                "mimo-v2-pro",
                "mimo-v2-omni",
                "mimo-v2.6-pro",
                "mimo-v2.6-flash",
                "space-bunny-free",
                "mimo-v2.5-pro",
                "mimo-v2.5",
                "hy4-preview",
                "hy3",
                "hy3-preview",
                "omen-alpha",
            ][..],
        ),
    ] {
        for model in models {
            let provider = resolve(
                &ProviderConfig::new(ProviderKind::Go, "synthetic-key")
                    .with_model(*model)
                    .with_base_url(server.uri()),
            )
            .await
            .unwrap();
            let reply = provider.chat(&[user_message("hello")], None).await.unwrap();
            assert_eq!(reply.text().as_deref(), Some("ok"), "{model}");
            let requests = server.received_requests().await.unwrap();
            let request = requests.last().unwrap();
            assert_eq!(request.url.path(), endpoint, "{model}");
            assert!(
                !request.headers[GO_SESSION_HEADER]
                    .to_str()
                    .unwrap()
                    .is_empty(),
                "{model}"
            );
            assert_eq!(request.body_json::<Value>().unwrap()["model"], *model);
        }
    }
    assert_eq!(server.received_requests().await.unwrap().len(), 43);
}

#[test]
fn kind_from_str_is_case_insensitive() {
    for (input, expected) in [
        ("go", ProviderKind::Go),
        ("Go", ProviderKind::Go),
        ("GO", ProviderKind::Go),
        ("opencode-go", ProviderKind::Go),
    ] {
        assert_eq!(input.parse::<ProviderKind>().unwrap(), expected, "{input}");
    }
}

#[test]
fn kind_from_str_rejects_unknown() {
    for input in ["", "gpt", "azure-openai", "openrouter", "go2"] {
        assert!(input.parse::<ProviderKind>().is_err(), "{input}");
    }
}

#[test]
fn kind_display_roundtrips() {
    assert_eq!(ProviderKind::Go.to_string(), "go");
}
