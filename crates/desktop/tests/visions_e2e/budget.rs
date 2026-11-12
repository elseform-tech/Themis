//! Daily budgets through the real app server: a Vision's spend reaching the cap stops its
//! chats (holding nothing back), refuses its driving calls, posts one notice, leaves the
//! person's own chat alone, and raising the cap lets it drive again.

use themis_core::providers::{GoModel, ModelPrice};

use super::*;

/// A model that bills a million input tokens a request ($1 at the catalog's price below); a
/// newest message with "stall" in it waits on the model instead.
async fn billing_provider() -> MockServer {
    let provider = MockServer::builder().start().await;
    Mock::given(method("POST"))
        .respond_with(|request: &Request| {
            let body: Value = request.body_json().unwrap_or(Value::Null);
            let reply = ResponseTemplate::new(200).set_body_json(json!({
                "choices": [{"message": {"role": "assistant", "content": "done"},
                    "finish_reason": "stop"}],
                "usage": {"prompt_tokens": 1_000_000, "completion_tokens": 1,
                    "total_tokens": 1_000_001}
            }));
            let last = body["messages"]
                .as_array()
                .and_then(|messages| messages.last());
            if last.is_some_and(|message| message["content"].to_string().contains("stall")) {
                return reply.set_delay(Duration::from_secs(30));
            }
            reply
        })
        .mount(&provider)
        .await;
    provider
}

fn logs(fx: &Fixture) -> String {
    std::fs::read_dir(fx.data.join("logs"))
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| std::fs::read_to_string(entry.path()).unwrap_or_default())
        .collect()
}

