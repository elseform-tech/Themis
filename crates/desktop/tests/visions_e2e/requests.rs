//! Requests between channels through the real app server: one chief asks another, which
//! answers through its own credential; both channels show the exchange.

use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn a_request_goes_out_and_its_answer_comes_back() {
    let _turn = ONE_SERVER.lock().await;
    let fx = fixture().await;
    let (juno, release) = fx.chief_and_channel("Juno", "release");
    let (tess, api) = fx.chief_and_channel("Tess", "api");
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
    let (tess, api) = fx.chief_and_channel("Tess", "api");
    let (_, asker) = fx.vision_run(&juno, &release).await;
    let (_, helper) = fx.vision_run(&tess, &api).await;
    let ask = |what: &str| {
        let asked = fx.agent_call_ok(
            &asker,
            "request_channel",
            json!({"channelId": api, "ask": what}),
        );
        asked["requestId"].as_str().unwrap().to_owned()
    };
    let (first, second) = (ask("one"), ask("two"));
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
        json!({"requestId": second, "accept": false}),
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
