//! Requests between channels through the real app server: one chief asks another, which
//! answers through its own credential; both channels show the exchange.

use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn a_request_goes_out_and_its_answer_comes_back() {
    let _turn = ONE_SERVER.lock().await;
    let fx = fixture().await;
    let (juno, release) = fx.chief_and_channel("Juno", "release");
    // At Always ask the request waits for an answer (above it, it is taken on at once).
    let (tess, api) = fx.chief_and_channel_in("Tess", "api", "custom");
    let (_, asker) = fx.vision_run(&juno, &release).await;
    let (_, helper) = fx.vision_run(&tess, &api).await;
    let asked = fx.agent_call_ok(
        &asker,
        "request_channel",
        json!({"channelId": api, "ask": "fix the export date"}),
    );
    let id = asked["requestId"].as_str().unwrap().to_owned();
    assert!(fx
        .entries(&api)
        .iter()
        .any(|e| e["type"] == "request_received" && e["request_id"] == id.as_str()));
    // Only the receiving chief answers; the asker cannot answer its own request.
    let own = fx.agent_call(
        &asker,
        "answer_request",
        json!({"requestId": id, "accept": false}),
    );
    assert!(!own.status.success());
    fx.agent_call_ok(
        &helper,
        "answer_request",
        json!({"requestId": id, "accept": false, "note": "busy"}),
    );
    for channel in [&release, &api] {
        let answered: Vec<Value> = fx
            .entries(channel)
            .into_iter()
            .filter(|e| e["type"] == "request_answered")
            .collect();
        assert_eq!(answered.len(), 1, "{answered:?}");
    }
    // A worker cannot send requests.
    let (rook, work) = fx.vision_and_channel("Rook", "work");
    let (_, worker) = fx.vision_run(&rook, &work).await;
    assert!(!fx
        .agent_call(
            &worker,
            "request_channel",
            json!({"channelId": api, "ask": "x"})
        )
        .status
        .success());
    fx.shut_down().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_paused_chief_may_still_decline_but_not_accept() {
    let _turn = ONE_SERVER.lock().await;
    let fx = fixture().await;
    let (juno, release) = fx.chief_and_channel("Juno", "release");
    // At Always ask a request waits for an answer.
    let (tess, api) = fx.chief_and_channel_in("Tess", "api", "custom");
    let (_, asker) = fx.vision_run(&juno, &release).await;
    let (_, helper) = fx.vision_run(&tess, &api).await;
    let asked = fx.agent_call_ok(
        &asker,
        "request_channel",
        json!({"channelId": api, "ask": "one"}),
    );
    let first = asked["requestId"].as_str().unwrap().to_owned();
    fx.call_ok("pause_vision", json!({"visionId": tess, "paused": true}));
    let accepted = fx.agent_call(
        &helper,
        "answer_request",
        json!({"requestId": first, "accept": true}),
    );
    assert!(!accepted.status.success());
    fx.agent_call_ok(
        &helper,
        "answer_request",
        json!({"requestId": first, "accept": false}),
    );
    fx.shut_down().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_chief_asks_another_channel_by_its_name() {
    let _turn = ONE_SERVER.lock().await;
    let fx = fixture().await;
    let (juno, release) = fx.chief_and_channel("Juno", "release");
    let (_, api) = fx.chief_and_channel("Tess", "api");
    let (_, asker) = fx.vision_run(&juno, &release).await;
    fx.agent_call_ok(
        &asker,
        "request_channel",
        json!({"channel": "#api", "ask": "fix the export date"}),
    );
    assert!(fx
        .entries(&api)
        .iter()
        .any(|e| e["type"] == "request_received"));
    assert!(!fx
        .agent_call(
            &asker,
            "request_channel",
            json!({"channel": "nowhere", "ask": "x"})
        )
        .status
        .success());
    fx.shut_down().await;
}

/// The whole exchange through each chief's `themis`: asked, taken on for the receiving channel
/// at once (no person), and its result back in the asking channel; a worker sends no result.
#[tokio::test(flavor = "multi_thread")]
async fn a_result_reaches_the_asking_channel() {
    let _turn = ONE_SERVER.lock().await;
    let fx = fixture().await;
    let (juno, release) = fx.chief_and_channel("Juno", "release");
    let (ada, design) = fx.chief_and_channel("Ada", "design");
    let (_, asker) = fx.vision_run(&juno, &release).await;
    let (_, helper) = fx.vision_run(&ada, &design).await;
    let asked = fx.agent_call_ok(
        &asker,
        "request_channel",
        json!({"channel": "design", "ask": "a layout for the recipe page"}),
    );
    let id = asked["requestId"].as_str().unwrap().to_owned();
    // Taken on when it arrived: both channels say so before any result.
    for channel in [&release, &design] {
        assert!(fx
            .entries(channel)
            .iter()
            .any(|e| e["type"] == "request_answered" && e["accepted"] == true));
    }
    fx.agent_call_ok(
        &helper,
        "send_result",
        json!({"requestId": id, "text": "A centered card in omelette.html"}),
    );
    let result: Vec<Value> = fx
        .entries(&release)
        .into_iter()
        .filter(|e| e["type"] == "request_result")
        .collect();
    assert_eq!(result.len(), 1, "{result:?}");
    assert_eq!(result[0]["text"], "A centered card in omelette.html");
    assert_eq!(result[0]["channel_id"], design.as_str());
    let (rook, work) = fx.vision_and_channel("Rook", "work");
    let (_, worker) = fx.vision_run(&rook, &work).await;
    assert!(!fx
        .agent_call(
            &worker,
            "send_result",
            json!({"requestId": id, "text": "x"})
        )
        .status
        .success());
    fx.shut_down().await;
}