fn budget_notices(fx: &Fixture, channel: &str) -> Vec<Value> {
    fx.entries(channel)
        .into_iter()
        .filter(|entry| entry["type"] == "notice" && entry["kind"] == "budget_reached")
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_budget_stops_a_visions_chats_and_refuses_its_sends() {
    let _turn = ONE_SERVER.lock().await;
    let fx = fixture().await;
    let provider = billing_provider().await;
    fx.use_provider(&provider).await;
    fx.state
        .set_model_catalog_for_test(vec![GoModel {
            id: "test-model".into(),
            price: Some(ModelPrice {
                input: 1.0,
                output: 0.0,
                ..ModelPrice::default()
            }),
            ..GoModel::default()
        }])
        .await;
    let (vision, channel) = fx.vision_and_channel("Wren", "hq");
    let (_own_chat, agent) = fx.vision_run(&vision, &channel).await;
    fx.set_visions(json!({"enabled": false, "daily_budget_usd": 0.5}));
    let send = |thread: &str, text: &str| {
        fx.agent_call(
            &agent,
            "send_message",
            json!({"threadId": thread, "text": text}),
        )
    };

    // One chat waits on the model; the other's request costs $1, past the $0.50 cap.
    let slow = fx.vision_starts_chat(&agent);
    json_of(&send(&slow, "stall on this"));
    wait_until(|| async { !provider.received_requests().await.unwrap().is_empty() }).await;
    assert!(fx.running(&slow).await);
    let quick = fx.vision_starts_chat(&agent);
    json_of(&send(&quick, "work on this"));

    // The quick run's end stops the slow one, without holding it back.
    wait_until(|| async { !fx.running(&slow).await && !fx.running(&quick).await }).await;
    assert_eq!(owner_hold(&fx.state, &slow), Some(Hold::None));
    assert!(fx.state.visions_over_budget(&vision));
    assert!(fx.state.visions_spent_today(Some(&vision)) >= 1.0);
    assert_eq!(
        budget_notices(&fx, &channel),
        vec![
            json!({"type": "notice", "kind": "budget_reached", "vision_id": vision,
            "unpriced_requests": 0})
        ]
    );

    // Its driving calls are refused, and the log keeps the reason only.
    let refusals = [
        send(&slow, "more work"),
        fx.agent_call(
            &agent,
            "enqueue_message",
            json!({"threadId": slow, "text": "later"}),
        ),
        fx.agent_call(
            &agent,
            "create_thread",
            json!({"projectRoot": fx.project, "provider": "go"}),
        ),
    ];
    for refused in &refusals {
        assert!(failed_with(refused, "today's budget"), "{}", text(refused));
    }
    let log = logs(&fx);
    assert!(log.contains("\"reason\":\"over_budget\""), "{log}");
    assert!(!log.contains("more work") && !log.contains("later\""));

    // The person's own message takes the chat over: it runs for them and is not stopped, and
    // its end posts no second notice.
    fx.call_ok(
        "send_message",
        json!({"threadId": quick, "text": "mine now", "reasoningEffort": null}),
    );
    wait_until(|| async { !fx.running(&quick).await }).await;
    assert_eq!(owner_hold(&fx.state, &quick), Some(Hold::TakenOver));
    assert_eq!(budget_notices(&fx, &channel).len(), 1);

    // Raising the cap lets the Vision drive again.
    fx.set_visions(json!({"enabled": false, "daily_budget_usd": 50}));
    json_of(&send(&slow, "finish it"));
    wait_until(|| async { !fx.running(&slow).await }).await;
    fx.shut_down().await;
}

/// A model that, for a message with "loop", reads a file again and again (20 steps, each
/// request billed $1) before it answers.
async fn looping_provider() -> MockServer {
    let provider = MockServer::builder().start().await;
    Mock::given(method("POST"))
        .respond_with(|request: &Request| {
            let body: Value = request.body_json().unwrap_or(Value::Null);
            let messages = body["messages"].as_array().cloned().unwrap_or_default();
            let last = messages
                .iter()
                .rposition(|m| m["role"] == "user")
                .unwrap_or(0);
            let steps = messages[last..]
                .iter()
                .filter(|m| m["role"] == "tool")
                .count();
            let usage = json!({"prompt_tokens": 1_000_000, "completion_tokens": 1,
                "total_tokens": 1_000_001});
            let looping = messages[last]["content"].to_string().contains("loop");
            if looping && steps < 20 {
                let call = json!({"id": format!("read-{steps}"), "type": "function",
                    "function": {"name": "read_file",
                        "arguments": json!({"file_path": "README.md"}).to_string()}});
                return ResponseTemplate::new(200)
                    .set_delay(Duration::from_millis(100))
                    .set_body_json(json!({"choices": [{"message": {"role": "assistant",
                        "content": null, "tool_calls": [call]}, "finish_reason": "tool_calls"}],
                        "usage": usage}));
            }
            ResponseTemplate::new(200).set_body_json(json!({"choices": [{"message":
                {"role": "assistant", "content": "done"}, "finish_reason": "stop"}],
                "usage": usage}))
        })
        .mount(&provider)
        .await;
    provider
}

/// One long turn of many requests is stopped mid-turn once a request passes the cap, not when
/// the turn ends.
#[tokio::test(flavor = "multi_thread")]
async fn one_long_turn_is_stopped_once_a_request_passes_the_cap() {
    let _turn = ONE_SERVER.lock().await;
    let fx = fixture().await;
    let provider = looping_provider().await;
    fx.use_provider(&provider).await;
    fx.state
        .set_model_catalog_for_test(vec![GoModel {
            id: "test-model".into(),
            price: Some(ModelPrice {
                input: 1.0,
                output: 0.0,
                ..ModelPrice::default()
            }),
            ..GoModel::default()
        }])
        .await;
    let (vision, channel) = fx.vision_and_channel("Wren", "hq");
    let (_own_chat, agent) = fx.vision_run(&vision, &channel).await;
    fx.set_visions(json!({"enabled": false, "daily_budget_usd": 2.5}));
    let work = fx.vision_starts_chat(&agent);
    json_of(&fx.agent_call(
        &agent,
        "send_message",
        json!({"threadId": work, "text": "loop over the readme"}),
    ));
    wait_until(|| async { !provider.received_requests().await.unwrap().is_empty() }).await;
    wait_until(|| async { !fx.running(&work).await }).await;
    let requests = provider.received_requests().await.unwrap().len();
    assert!(
        (3..=6).contains(&requests),
        "stopped soon after the third $1 request, not after 21: {requests}"
    );
    assert_eq!(owner_hold(&fx.state, &work), Some(Hold::None));
    assert_eq!(budget_notices(&fx, &channel).len(), 1);
    fx.shut_down().await;
}
