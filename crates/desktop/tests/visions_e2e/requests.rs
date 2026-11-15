//! Requests between channels through the real app server: one chief asks another, nobody
//! answers it, and the receiving channel takes it on when its chief can run; both channels show
//! the exchange.

use super::*;

/// A request to a paused chief waits; nobody answers it (neither the chief nor the person has
/// a call for that); when the chief's next run starts, it is taken on in both channels.
#[tokio::test(flavor = "multi_thread")]
async fn a_waiting_request_is_taken_on_when_its_chief_next_runs() {
    let _turn = ONE_SERVER.lock().await;
    let fx = fixture().await;
    let provider = quick_provider().await;
    fx.use_provider(&provider).await;
    let (juno, release) = fx.chief_and_channel("Juno", "release");
    let (tess, api) = fx.chief_and_channel("Tess", "api");
    let (_, asker) = fx.vision_run(&juno, &release).await;
    let (_, helper) = fx.vision_run(&tess, &api).await;
    fx.call_ok("pause_vision", json!({"visionId": tess, "paused": true}));
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
    let taken = |channel: &str| {
        fx.entries(channel)
            .into_iter()
            .filter(|e| e["type"] == "request_answered" && e["request_id"] == id.as_str())
            .count()
    };
    assert_eq!(taken(&api), 0, "its chief is paused");
    // Nobody answers a request: the chief's and the person's answer calls are gone.
    let answer = json!({"requestId": id, "accept": true});
    for token in [&asker, &helper] {
        assert!(!fx
            .agent_call(token, "answer_request", answer.clone())
            .status
            .success());
    }
    assert!(!fx.call("answer_channel_request", answer).status.success());
    assert_eq!(taken(&api), 0);

    // Resumed, and woken by the person: its run starts and takes the request on.
    fx.call_ok("pause_vision", json!({"visionId": tess, "paused": false}));
    fx.set_visions(json!({}));
    fx.call_ok(
        "post_channel_message",
        json!({"channelId": api, "text": "@Tess anything waiting?"}),
    );
    let refused = || {
        fx.entries(&api)
            .iter()
            .any(|e| e["type"] == "notice" && e["kind"] == "sandbox_unavailable")
    };
    wait_until(|| async { taken(&api) == 1 || refused() }).await;
    if refused() {
        eprintln!("skipping: no sandbox here can confine a Vision run");
        fx.shut_down().await;
        return;
    }
    assert_eq!(taken(&release), 1);
    let line = fx
        .entries(&api)
        .into_iter()
        .find(|e| e["type"] == "request_answered")
        .unwrap();
    assert_eq!(line["vision_id"], tess.as_str());
    assert_eq!(line["accepted"], true);
    // A worker cannot send requests.
    fx.no_wakes();
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
    // Prices known, so the receiving channel's budget allows taking the request on.
    let provider = quick_provider().await;
    fx.use_provider(&provider).await;
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
