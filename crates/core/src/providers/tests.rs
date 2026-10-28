use super::*;
use std::sync::Mutex;

use autoagents::llm::chat::FunctionTool;
use autoagents::llm::FunctionCall;
use serde_json::{json, Value};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Serializes tests that touch `THEMIS_*` environment variables (`resolve`
/// reads them, and process env is global across test threads).
static ENV_LOCK: Mutex<()> = Mutex::new(());

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
    let server = MockServer::start().await;
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
    let server = MockServer::start().await;
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

fn user_message(content: &str) -> ChatMessage {
    ChatMessage {
        role: ChatRole::User,
        message_type: MessageType::Text,
        content: content.to_owned(),
    }
}

fn chat_ok_body(text: &str) -> Value {
    json!({
        "id": "chatcmpl-test",
        "object": "chat.completion",
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": text},
            "finish_reason": "stop"
        }],
        "usage": {"prompt_tokens": 3, "completion_tokens": 5, "total_tokens": 8}
    })
}

fn tool_call_body() -> Value {
    json!({
        "choices": [{
            "message": {
                "role": "assistant",
                "content": null,
                "tool_calls": [{
                    "id": "call_1",
                    "type": "function",
                    "function": {
                        "name": "read_file",
                        "arguments": "{\"path\": \"src/main.rs\"}"
                    }
                }]
            }
        }]
    })
}

fn test_tool() -> Tool {
    Tool {
        tool_type: "function".to_owned(),
        function: FunctionTool {
            name: "read_file".to_owned(),
            description: "Reads a file".to_owned(),
            parameters: json!({"type": "object"}),
        },
    }
}

async fn mount_chat(mock: &MockServer, body: Value) {
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(mock)
        .await;
}

#[tokio::test]
async fn responses_reject_incomplete_streams() {
    use futures_util::StreamExt;
    for ending in ["response.failed", "response.incomplete", "response.created"] {
        let server = MockServer::start().await;
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
    let server = MockServer::start().await;
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
                crate::context_metrics::UsageKind::GoalCheck,
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
        .all(|record| record.kind == crate::context_metrics::UsageKind::GoalCheck));
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
    let server = MockServer::start().await;
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
        let server = MockServer::start().await;
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
    let server = MockServer::start().await;
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
    let server = MockServer::start().await;
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
        let server = MockServer::start().await;
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
    let server = MockServer::start().await;
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
        let server = MockServer::start().await;
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
    let server = MockServer::start().await;
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
    let server = MockServer::start().await;
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
    let server = MockServer::start().await;
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
        let server = MockServer::start().await;
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
    let server = MockServer::start().await;
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

#[tokio::test]
async fn streaming_keeps_public_text_and_assembles_tool_arguments() {
    use futures_util::StreamExt;
    let server = MockServer::start().await;
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

#[tokio::test]
async fn chat_reasoning_is_replayed_with_its_own_tool_calls_but_not_into_summaries() {
    use futures_util::StreamExt;
    use wiremock::matchers::body_partial_json;
    let server = MockServer::start().await;
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
    let server = MockServer::start().await;
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

#[tokio::test]
async fn a_streamed_responses_cut_off_is_reported_as_truncated() {
    use futures_util::StreamExt;
    let server = MockServer::start().await;
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
    let server = MockServer::start().await;
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

#[tokio::test]
async fn messages_thinking_blocks_return_first_and_unchanged_with_their_tool_uses() {
    use futures_util::StreamExt;
    let server = MockServer::start().await;
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
    let server = MockServer::start().await;
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
    let server = MockServer::start().await;
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
    let server = MockServer::start().await;
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
    let server = MockServer::start().await;
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
    let server = MockServer::start().await;
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
    let server = MockServer::start().await;
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
    let server = MockServer::start().await;
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
    let server = MockServer::start().await;
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
    let server = MockServer::start().await;
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
    let server = MockServer::start().await;
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
    let server = MockServer::start().await;
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
    let server = MockServer::start().await;
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
    let server = MockServer::start().await;
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
    let server = MockServer::start().await;
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
    let server = MockServer::start().await;
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
    let server = MockServer::start().await;
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
        let server = MockServer::start().await;
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
    let server = MockServer::start().await;
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
        let server = MockServer::start().await;
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
    let server = MockServer::start().await;
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
    let server = MockServer::start().await;
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
    let server = MockServer::start().await;
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
    let go = MockServer::start().await;
    mount_chat(&go, chat_ok_body("answer")).await;
    let local = MockServer::start().await;
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
    let server = MockServer::start().await;
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
